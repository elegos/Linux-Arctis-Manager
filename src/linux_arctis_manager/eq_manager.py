from __future__ import annotations

import json
import logging
import shutil
import subprocess
import threading
from dataclasses import dataclass, field

import pulsectl

from linux_arctis_manager.app_matcher import AppEQOverride
from linux_arctis_manager.constants import PULSE_CHAT_NODE_NAME, PULSE_MEDIA_NODE_NAME
from linux_arctis_manager.eq_preset import EQMode, EQPreset

logger = logging.getLogger('EQManager')

LADSPA_PLUGIN  = 'mbeq_1197'
LADSPA_LABEL   = 'mbeq'
EQ_SINK_SUFFIX = '_EQ_internal'

# mbeq_1197 control port names, in the same order as EQPreset.to_ladspa_controls().
# Used to push gain changes to an already-running filter node live, via PipeWire's
# native protocol, instead of reloading the module (which recreates the sink).
LADSPA_CONTROL_NAMES = [
    '50Hz gain (low shelving)', '100Hz gain', '156Hz gain', '220Hz gain',
    '311Hz gain', '440Hz gain', '622Hz gain', '880Hz gain', '1250Hz gain',
    '1750Hz gain', '2500Hz gain', '3500Hz gain', '5000Hz gain',
    '10000Hz gain', '20000Hz gain',
]

# All-zero gains == unity-gain passthrough for mbeq_1197; used to "disable"
# EQ on a channel without tearing down its sink.
NEUTRAL_CONTROLS = [0.0] * 15

_PW_CLI_AVAILABLE  = shutil.which('pw-cli') is not None
_PW_DUMP_AVAILABLE = shutil.which('pw-dump') is not None


def _pipewire_node_id(sink_name: str) -> int | None:
    """Look up the PipeWire node id of a running sink by name, via pw-dump."""
    try:
        out = subprocess.run(
            ['pw-dump'], capture_output=True, text=True, timeout=2, check=True,
        ).stdout
        for obj in json.loads(out):
            if obj.get('type') != 'PipeWire:Interface:Node':
                continue
            props = obj.get('info', {}).get('props', {})
            if props.get('node.name') == sink_name and props.get('media.class') == 'Audio/Sink':
                return obj.get('id')
    except Exception as e:
        logger.debug('pw-dump node lookup failed for %r: %s', sink_name, e)
    return None


def _pipewire_set_controls(node_id: int, controls: list[float]) -> bool:
    """Push new LADSPA control values to a running filter-chain node in place.

    Writes the node's Props param over the native PipeWire protocol (pw-cli),
    not PulseAudio's — module-ladspa-sink's control values have no PulseAudio
    equivalent, but PipeWire exposes them as a writable Props param, keyed by
    control name, on the sink's adapter node. This changes gains without
    reloading the module, so no sink is added/removed and no stream is moved.
    """
    pairs = ', '.join(
        f'"{name}", {value:.4f}' for name, value in zip(LADSPA_CONTROL_NAMES, controls)
    )
    spec = f'{{ params = [ {pairs} ] }}'
    try:
        subprocess.run(
            ['pw-cli', 's', str(node_id), 'Props', spec],
            capture_output=True, text=True, timeout=2, check=True,
        )
        return True
    except Exception as e:
        logger.debug('pw-cli control update failed for node %d: %s', node_id, e)
        return False

_LADSPA_SEARCH_PATHS = [
    '/usr/lib/ladspa',
    '/usr/lib64/ladspa',
    '/usr/local/lib/ladspa',
    '/usr/local/lib64/ladspa',
]


def ladspa_plugin_available(plugin: str = LADSPA_PLUGIN) -> bool:
    """Return True if the LADSPA .so file is found in any standard search path."""
    import os
    paths: list[str] = []
    env = os.environ.get('LADSPA_PATH', '')
    if env:
        paths.extend(p for p in env.split(':') if p)
    paths.extend(_LADSPA_SEARCH_PATHS)
    paths.append(os.path.expanduser('~/.ladspa'))
    return any(os.path.isfile(os.path.join(d, f'{plugin}.so')) for d in paths)


@dataclass
class ChannelEQConfig:
    enabled: bool = False
    mode: EQMode = 'simple'
    preset: EQPreset | None = None


@dataclass
class EQConfig:
    media: ChannelEQConfig = field(default_factory=ChannelEQConfig)
    chat: ChannelEQConfig = field(default_factory=ChannelEQConfig)
    app_overrides: list[AppEQOverride] = field(default_factory=list)


class EQManager:
    def __init__(self) -> None:
        self._pulse: pulsectl.Pulse | None = None
        self._active_modules: dict[str, int] = {}   # channel -> currently-routed module index
        self._active_names:   dict[str, str] = {}   # channel -> currently-routed sink name
        self._stale_modules:  dict[str, int] = {}   # channel -> replaced module index, pending unload
        self._all_modules:    list[int]       = []   # every module loaded this session (for teardown)
        self._sink_counter:   int             = 0    # monotonic counter for unique sink names
        self._config: EQConfig = EQConfig()
        self._monitor_thread: threading.Thread | None = None
        self._stopping = False

    def _get_pulse(self) -> pulsectl.Pulse:
        if self._pulse is None:
            self._pulse = pulsectl.Pulse('arctis-eq-manager')
        return self._pulse

    def _next_sink_name(self, channel: str) -> str:
        """Return a unique sink name for this channel, advancing the counter."""
        node = PULSE_MEDIA_NODE_NAME if channel == 'media' else PULSE_CHAT_NODE_NAME
        name = f'{node}{EQ_SINK_SUFFIX}_{self._sink_counter}'
        self._sink_counter += 1
        return name

    # ------------------------------------------------------------------
    # Initial setup (called once when the headset connects)
    # ------------------------------------------------------------------

    def setup(self, physical_sink_name: str, config: EQConfig) -> dict[str, str]:
        """Create LADSPA EQ sinks for initial device setup.

        Returns {channel: sink_name} — the targets for the null-sink loopbacks.
        """
        self._config = config
        targets: dict[str, str] = {'media': physical_sink_name, 'chat': physical_sink_name}

        for channel, cfg in [('media', config.media), ('chat', config.chat)]:
            if not cfg.enabled or cfg.preset is None:
                continue
            name = self._next_sink_name(channel)
            if self._create_ladspa_sink(channel, name, physical_sink_name, cfg.preset.to_ladspa_controls()):
                targets[channel] = name

        return targets

    # ------------------------------------------------------------------
    # Live EQ update (called on every preset / gain change)
    # ------------------------------------------------------------------

    def reapply(self, physical_sink_name: str, config: EQConfig) -> tuple[dict[str, str], set[str]]:
        """Apply new EQ settings, updating gains in place where possible.

        Returns (targets, changed_channels): targets is {channel: sink_name}
        for the loopback cables; changed_channels is the subset of targets
        whose sink actually changed and therefore needs its cable rerouted.

        Once a channel has a live LADSPA sink, it keeps routing through that
        same sink for the rest of the session — including while EQ is
        toggled off, which is applied as all-zero ("neutral", unity-gain)
        controls rather than by rerouting back to the physical sink. This
        way toggling EQ on/off is just another live gain push (see
        _pipewire_set_controls): no cable move, no module churn. A channel
        only gets its cable touched the first time it's enabled (no sink
        exists yet) or if a live update fails (e.g. pw-cli unavailable),
        in which case _create_ladspa_sink() loads a fresh module.

        The module a channel replaces this way is kept loaded (idle) until
        the caller confirms the loopback cable has been switched to the new
        sink — see unload_stale_module(). Unloading a module while its
        loopback is still attached broadcasts a "sink removed" event that
        apps like Spotify react to by resetting their audio stream, so the
        old module must outlive the switch, not the whole session.
        """
        self.stop_stream_monitor()
        new_targets: dict[str, str] = {}
        changed_channels: set[str] = set()

        for channel, cfg in [('media', config.media), ('chat', config.chat)]:
            old_idx = self._active_modules.get(channel)
            old_name = self._active_names.get(channel)
            previous_target = old_name or physical_sink_name
            want_eq = cfg.enabled and cfg.preset is not None
            controls = cfg.preset.to_ladspa_controls() if want_eq else NEUTRAL_CONTROLS

            if old_name and self._update_ladspa_controls(old_name, controls):
                # Sink already exists for this channel — gains updated (real
                # or neutral) in place. Cable and module are untouched.
                new_targets[channel] = old_name
                continue

            if want_eq:
                name = self._next_sink_name(channel)
                if self._create_ladspa_sink(channel, name, physical_sink_name, controls):
                    new_targets[channel] = name
                    if old_idx is not None and old_idx != self._active_modules.get(channel):
                        self._stale_modules[channel] = old_idx
                else:
                    # Load failed; fall back to physical for this channel.
                    # Old module (if any) is still active — leave it.
                    new_targets[channel] = old_name or physical_sink_name
            else:
                # EQ off and no sink exists yet (or the live neutral update
                # failed) — stay on/fall back to the physical sink.
                new_targets[channel] = physical_sink_name
                self._active_names.pop(channel, None)
                self._active_modules.pop(channel, None)
                if old_idx is not None:
                    self._stale_modules[channel] = old_idx

            if new_targets[channel] != previous_target:
                changed_channels.add(channel)

        self._config = config
        self.start_stream_monitor()
        return new_targets, changed_channels

    def _update_ladspa_controls(self, sink_name: str, controls: list[float]) -> bool:
        """Try to push new gains to sink_name's running filter node in place."""
        if not (_PW_CLI_AVAILABLE and _PW_DUMP_AVAILABLE):
            return False
        node_id = _pipewire_node_id(sink_name)
        if node_id is None:
            return False
        if _pipewire_set_controls(node_id, controls):
            logger.debug('Live-updated EQ gains for %r (node %d)', sink_name, node_id)
            return True
        return False

    def unload_stale_module(self, channel: str) -> None:
        """Unload the module `channel` was routed through before the most recent reapply().

        Call this only after the loopback cable has been switched to the new
        sink for this channel, so the old module is no longer in use.
        """
        idx = self._stale_modules.pop(channel, None)
        if idx is None or self._pulse is None:
            return
        try:
            self._pulse.module_unload(idx)
            if idx in self._all_modules:
                self._all_modules.remove(idx)
            logger.debug('Unloaded stale EQ module %d for %s', idx, channel)
        except Exception as e:
            logger.warning('Error unloading stale EQ module %d for %s: %s', idx, channel, e)

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _create_ladspa_sink(self, channel: str, eq_sink_name: str,
                             master: str, controls_list: list[float]) -> bool:
        controls_str = ','.join(f'{g:.2f}' for g in controls_list)
        description = f'Arctis {channel.capitalize()} EQ (internal)'.replace(' ', '\\ ')
        args = (
            f'sink_name={eq_sink_name} '
            f'master={master} '
            f'plugin={LADSPA_PLUGIN} '
            f'label={LADSPA_LABEL} '
            f'control={controls_str} '
            f'sink_properties=node.description="{description}"'
        )
        logger.debug('module-ladspa-sink args: %s', args)
        try:
            pulse = self._get_pulse()
            idx = pulse.module_load('module-ladspa-sink', args)
            self._active_modules[channel] = idx
            self._active_names[channel]   = eq_sink_name
            self._all_modules.append(idx)
            logger.info('EQ sink %r loaded for %s (module %d)', eq_sink_name, channel, idx)
            return True
        except Exception as e:
            logger.error('Failed to create EQ sink for %s: %s', channel, e)
            return False

    # ------------------------------------------------------------------
    # Stream monitor (per-app EQ overrides)
    # ------------------------------------------------------------------

    def start_stream_monitor(self) -> None:
        if not self._config.app_overrides:
            return
        self._stopping = False
        self._monitor_thread = threading.Thread(
            target=self._monitor_loop, daemon=True, name='arctis-eq-monitor'
        )
        self._monitor_thread.start()

    def stop_stream_monitor(self) -> None:
        self._stopping = True
        try:
            if self._pulse:
                self._pulse.event_listen_stop()
        except Exception:
            pass

    def _monitor_loop(self) -> None:
        try:
            pulse = pulsectl.Pulse('arctis-eq-stream-monitor')
            pulse.event_mask_set('sink_input')
            pulse.event_callback_set(lambda e: self._on_stream_event(pulse, e))
            while not self._stopping:
                try:
                    pulse.event_listen(timeout=1.0)
                except pulsectl.PulseEventLoopStop:
                    break
                except Exception as e:
                    logger.debug(f'EQ monitor event error: {e}')
            pulse.close()
        except Exception as e:
            logger.error(f'EQ stream monitor failed: {e}')

    def _on_stream_event(self, pulse: pulsectl.Pulse, event: pulsectl.PulseEventInfo) -> None:
        if event.t != pulsectl.PulseEventTypeEnum.new:
            return
        try:
            inputs = pulse.sink_input_list()
            stream = next((s for s in inputs if s.index == event.index), None)
            if stream is None:
                return
            props = dict(stream.proplist)
            for override in self._config.app_overrides:
                if override.matcher.matches(props):
                    self._route_stream(pulse, stream.index, override.channel)
                    break
        except Exception as e:
            logger.debug(f'Error processing stream event: {e}')

    def _route_stream(self, pulse: pulsectl.Pulse, stream_index: int, channel: str) -> None:
        eq_sink_name = self._active_names.get(channel)
        if not eq_sink_name:
            return
        try:
            sinks = pulse.sink_list()
            target = next((s for s in sinks if s.name == eq_sink_name), None)
            if target:
                pulse.sink_input_move(stream_index, target.index)
                logger.info(f'Routed stream {stream_index} to {eq_sink_name}')
        except Exception as e:
            logger.warning(f'Failed to route stream {stream_index}: {e}')

    # ------------------------------------------------------------------
    # Teardown
    # ------------------------------------------------------------------

    def teardown(self) -> None:
        self.stop_stream_monitor()
        if self._all_modules and self._pulse:
            pulse = self._pulse
            for idx in self._all_modules:
                try:
                    pulse.module_unload(idx)
                    logger.debug('Unloaded LADSPA module %d', idx)
                except Exception as e:
                    logger.warning('Error unloading LADSPA module %d: %s', idx, e)
        self._all_modules.clear()
        self._active_modules.clear()
        self._active_names.clear()
        self._stale_modules.clear()
        if self._pulse:
            try:
                self._pulse.close()
            except Exception:
                pass
            self._pulse = None
