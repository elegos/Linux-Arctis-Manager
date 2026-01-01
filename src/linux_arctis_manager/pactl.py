import logging
import pulsectl

from linux_arctis_manager.constants import PULSE_CHAT_NODE_NAME, PULSE_MEDIA_NODE_NAME, STEELSERIES_VENDOR_ID


class PulseAudioManager:
    _instance: 'PulseAudioManager|None' = None

    @staticmethod
    def get_instance() -> 'PulseAudioManager':
        if PulseAudioManager._instance is None:
            PulseAudioManager._instance = PulseAudioManager()

        return PulseAudioManager._instance

    def __init__(self):
        self.pulse = pulsectl.Pulse('linux-arctis-manager')
        self.logger = logging.getLogger('PulseAudioManager')
    
    def get_arctis_sinks(self) -> list[pulsectl.PulseSinkInfo]:
        sinks: list[pulsectl.PulseSinkInfo]|pulsectl.PulseSinkInfo = self.pulse.sink_list() # pyright: ignore[reportUnknownMemberType, reportUnknownVariableType]

        sinks = sinks if type(sinks) is list else [sinks]
        sinks = [s for s in sinks if s.proplist.get('node.name', '') in (PULSE_MEDIA_NODE_NAME, PULSE_CHAT_NODE_NAME) \
                 or s.proplist.get('device.vendor.id', '') == STEELSERIES_VENDOR_ID]

        return sinks if type(sinks) is list else [sinks]

    def create_virtual_sink(self, name: str, description: str, sink_output: str) -> None:
        sink = next((s for s in self.get_arctis_sinks() if s.proplist.get('node.name', '') == name), None)
        if sink:
            return
        
        self.logger.info(f'Creating virtual sink "{name}" -> "{sink_output}"...')
        self.pulse.module_load(
            'module-null-sink',
            f'sink_name={name} '
            f'sink_properties=node.description="{description.replace(' ', '\\ ')}"'
        )

        self.pulse.module_load(
            'module-loopback',
            f'source={name}.monitor '
            f'sink={sink_output} '
            'latency_msec=0'
        )
    
    def remove_virtual_sink(self, name: str) -> None:
        sink = next((s for s in self.get_arctis_sinks() if s.proplist.get('node.name', '') == name), None)
        if not sink:
            return
        
        self.logger.info(f'Removing virtual sink "{name}"...')
        modules = self.pulse.module_list()
        for module in modules:
            if module.argument and f'sink_name={name}' in module.argument:
                self.pulse.module_unload(module.index)
            if module.argument and f'sink={name}' in module.argument:
                self.pulse.module_unload(module.index)

    def sinks_setup(self):
        sinks = self.get_arctis_sinks()

        real_sink = next((s for s in sinks if s.proplist.get('device.vendor.id', '') == STEELSERIES_VENDOR_ID), None)

        if not real_sink:
            self.logger.warning('No SteelSeries Arctis sink found.')
            return
        
        self.create_virtual_sink(PULSE_MEDIA_NODE_NAME, 'Arctis Media', real_sink.name)
        self.create_virtual_sink(PULSE_CHAT_NODE_NAME, 'Arctis Chat', real_sink.name)

    def sinks_teardown(self):
        self.logger.info('Removing virtual sinks...')

        self.remove_virtual_sink(PULSE_MEDIA_NODE_NAME)
        self.remove_virtual_sink(PULSE_CHAT_NODE_NAME)
