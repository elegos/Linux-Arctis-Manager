from __future__ import annotations

import logging
import threading
from dataclasses import dataclass, field

import pulsectl

from linux_arctis_manager.app_matcher import AppEQOverride
from linux_arctis_manager.constants import PULSE_CHAT_NODE_NAME, PULSE_MEDIA_NODE_NAME
from linux_arctis_manager.eq_preset import EQMode, EQPreset

logger = logging.getLogger('EQManager')

LADSPA_PLUGIN = 'mbeq_1197'
LADSPA_LABEL = 'mbeq'

EQ_SINK_SUFFIX = '_EQ'


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
        self._loaded_modules: dict[str, int] = {}   # channel -> module index
        self._config: EQConfig = EQConfig()
        self._monitor_thread: threading.Thread | None = None
        self._stopping = False

    def _get_pulse(self) -> pulsectl.Pulse:
        if self._pulse is None:
            self._pulse = pulsectl.Pulse('arctis-eq-manager')
        return self._pulse

    def setup(self, physical_sink_name: str, config: EQConfig) -> dict[str, str]:
        """
        Create LADSPA EQ sinks for enabled channels.

        Returns {channel: sink_name} where sink_name is the target for the
        virtual null-sink loopback.  If EQ is disabled for a channel the
        physical_sink_name is returned unchanged.
        """
        self._config = config
        targets: dict[str, str] = {
            'media': physical_sink_name,
            'chat': physical_sink_name,
        }
        channel_node = {'media': PULSE_MEDIA_NODE_NAME, 'chat': PULSE_CHAT_NODE_NAME}
        channel_cfg = {'media': config.media, 'chat': config.chat}

        for channel, cfg in channel_cfg.items():
            if not cfg.enabled or cfg.preset is None:
                continue
            eq_sink_name = f'{channel_node[channel]}{EQ_SINK_SUFFIX}'
            if self._create_ladspa_sink(channel, eq_sink_name, physical_sink_name, cfg):
                targets[channel] = eq_sink_name

        return targets

    def _create_ladspa_sink(self, channel: str, eq_sink_name: str,
                             master: str, cfg: ChannelEQConfig) -> bool:
        controls = ','.join(f'{g:.2f}' for g in cfg.preset.to_ladspa_controls())  # type: ignore[union-attr]
        args = (
            f'sink_name={eq_sink_name} '
            f'master={master} '
            f'plugin={LADSPA_PLUGIN} '
            f'label={LADSPA_LABEL} '
            f'control={controls}'
        )
        try:
            pulse = self._get_pulse()
            idx = pulse.module_load('module-ladspa-sink', args)
            self._loaded_modules[channel] = idx
            logger.info(f'EQ sink {eq_sink_name!r} created for {channel} (mode={cfg.mode})')
            return True
        except Exception as e:
            logger.error(f'Failed to create EQ sink for {channel}: {e}')
            return False

    def remove_channel_eq(self, channel: str) -> None:
        if channel not in self._loaded_modules:
            return
        try:
            self._get_pulse().module_unload(self._loaded_modules.pop(channel))
        except Exception as e:
            logger.warning(f'Error removing EQ sink for {channel}: {e}')

    def start_stream_monitor(self) -> None:
        """Start a background thread that routes new streams to per-app EQ sinks."""
        if not self._config.app_overrides:
            return
        self._stopping = False
        self._monitor_thread = threading.Thread(
            target=self._monitor_loop, daemon=True, name='arctis-eq-monitor'
        )
        self._monitor_thread.start()

    def stop_stream_monitor(self) -> None:
        self._stopping = True
        # Wake the event loop so it can exit
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
        channel_node = {'media': PULSE_MEDIA_NODE_NAME, 'chat': PULSE_CHAT_NODE_NAME}
        eq_sink_name = f'{channel_node.get(channel, PULSE_MEDIA_NODE_NAME)}{EQ_SINK_SUFFIX}'
        try:
            sinks = pulse.sink_list()
            target = next((s for s in sinks if s.name == eq_sink_name), None)
            if target:
                pulse.sink_input_move(stream_index, target.index)
                logger.info(f'Routed stream {stream_index} to {eq_sink_name}')
        except Exception as e:
            logger.warning(f'Failed to route stream {stream_index}: {e}')

    def teardown(self) -> None:
        self.stop_stream_monitor()
        for channel in list(self._loaded_modules.keys()):
            self.remove_channel_eq(channel)
        if self._pulse:
            try:
                self._pulse.close()
            except Exception:
                pass
            self._pulse = None
