from __future__ import annotations

import ctypes
import logging
import os
import signal
import subprocess
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

import pulsectl

logger = logging.getLogger('NCManager')

# ── LADSPA plugin identifiers ──────────────────────────────────────────────
# Plugin filenames (without .so); labels are the LADSPA label string.
# swh-plugins: sudo dnf install ladspa-swh-plugins  /  sudo apt install swh-plugins
# rnnoise (Fedora): sudo dnf copr enable ycollet/audinux && sudo dnf install ladspa-noise-suppression-for-voice
# rnnoise (Debian/Ubuntu): sudo apt install ladspa-plugin-rnnoise

RNNOISE_PLUGIN = 'librnnoise_ladspa'    # /usr/lib64/ladspa/librnnoise_ladspa.so
RNNOISE_LABEL  = 'noise_suppressor_mono'

# swh-plugins ships with numeric-suffix filenames (e.g. gate_1410.so on Fedora/Debian).
# Some distros may use bare names — each list is tried in order; first hit wins.
# Each entry: (so_filename_without_extension, ladspa_label)
_HPF_CANDIDATES  = [('highpass_iir_1890', 'highpass_iir'), ('hpf', 'hpf')]
HPF_CONTROLS     = '90,1'   # cutoff=90 Hz, stages=1 (2-pole)

_GATE_CANDIDATES = [('gate_1410', 'gate'), ('gate', 'gate')]

_COMP_CANDIDATES = [('sc4_1882', 'sc4'), ('sc4', 'sc4')]

# Mono compressor for the native filter-chain graph: the whole graph must be
# single-channel for filter-chain to auto-link stages, and sc4 is stereo-only.
_COMP_MONO_CANDIDATES = [('sc4m_1916', 'sc4m'), ('sc4m', 'sc4m')]

# Fixed virtual source that apps should select once and always have available.
ARCTIS_NC_SINK = 'Arctis_NC_Sink'
ARCTIS_NC_MIC  = 'Arctis_NC_Mic'
ARCTIS_NC_MIC_DESC = 'Arctis Manager NC Mic'

# Human-readable names for each chain stage, shown as the source's
# node.description in OS sound settings so it reads as internal plumbing.
# (Legacy PulseAudio-module path only.)
_STAGE_DESCRIPTIONS = {
    'HPF':     'Arctis NC: High-pass filter (internal)',
    'RNNoise': 'Arctis NC: Noise suppression (internal)',
    'Gate':    'Arctis NC: Noise gate (internal)',
    'Comp':    'Arctis NC: Compressor (internal)',
}

# ── Native filter-chain path ───────────────────────────────────────────────
# The whole NC chain runs as ONE libpipewire-module-filter-chain graph inside
# a dedicated `pipewire -c <generated conf>` subprocess, exposing a single
# Audio/Source node (ARCTIS_NC_MIC). The graph's capture side is a plain
# stream, invisible in OS sound settings, so no per-stage sources appear.
#
# All stage plugins found on the system are always baked into the graph;
# a stage the user disables is neutralized via its controls (HPF cutoff to
# sub-audible, gate to its bypass port, compressor to unity) instead of being
# removed, so settings changes never rebuild the graph — they are pushed live
# to the running node's Props param (keyed "<stage>:<LADSPA port name>") and
# the source device never disappears from running apps.

ARCTIS_NC_INPUT = 'Arctis_NC_Mic_input'   # capture-side stream node name

# Exact control port names (LADSPA names from analyseplugin; HPF is the
# PipeWire-builtin biquad), used both in the generated graph config and as
# live-update Props keys.
_HPF_PORTS  = ('Freq', 'Q')
_GATE_PORTS = ('LF key filter (Hz)', 'HF key filter (Hz)', 'Threshold (dB)',
               'Attack (ms)', 'Hold (ms)', 'Decay (ms)', 'Range (dB)',
               'Output select (-1 = key listen, 0 = gate, 1 = bypass)')
_COMP_PORTS = ('RMS/peak', 'Attack time (ms)', 'Release time (ms)',
               'Threshold level (dB)', 'Ratio (1:n)', 'Knee radius (dB)',
               'Makeup gain (dB)')


def _native_tools_available() -> bool:
    import shutil
    return all(shutil.which(t) for t in ('pipewire', 'pw-cli', 'pw-dump'))


def _set_pdeathsig() -> None:
    """Kill the filter-chain subprocess if the daemon dies without teardown."""
    libc = ctypes.CDLL('libc.so.6', use_errno=True)
    PR_SET_PDEATHSIG = 1
    libc.prctl(PR_SET_PDEATHSIG, signal.SIGTERM)


def _hpf_controls(enabled: bool) -> dict[str, float]:
    # PipeWire builtin bq_highpass. Disabled: 10 Hz cutoff — sub-audible,
    # measured transparent (unlike the swh highpass_iir LADSPA plugin, whose
    # IIR goes unstable at very low normalized cutoffs).
    return {
        _HPF_PORTS[0]: 90.0 if enabled else 10.0,
        _HPF_PORTS[1]: 0.707,
    }


def _gate_controls(cfg: GateConfig) -> dict[str, float]:
    # Disabled: the gate plugin has a true bypass on its output-select port.
    # Hold port minimum is 2 ms; range port minimum is -90 dB.
    return {
        _GATE_PORTS[0]: 150.0,
        _GATE_PORTS[1]: 4000.0,
        _GATE_PORTS[2]: float(cfg.threshold),
        _GATE_PORTS[3]: float(cfg.attack),
        _GATE_PORTS[4]: 2.0,
        _GATE_PORTS[5]: float(cfg.release),
        _GATE_PORTS[6]: float(max(-90, cfg.reduction)),
        _GATE_PORTS[7]: 0 if cfg.enabled else 1,
    }


def _comp_controls(cfg: CompressorConfig) -> dict[str, float]:
    # Disabled: ratio 1:1 with no makeup gain is unity. Threshold port
    # minimum is -30 dB.
    return {
        _COMP_PORTS[0]: 0.0,
        _COMP_PORTS[1]: 20.0,
        _COMP_PORTS[2]: 150.0,
        _COMP_PORTS[3]: float(max(-30, cfg.threshold)),
        _COMP_PORTS[4]: cfg.ratio / 10 if cfg.enabled else 1.0,
        _COMP_PORTS[5]: 1.0,
        _COMP_PORTS[6]: float(cfg.makeup) if cfg.enabled else 0.0,
    }

_LADSPA_SEARCH_PATHS = [
    '/usr/lib/ladspa',
    '/usr/lib64/ladspa',
    '/usr/local/lib/ladspa',
    '/usr/local/lib64/ladspa',
    os.path.expanduser('~/.ladspa'),
]


def _plugin_available(plugin: str) -> bool:
    paths: list[str] = []
    env = os.environ.get('LADSPA_PATH', '')
    if env:
        paths.extend(p for p in env.split(':') if p)
    paths.extend(_LADSPA_SEARCH_PATHS)
    return any(os.path.isfile(os.path.join(d, f'{plugin}.so')) for d in paths)


_RNNOISE_CANDIDATES = [RNNOISE_PLUGIN, 'rnnoise_ladspa']


def _find_plugin(candidates: list[tuple[str, str]]) -> tuple[str, str] | None:
    """Return (so_filename, label) for first available candidate, or None."""
    return next(((p, l) for p, l in candidates if _plugin_available(p)), None)


def rnnoise_available() -> bool:
    return any(_plugin_available(p) for p in _RNNOISE_CANDIDATES)


def _rnnoise_plugin_name() -> str | None:
    return next((p for p in _RNNOISE_CANDIDATES if _plugin_available(p)), None)


def swh_available() -> bool:
    return _find_plugin(_GATE_CANDIDATES) is not None and _find_plugin(_COMP_CANDIDATES) is not None


# ── Config dataclasses ─────────────────────────────────────────────────────

@dataclass
class GateConfig:
    enabled:   bool = False
    threshold: int  = -42   # dB
    reduction: int  = -72   # dB (negative attenuation)
    attack:    int  = 2     # ms
    release:   int  = 450   # ms

    def ladspa_controls(self) -> str:
        # gate control ports: LF key filter (Hz), HF key filter (Hz),
        # threshold (dB), attack (ms), hold (ms), decay (ms), range (dB), output select
        return f'150,4000,{self.threshold},{self.attack},0,{self.release},{self.reduction},0'

    def summary(self) -> str:
        return (f'gate(enabled={self.enabled}, thr={self.threshold} dB, '
                f'red={self.reduction} dB, atk={self.attack} ms, rel={self.release} ms)')


@dataclass
class CompressorConfig:
    enabled:   bool = False
    threshold: int  = -18   # dB
    ratio:     int  = 18    # stored as 10× (1.8 → 18)
    makeup:    int  = 4     # dB

    def ladspa_controls(self) -> str:
        # sc4 control ports: RMS/peak, attack (ms), release (ms),
        # threshold (dB), ratio (n:1), knee radius (dB), makeup gain (dB)
        return f'0,20,150,{self.threshold},{self.ratio / 10:.1f},1,{self.makeup}'

    def summary(self) -> str:
        return (f'comp(enabled={self.enabled}, thr={self.threshold} dB, '
                f'ratio={self.ratio / 10:.1f}:1, makeup=+{self.makeup} dB)')


@dataclass
class NCConfig:
    preset:      str             = 'off'
    source_id:   str             = ''
    hpf_enabled: bool            = False
    gate:        GateConfig      = field(default_factory=GateConfig)
    compressor:  CompressorConfig = field(default_factory=CompressorConfig)

    @property
    def active(self) -> bool:
        return self.preset != 'off'

    def summary(self) -> str:
        return (f'preset={self.preset!r}  source={self.source_id!r}  '
                f'hpf={self.hpf_enabled}  '
                f'{self.gate.summary()}  '
                f'{self.compressor.summary()}')


# ── Manager ────────────────────────────────────────────────────────────────

class NCManager:
    def __init__(self) -> None:
        self._pulse: pulsectl.Pulse | None = None
        self._chain_modules: list[int] = []     # LADSPA source modules (legacy path)
        self._loopback_module: int | None = None
        self._null_sink_module: int | None = None
        self._counter = 0
        # Native filter-chain state
        self._proc: subprocess.Popen | None = None
        self._input_node_id: int | None = None
        self._graph_source: str = ''            # source_id baked into running graph
        self._graph_stages: tuple[str, ...] = ()  # stage names baked into running graph

    def _pulse_conn(self) -> pulsectl.Pulse:
        if self._pulse is None:
            self._pulse = pulsectl.Pulse('arctis-nc-manager')
        return self._pulse

    def _next_name(self, stage: str) -> str:
        name = f'{ARCTIS_NC_SINK}_{stage}_{self._counter}'
        self._counter += 1
        return name

    # ── Public API ────────────────────────────────────────────────────

    def apply(self, config: NCConfig) -> bool:
        """
        Build (or update) the NC chain for config.source_id.
        Logs one INFO line with the full preset + parameter summary.
        Returns True on success, False on failure.

        Prefers the native single-node filter-chain path; falls back to the
        legacy per-plugin PulseAudio module chain if native tools are missing
        or the graph fails to come up.
        """
        logger.info('Applying NC: %s', config.summary())

        if not config.active:
            self._stop_native()
            self._teardown_chain()
            logger.info('NC disabled (preset=off) — chain torn down')
            return True

        if not config.source_id:
            logger.error('NC apply failed: no source_id configured')
            return False

        rnnoise_plugin = _rnnoise_plugin_name()
        if not rnnoise_plugin:
            logger.error('NC apply failed: RNNoise LADSPA plugin not found (tried: %s)',
                         ', '.join(f'{p}.so' for p in _RNNOISE_CANDIDATES))
            return False

        if _native_tools_available():
            if self._apply_native(config, rnnoise_plugin):
                # Sweep any leftovers from a previous legacy-path run.
                self._teardown_chain()
                return True
            logger.warning('NC: native filter-chain path failed — '
                           'falling back to PulseAudio module chain')
            self._stop_native()

        return self._apply_legacy(config, rnnoise_plugin)

    def _apply_legacy(self, config: NCConfig, rnnoise_plugin: str) -> bool:
        """Per-plugin module-ladspa-source chain (one PA source per stage)."""
        self._teardown_chain()
        pulse = self._pulse_conn()
        current = config.source_id
        stages_loaded: list[str] = ['<physical>']

        # Stage 1: HPF (optional, requires swh-plugins)
        if config.hpf_enabled:
            hpf = _find_plugin(_HPF_CANDIDATES)
            if not hpf:
                logger.error('NC: HPF requested but no HPF plugin found — skipping stage')
            else:
                hpf_plugin, hpf_label = hpf
                name = self._next_name('HPF')
                ok = self._load_ladspa_source(pulse, name, current,
                                              hpf_plugin, hpf_label, HPF_CONTROLS, 'HPF')
                if ok:
                    current = name
                    stages_loaded.append('HPF')
                else:
                    logger.error('NC: HPF stage failed — continuing without it')

        # Stage 2: RNNoise (always, when NC is active)
        name = self._next_name('RNNoise')
        ok = self._load_ladspa_source(pulse, name, current,
                                      rnnoise_plugin, RNNOISE_LABEL, '', 'RNNoise')
        if not ok:
            logger.error('NC apply failed: RNNoise stage could not be loaded')
            self._teardown_chain()
            return False
        current = name
        stages_loaded.append('RNNoise')

        # Stage 3: Gate (optional, requires swh-plugins)
        if config.gate.enabled:
            gate = _find_plugin(_GATE_CANDIDATES)
            if not gate:
                logger.error('NC: gate requested but no gate plugin found — skipping stage')
            else:
                gate_plugin, gate_label = gate
                name = self._next_name('Gate')
                ok = self._load_ladspa_source(pulse, name, current,
                                              gate_plugin, gate_label,
                                              config.gate.ladspa_controls(), 'Gate')
                if ok:
                    current = name
                    stages_loaded.append('Gate')
                else:
                    logger.error('NC: gate stage failed — continuing without it')

        # Stage 4: Compressor (optional, requires swh-plugins)
        if config.compressor.enabled:
            comp = _find_plugin(_COMP_CANDIDATES)
            if not comp:
                logger.error('NC: compressor requested but no compressor plugin found — skipping stage')
            else:
                comp_plugin, comp_label = comp
                name = self._next_name('Comp')
                ok = self._load_ladspa_source(pulse, name, current,
                                              comp_plugin, comp_label,
                                              config.compressor.ladspa_controls(), 'Comp')
                if ok:
                    current = name
                    stages_loaded.append('Comp')
                else:
                    logger.error('NC: compressor stage failed — continuing without it')

        # Null sink + loopback for a stable, fixed virtual source name
        null_sink_ok = self._ensure_null_sink(pulse)
        if not null_sink_ok:
            logger.error('NC apply failed: could not create null sink %r', ARCTIS_NC_SINK)
            self._teardown_chain()
            return False

        loopback_ok = self._load_loopback(pulse, current)
        if not loopback_ok:
            logger.error('NC apply failed: could not create loopback from %r to %r', current, ARCTIS_NC_SINK)
            self._teardown_chain()
            return False

        chain_str = ' → '.join(stages_loaded + [ARCTIS_NC_MIC])
        logger.info('NC chain active: %s', chain_str)
        logger.info('NC virtual source ready: apps should select "%s"', ARCTIS_NC_MIC)
        return True

    # ── Native filter-chain path ──────────────────────────────────────

    def _apply_native(self, config: NCConfig, rnnoise_plugin: str) -> bool:
        stages = self._native_stages(config, rnnoise_plugin)
        stage_names = tuple(stage[0] for stage in stages)

        if (self._proc is not None and self._proc.poll() is None
                and self._graph_source == config.source_id
                and self._graph_stages == stage_names):
            return self._native_update_controls(stages)

        self._stop_native()
        conf_path = self._write_native_conf(config, stages)
        try:
            self._proc = subprocess.Popen(
                ['pipewire', '-c', str(conf_path)],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                preexec_fn=_set_pdeathsig,
            )
        except Exception as e:
            logger.error('NC: failed to spawn filter-chain process: %s', e)
            return False

        node_id = self._wait_for_input_node(timeout=3.0)
        if node_id is None:
            logger.error('NC: filter-chain node did not appear — aborting native path')
            self._stop_native()
            return False

        self._input_node_id = node_id
        self._graph_source = config.source_id
        self._graph_stages = stage_names
        chain_str = ' → '.join(['<physical>'] + list(stage_names) + [ARCTIS_NC_MIC])
        logger.info('NC native chain active (single node): %s', chain_str)
        logger.info('NC virtual source ready: apps should select "%s"', ARCTIS_NC_MIC)
        return True

    def _native_stages(self, config: NCConfig,
                       rnnoise_plugin: str) -> list[tuple[str, str, str, str, dict[str, float]]]:
        """Return (name, type, plugin, label, controls) for every graph stage.

        Every stage whose plugin exists on the system is included regardless
        of its enabled flag; disabled stages get neutral/bypass controls so
        toggling never changes the graph shape. The HPF is PipeWire's builtin
        biquad, so it is always available.
        """
        stages: list[tuple[str, str, str, str, dict[str, float]]] = []

        stages.append(('hpf', 'builtin', '', 'bq_highpass',
                       _hpf_controls(config.hpf_enabled)))

        stages.append(('rnnoise', 'ladspa', rnnoise_plugin, RNNOISE_LABEL, {}))

        gate = _find_plugin(_GATE_CANDIDATES)
        if gate:
            stages.append(('gate', 'ladspa', gate[0], gate[1], _gate_controls(config.gate)))
        elif config.gate.enabled:
            logger.error('NC: gate requested but no gate plugin found — skipping stage')

        comp = _find_plugin(_COMP_MONO_CANDIDATES)
        if comp:
            stages.append(('comp', 'ladspa', comp[0], comp[1], _comp_controls(config.compressor)))
        elif config.compressor.enabled:
            logger.error('NC: compressor requested but no mono compressor plugin '
                         '(sc4m) found — skipping stage')

        return stages

    def _write_native_conf(self, config: NCConfig,
                           stages: list[tuple[str, str, str, str, dict[str, float]]]) -> Path:
        nodes = []
        for name, node_type, plugin, label, controls in stages:
            block  = ('                    {\n'
                      f'                        type   = {node_type}\n'
                      f'                        name   = {name}\n')
            if plugin:
                block += f'                        plugin = {plugin}\n'
            block += f'                        label  = {label}\n'
            if controls:
                block += '                        control = {\n'
                for port, value in controls.items():
                    block += f'                            "{port}" = {value}\n'
                block += '                        }\n'
            block += '                    }'
            nodes.append(block)
        nodes_str = '\n'.join(nodes)

        # Explicit links between consecutive stages: multi-node auto-linking
        # is unreliable on PipeWire 1.6.x (graph silently passes no audio).
        # Builtin nodes expose In/Out ports; LADSPA nodes Input/Output.
        def out_port(stage): return 'Out' if stage[1] == 'builtin' else 'Output'
        def in_port(stage):  return 'In'  if stage[1] == 'builtin' else 'Input'
        links = [
            f'                    {{ output = "{a[0]}:{out_port(a)}" '
            f'input = "{b[0]}:{in_port(b)}" }}'
            for a, b in zip(stages, stages[1:])
        ]
        links_str = '\n'.join(links)

        mic_desc = ARCTIS_NC_MIC_DESC
        conf = f'''# Generated by Arctis Manager — noise-cancellation filter chain.
# All stages run inside this single graph; only one source node is exposed.
context.properties = {{
    log.level = 2
}}

context.spa-libs = {{
    audio.convert.* = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}}

context.modules = [
    {{ name = libpipewire-module-rt
        args = {{ }}
        flags = [ ifexists nofail ]
    }}
    {{ name = libpipewire-module-protocol-native }}
    {{ name = libpipewire-module-client-node }}
    {{ name = libpipewire-module-adapter }}
    {{ name = libpipewire-module-filter-chain
        args = {{
            node.description = "{mic_desc}"
            media.name       = "{mic_desc}"
            filter.graph = {{
                nodes = [
{nodes_str}
                ]
                links = [
{links_str}
                ]
            }}
            audio.position = [ FL FR ]
            capture.props = {{
                node.name    = "{ARCTIS_NC_INPUT}"
                node.passive = true
                node.always-process = true
                target.object = "{config.source_id}"
            }}
            playback.props = {{
                node.name    = "{ARCTIS_NC_MIC}"
                media.class  = Audio/Source
                # Present as a regular (non-virtual) device so KDE's volume
                # applet lists it alongside real microphones; virtual/filter
                # devices are hidden there by default.
                node.virtual = false
                device.class = "sound"
            }}
        }}
    }}
]
'''
        conf_dir = Path(os.environ.get('XDG_RUNTIME_DIR', tempfile.gettempdir())) / 'arctis-manager'
        conf_dir.mkdir(parents=True, exist_ok=True)
        conf_path = conf_dir / 'nc-filter-chain.conf'
        conf_path.write_text(conf)
        return conf_path

    def _wait_for_input_node(self, timeout: float) -> int | None:
        import json
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self._proc is not None and self._proc.poll() is not None:
                logger.error('NC: filter-chain process exited with code %s', self._proc.returncode)
                return None
            try:
                out = subprocess.run(['pw-dump'], capture_output=True, text=True,
                                     timeout=2, check=True).stdout
                for obj in json.loads(out):
                    if obj.get('type') != 'PipeWire:Interface:Node':
                        continue
                    props = obj.get('info', {}).get('props', {})
                    if props.get('node.name') == ARCTIS_NC_INPUT:
                        return obj.get('id')
            except Exception as e:
                logger.debug('NC: pw-dump poll failed: %s', e)
            time.sleep(0.2)
        return None

    def _native_update_controls(
            self, stages: list[tuple[str, str, str, str, dict[str, float]]]) -> bool:
        """Push current (possibly neutral) controls to the running graph in place."""
        if self._input_node_id is None:
            return False
        pairs = []
        for name, _, _, _, controls in stages:
            for port, value in controls.items():
                pairs.append(f'"{name}:{port}", {value}')
        if not pairs:
            return True
        spec = '{ params = [ ' + ', '.join(pairs) + ' ] }'
        try:
            subprocess.run(
                ['pw-cli', 's', str(self._input_node_id), 'Props', spec],
                capture_output=True, text=True, timeout=2, check=True,
            )
            logger.info('NC: controls updated live on filter-chain node %d', self._input_node_id)
            return True
        except Exception as e:
            logger.warning('NC: live control update failed: %s', e)
            return False

    def _stop_native(self) -> None:
        if self._proc is not None:
            try:
                self._proc.terminate()
                self._proc.wait(timeout=2)
            except Exception:
                try:
                    self._proc.kill()
                except Exception:
                    pass
            logger.debug('NC: filter-chain process stopped')
        self._proc = None
        self._input_node_id = None
        self._graph_source = ''
        self._graph_stages = ()

    def teardown(self) -> None:
        logger.info('NC teardown: unloading chain')
        self._stop_native()
        self._teardown_chain()
        if self._pulse:
            try:
                self._pulse.close()
            except Exception:
                pass
            self._pulse = None

    # ── Internal helpers ──────────────────────────────────────────────

    def _load_ladspa_source(self, pulse: pulsectl.Pulse, name: str,
                             master: str, plugin: str, label: str,
                             controls: str, stage: str) -> bool:
        description = _STAGE_DESCRIPTIONS.get(stage, name).replace(' ', '\\ ')
        args = (f'source_name={name} master={master} '
                f'plugin={plugin} label={label} '
                f'source_properties=node.description="{description}"')
        if controls:
            args += f' control={controls}'
        logger.debug('module-ladspa-source args: %s', args)
        try:
            idx = pulse.module_load('module-ladspa-source', args)
            self._chain_modules.append(idx)
            logger.info('NC LADSPA source %r loaded (module %d)', name, idx)
            return True
        except Exception as e:
            logger.error('Failed to load NC LADSPA source %r [%s/%s]: %s', name, plugin, label, e)
            return False

    def _ensure_null_sink(self, pulse: pulsectl.Pulse) -> bool:
        try:
            sinks = pulse.sink_list()
            existing = next(
                (s for s in sinks if s.proplist.get('node.name', '') == ARCTIS_NC_SINK
                 or s.name == ARCTIS_NC_SINK),
                None,
            )
            if existing:
                logger.debug('NC null sink %r already exists', ARCTIS_NC_SINK)
                return True
            sink_desc = 'Arctis\\ NC\\ Output\\ (internal)'
            source_desc = ARCTIS_NC_MIC_DESC.replace(' ', '\\ ')
            idx = pulse.module_load(
                'module-null-sink',
                f'sink_name={ARCTIS_NC_SINK} '
                f'sink_properties=node.description="{sink_desc}" '
                f'source_name={ARCTIS_NC_MIC} '
                f'source_properties=node.description="{source_desc}"',
            )
            self._null_sink_module = idx
            logger.info('NC null sink %r created (module %d)', ARCTIS_NC_SINK, idx)
            return True
        except Exception as e:
            logger.error('Failed to create NC null sink: %s', e)
            return False

    def _load_loopback(self, pulse: pulsectl.Pulse, source_name: str) -> bool:
        try:
            idx = pulse.module_load(
                'module-loopback',
                f'source={source_name} sink={ARCTIS_NC_SINK} latency_msec=1',
            )
            self._loopback_module = idx
            logger.info('NC loopback %r → %r loaded (module %d)', source_name, ARCTIS_NC_SINK, idx)
            return True
        except Exception as e:
            logger.error('Failed to load NC loopback: %s', e)
            return False

    def _teardown_chain(self) -> None:
        if not self._chain_modules and self._loopback_module is None and self._null_sink_module is None:
            return
        pulse = self._pulse_conn()
        for mod_id in list(reversed(self._chain_modules)):
            try:
                pulse.module_unload(mod_id)
                logger.debug('NC: unloaded module %d', mod_id)
            except Exception as e:
                logger.warning('NC: failed to unload module %d: %s', mod_id, e)
        self._chain_modules.clear()

        for mod_id, label in [
            (self._loopback_module, 'loopback'),
            (self._null_sink_module, 'null sink'),
        ]:
            if mod_id is not None:
                try:
                    pulse.module_unload(mod_id)
                    logger.debug('NC: unloaded %s (module %d)', label, mod_id)
                except Exception as e:
                    logger.warning('NC: failed to unload %s %d: %s', label, mod_id, e)
        self._loopback_module = None
        self._null_sink_module = None
