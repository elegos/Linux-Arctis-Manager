from __future__ import annotations

import logging
import os
from dataclasses import dataclass, field

import pulsectl

logger = logging.getLogger('NCManager')

# ── LADSPA plugin identifiers ──────────────────────────────────────────────
# Plugin filenames (without .so); labels are the LADSPA label string.
# swh-plugins: sudo dnf install ladspa-swh-plugins  /  sudo apt install swh-plugins
# rnnoise (Fedora): sudo dnf copr enable ycollet/audinux && sudo dnf install ladspa-noise-suppression-for-voice
# rnnoise (Debian/Ubuntu): sudo apt install ladspa-plugin-rnnoise

RNNOISE_PLUGIN = 'librnnoise_ladspa'    # /usr/lib64/ladspa/librnnoise_ladspa.so
RNNOISE_LABEL  = 'noise_suppressor_mono'
# Control ports: VAD Threshold (%), VAD Grace Period (ms), Retroactive VAD Grace (ms).
# The plugin HARD-MUTES frames its VAD judges non-voice.  At the default 50 %
# threshold quiet consonants score below the line and get chopped — liquids
# after plosives, nasal word endings ("clipping" → "kippin'").  15 % keeps
# quiet-mic consonants; 350 ms grace bridges word-internal dips and endings.
RNNOISE_CONTROLS = '15,350,0'

# swh-plugins ships with numeric-suffix filenames (e.g. gate_1410.so on Fedora/Debian).
# Some distros may use bare names — each list is tried in order; first hit wins.
# Each entry: (so_filename_without_extension, ladspa_label)
_HPF_CANDIDATES  = [('highpass_iir_1890', 'highpass_iir'), ('hpf', 'hpf')]
HPF_CONTROLS     = '90,1'   # cutoff=90 Hz, stages=1 (2-pole)

_GATE_CANDIDATES = [('gate_1410', 'gate'), ('gate', 'gate')]

_COMP_CANDIDATES = [('sc4_1882', 'sc4'), ('sc4', 'sc4')]

# Fixed virtual source that apps should select once and always have available.
ARCTIS_NC_SINK = 'Arctis_NC_Sink'
ARCTIS_NC_MIC  = 'Arctis_NC_Mic'
ARCTIS_NC_MIC_DESC = 'Arctis Manager NC Mic'

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
        self._chain_modules: list[int] = []     # LADSPA source modules
        self._virtual_source_module: int | None = None
        self._counter = 0
        self.output_source: str | None = None   # name of the final chain source; None when inactive

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
        Build (or rebuild) the LADSPA NC chain for config.source_id.
        Logs one INFO line with the full preset + parameter summary.
        Returns True on success, False on failure.
        """
        logger.info('Applying NC: %s', config.summary())

        self._teardown_chain()

        if not config.active:
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
                                              hpf_plugin, hpf_label, HPF_CONTROLS)
                if ok:
                    current = name
                    stages_loaded.append('HPF')
                else:
                    logger.error('NC: HPF stage failed — continuing without it')

        # Stage 2: RNNoise (always, when NC is active)
        name = self._next_name('RNNoise')
        ok = self._load_ladspa_source(pulse, name, current,
                                      rnnoise_plugin, RNNOISE_LABEL, RNNOISE_CONTROLS)
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
                                              config.gate.ladspa_controls())
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
                                              config.compressor.ladspa_controls())
                if ok:
                    current = name
                    stages_loaded.append('Comp')
                else:
                    logger.error('NC: compressor stage failed — continuing without it')

        # Wrap the final LADSPA stage in a stable virtual source so apps don't
        # need to change their mic selection when NC chain settings change.
        if not self._load_virtual_source(pulse, current):
            logger.error('NC apply failed: could not create virtual source %r', ARCTIS_NC_MIC)
            self._teardown_chain()
            return False

        chain_str = ' → '.join(stages_loaded + [ARCTIS_NC_MIC])
        logger.info('NC chain active: %s', chain_str)
        self.output_source = ARCTIS_NC_MIC
        return True

    def teardown(self) -> None:
        logger.info('NC teardown: unloading chain')
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
                             controls: str) -> bool:
        args = (f'source_name={name} master={master} '
                f'plugin={plugin} label={label}')
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

    def _load_virtual_source(self, pulse: pulsectl.Pulse, master: str) -> bool:
        for mod_name in ('module-remap-source', 'module-virtual-source'):
            try:
                idx = pulse.module_load(
                    mod_name,
                    f'source_name={ARCTIS_NC_MIC} '
                    f'master={master} '
                    f'source_properties=node.description="{ARCTIS_NC_MIC_DESC}"',
                )
                self._virtual_source_module = idx
                logger.info('NC source %r via %s master=%r (module %d)',
                            ARCTIS_NC_MIC, mod_name, master, idx)
                return True
            except Exception as e:
                logger.warning('NC %s failed: %s', mod_name, e)
        logger.error('Failed to create NC virtual source (tried remap-source and virtual-source)')
        return False

    def _teardown_chain(self) -> None:
        self.output_source = None
        if not self._chain_modules and self._virtual_source_module is None:
            return
        pulse = self._pulse_conn()

        if self._virtual_source_module is not None:
            try:
                pulse.module_unload(self._virtual_source_module)
                logger.debug('NC: unloaded virtual source (module %d)', self._virtual_source_module)
            except Exception as e:
                logger.warning('NC: failed to unload virtual source %d: %s',
                               self._virtual_source_module, e)
            self._virtual_source_module = None

        for mod_id in list(reversed(self._chain_modules)):
            try:
                pulse.module_unload(mod_id)
                logger.debug('NC: unloaded module %d', mod_id)
            except Exception as e:
                logger.warning('NC: failed to unload module %d: %s', mod_id, e)
        self._chain_modules.clear()
