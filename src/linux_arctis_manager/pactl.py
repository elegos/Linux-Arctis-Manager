import logging
import pulsectl

from linux_arctis_manager.constants import PULSE_CHAT_NODE_NAME, PULSE_MEDIA_NODE_NAME, STEELSERIES_VENDOR_ID

ONLY_PHYSICAL = 1
ONLY_VIRTUAL = 2
ALL_SINKS = 3


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
    
    def get_arctis_sinks(self, mode: int = ALL_SINKS) -> list[pulsectl.PulseSinkInfo]:
        sinks: list[pulsectl.PulseSinkInfo] = self.pulse.sink_list()
        sinks = sinks if type(sinks) is list else [sinks] # pyright: ignore[reportAssignmentType]

        physical = [s for s in sinks if s.proplist.get('device.vendor.id', '') == STEELSERIES_VENDOR_ID]
        virtual = [s for s in sinks if s.proplist.get('node.name', '') in (PULSE_MEDIA_NODE_NAME, PULSE_CHAT_NODE_NAME)]

        if mode == ONLY_PHYSICAL:
            sinks = physical
        elif mode == ONLY_VIRTUAL:
            sinks = virtual
        else:
            sinks = physical + virtual

        return sinks

    def create_virtual_sink(self, name: str, description: str, sink_output: str) -> None:
        sink = next((s for s in self.get_arctis_sinks(ONLY_VIRTUAL) if s.proplist.get('node.name', '') == name), None)
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
        sink = next((s for s in self.get_arctis_sinks(ONLY_VIRTUAL) if s.proplist.get('node.name', '') == name), None)
        if not sink:
            return
        
        self.logger.info(f'Removing virtual sink "{name}"...')
        modules = self.pulse.module_list()
        for module in modules:
            if module.argument and name in module.argument:
                self.pulse.module_unload(module.index)

    def sinks_setup(self):
        real_sink = self.get_arctis_sinks(ONLY_PHYSICAL)

        if not real_sink:
            self.logger.warning('No SteelSeries Arctis sink found.')
            return
        
        self.create_virtual_sink(PULSE_MEDIA_NODE_NAME, 'Arctis Media', real_sink[0].name)
        self.create_virtual_sink(PULSE_CHAT_NODE_NAME, 'Arctis Chat', real_sink[0].name)

    def sinks_teardown(self):
        self.logger.info('Removing virtual sinks...')

        self.remove_virtual_sink(PULSE_MEDIA_NODE_NAME)
        self.remove_virtual_sink(PULSE_CHAT_NODE_NAME)
