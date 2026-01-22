import logging
import time

import pulsectl

from linux_arctis_manager.constants import (PULSE_CHAT_NODE_NAME,
                                            PULSE_MEDIA_NODE_NAME,
                                            STEELSERIES_VENDOR_ID)

ONLY_PHYSICAL = 1
ONLY_VIRTUAL = 2
ALL_SINKS = 3

class TypedPulseSinkInfo(pulsectl.PulseSinkInfo):
    name: str


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
    
    def get_arctis_sinks(self, mode: int = ALL_SINKS) -> list[TypedPulseSinkInfo]:
        sinks: list[TypedPulseSinkInfo] = self.pulse.sink_list()
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
        escaped_node_description = description.replace(' ', '\\ ')
        self.pulse.module_load(
            'module-null-sink',
            f'sink_name={name} '
            f'sink_properties=node.description="{escaped_node_description}"'
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
    
    def wait_for_physical_device(self, vendor_id: int, product_id: int, attempts: int = 10) -> bool:
        vendor_id_hex = f'0x{vendor_id:04x}'
        product_id_hex = f'0x{product_id:04x}'

        while attempts > 0:
            for sink in self.get_arctis_sinks(ONLY_PHYSICAL):
                if sink.proplist.get('device.vendor.id', '') == vendor_id_hex and sink.proplist.get('device.product.id', '') == product_id_hex:
                    return True
            attempts -= 1
            time.sleep(1)
        
        self.logger.error(f'Failed to find SteelSeries Arctis device {vendor_id:04x}:{product_id:04x} after {attempts} attempts')

        return False

    def redirect_audio(self, output_sink_node_name: str) -> None:
        self.logger.info(f'Redirecting audio to {output_sink_node_name}...')

        sink = next((s for s in self.pulse.sink_list() if s.proplist.get('node.name', '') == output_sink_node_name), None)
        if sink is None:
            self.logger.error(f'Failed to find sink {output_sink_node_name} to set it as default')
            return
        self.pulse.default_set(sink)
    
    def get_default_device(self) -> TypedPulseSinkInfo|None:
        server_info = self.pulse.server_info()
        default_sink_name: str|None = getattr(server_info, 'default_sink_name', None)

        if default_sink_name is None:
            return None
        
        sink = next((s for s in self.pulse.sink_list() if s.proplist.get('node.name', '') == default_sink_name), None)

        return sink

    def set_mix(self, media_mix: int, chat_mix: int):
        if media_mix > 100:
            media_mix = 100
        if chat_mix > 100:
            chat_mix = 100

        sinks = self.get_arctis_sinks(ONLY_VIRTUAL)

        media = next((s for s in sinks if s.proplist.get('node.name', '') == PULSE_MEDIA_NODE_NAME), None)
        chat = next((s for s in sinks if s.proplist.get('node.name', '') == PULSE_CHAT_NODE_NAME), None)

        if media:
            self.pulse.volume_set_all_chans(media, media_mix / 100)
        if chat:
            self.pulse.volume_set_all_chans(chat, chat_mix / 100)

    def sinks_setup(self, device_name: str):
        real_sink = self.get_arctis_sinks(ONLY_PHYSICAL)

        if not real_sink:
            self.logger.warning('No SteelSeries Arctis sink found.')
            return
        
        self.create_virtual_sink(PULSE_MEDIA_NODE_NAME, f'{device_name} Media', real_sink[0].name)
        self.create_virtual_sink(PULSE_CHAT_NODE_NAME, f'{device_name} Chat', real_sink[0].name)

    def sinks_teardown(self):
        self.logger.info('Removing virtual sinks...')

        self.remove_virtual_sink(PULSE_MEDIA_NODE_NAME)
        self.remove_virtual_sink(PULSE_CHAT_NODE_NAME)
