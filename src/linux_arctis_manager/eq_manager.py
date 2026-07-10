from __future__ import annotations

import logging
import threading
from dataclasses import dataclass, field

import pulsectl

from linux_arctis_manager.app_matcher import AppEQOverride
from linux_arctis_manager.constants import PULSE_CHAT_NODE_NAME, PULSE_MEDIA_NODE_NAME
from linux_arctis_manager.eq_preset import EQMode, EQPreset

logger = logging.getLogger('EQManager')

LADSPA_PLUGIN  = 'mbeq_1197'
LADSPA_LABEL   = 'mbeq'
EQ_SINK_SUFFIX = '_EQ'

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
            if self._create_ladspa_sink(channel, name, physical_sink_name, cfg):
                targets[channel] = name

        return targets

    # ------------------------------------------------------------------
    # Live EQ update (called on every preset / gain change)
    # ------------------------------------------------------------------

    def reapply(self, physical_sink_name: str, config: EQConfig) -> dict[str, str]:
        """Load new LADSPA modules and return new loopback targets.

        Old modules are intentionally left loaded (idle, suspended by PipeWire).
        Unloading a PulseAudio/PipeWire module broadcasts a "sink removed" event
        that apps like Spotify react to by resetting their audio stream — even
        though the app is not connected to that sink.  Keeping idle modules avoids
        this and is effectively free (PipeWire suspends nodes with no connections).

        All accumulated modules are cleaned up in teardown() when the headset
        disconnects, at which point Spotify's audio is already broken anyway.
        """
        self.stop_stream_monitor()
        new_targets: dict[str, str] = {}

        for channel, cfg in [('media', config.media), ('chat', config.chat)]:
            if cfg.enabled and cfg.preset is not None:
                name = self._next_sink_name(channel)
                if self._create_ladspa_sink(channel, name, physical_sink_name, cfg):
                    new_targets[channel] = name
                else:
                    # Load failed; fall back to physical for this channel.
                    # Old module (if any) is still active — leave it.
                    new_targets[channel] = self._active_names.get(channel, physical_sink_name)
            else:
                new_targets[channel] = physical_sink_name
                # Forget the active name so _route_stream doesn't try to use it.
                self._active_names.pop(channel, None)
                self._active_modules.pop(channel, None)

        self._config = config
        self.start_stream_monitor()
        return new_targets

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _create_ladspa_sink(self, channel: str, eq_sink_name: str,
                             master: str, cfg: ChannelEQConfig) -> bool:
        controls_list = cfg.preset.to_ladspa_controls()  # type: ignore[union-attr]
        controls_str  = ','.join(f'{g:.2f}' for g in controls_list)
        args = (
            f'sink_name={eq_sink_name} '
            f'master={master} '
            f'plugin={LADSPA_PLUGIN} '
            f'label={LADSPA_LABEL} '
            f'control={controls_str}'
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
        if self._pulse:
            try:
                self._pulse.close()
            except Exception:
                pass
            self._pulse = None
