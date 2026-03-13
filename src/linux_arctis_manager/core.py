import asyncio
import logging
from typing import Any, Callable, Coroutine, Literal, cast

import usb
from usb.core import Device

from linux_arctis_manager.config import (DeviceConfiguration,
                                         load_device_configurations,
                                         parsed_status)
from linux_arctis_manager.constants import (PULSE_CHAT_NODE_NAME,
                                            PULSE_MEDIA_NODE_NAME)
from linux_arctis_manager.pactl import PulseAudioManager
from linux_arctis_manager.settings import DeviceSettings, GeneralSettings
from linux_arctis_manager.usb_devices_monitor import USBDevicesMonitor
from linux_arctis_manager.utils import ObservableDict


class TypedDevice(Device):
    idVendor: int
    idProduct: int


class CoreEngine:
    logger: logging.Logger
    device_configurations: list[DeviceConfiguration]
    pa_audio_manager: PulseAudioManager
    usb_devices_monitor: USBDevicesMonitor

    device_config: DeviceConfiguration | None = None
    usb_device: TypedDevice | None = None
    general_settings: GeneralSettings
    device_settings: DeviceSettings

    device_status: ObservableDict[str, int]|None = None

    media_mix: int
    chat_mix: int

    device_status_observers: list[Callable[[dict[str, int]]]]
    
    def __init__(self) -> None:
        self.media_mix = 100
        self.chat_mix = 100
        self.device_status_observers = []

        self.general_settings = GeneralSettings.read_from_file()

        self.logger = logging.getLogger('CoreEngine')
        self.pa_audio_manager = PulseAudioManager.get_instance()
        self.usb_devices_monitor = USBDevicesMonitor.get_instance()

        self.reload_device_configurations()
        self.usb_devices_monitor.register_on_connect(self.on_device_connected)
        self.usb_devices_monitor.register_on_disconnect(self.on_device_disconnected)

    def new_device_status(self) -> ObservableDict:
        device_status = ObservableDict()
        device_status.add_observer(self.on_device_status_changed)

        return device_status
    
    def start(self) -> Coroutine:
        self._stopping = False
        self.usb_devices_monitor.start()

        return self.loop()
    
    def stop(self):
        self.logger.info("Stopping CoreEngine...")
        self._stopping = True
        self.usb_devices_monitor.stop()

    def manage_mix_change(self):
        if not self.device_status or not self.device_config:
            return

        new_media_mix = self.device_status.get('media_mix', None)
        new_chat_mix = self.device_status.get('chat_mix', None)

        if new_media_mix is None or new_chat_mix is None:
            return
        
        new_media_mix = parsed_status({'media_mix': new_media_mix}, self.device_config).get('media_mix', self.media_mix)
        new_chat_mix = parsed_status({'chat_mix': new_chat_mix}, self.device_config).get('chat_mix', self.chat_mix)

        if new_media_mix != self.media_mix or new_chat_mix != self.chat_mix:
            self.media_mix = new_media_mix
            self.chat_mix = new_chat_mix
            self.pa_audio_manager.set_mix(self.media_mix, self.chat_mix)
    
    async def listen_endpoint_loop(self, interface_id: int):
        if self.usb_device is None:
            return

        endpoint, max_packet_size = self.guess_interface_endpoint('in', interface_id)

        if not endpoint:
            self.logger.warning(f'Failed to find listen interface endpoint for device: {self.usb_device.idProduct:04x}:{self.usb_device.idVendor:04x}')
            return
        
        try:
            read_input: list[int] = list(await asyncio.to_thread(self.usb_device.read, endpoint, max_packet_size, 200))
            if self.device_config is None:
                return

            if self.device_config.status is not None:
                self.logger.debug(f'Response: {read_input}')

                for mapping in self.device_config.status.response_mapping:
                    starts_with = f'{mapping.starts_with:02x}'
                    if len(starts_with) % 2 != 0:
                        starts_with = f'0{starts_with}'
                    read_hex_str = ''.join(f'{byte:02x}' for byte in read_input)

                    if read_hex_str.startswith(starts_with):
                        device_status = mapping.get_status_values(read_input)
                        if self.device_status is None:
                            self.device_status = self.new_device_status()
                        self.device_status.update(device_status)
                
                self.manage_mix_change()

            await asyncio.sleep(0.1)
        except usb.core.USBError as e:
            if e.errno not in [16, 110]: # 16 (busy), 110 (timeout)
                self.logger.warning('USB error: %s', e)
        except AttributeError as e:
            # If the device disconnects, self.usb_device might be None and generate the error
            pass
        
    
    async def loop(self):
        listen_coroutines: list[asyncio.Task] = []
        while not self._stopping:
            if not self.usb_device:
                await asyncio.sleep(0.1)
                continue

            if self.device_config is not None:
                listen_coroutines = [asyncio.create_task(self.listen_endpoint_loop(interface_id)) for interface_id in self.device_config.listen_interface_indexes]
            
            self.request_device_status()
        
            await asyncio.gather(*listen_coroutines)

    def on_device_connected(self, vendor_id: int, product_id: int) -> None:
        for device_config in self.device_configurations:
            if device_config.vendor_id == vendor_id and product_id in device_config.product_ids:
                self.configure_virtual_sinks()
                break
    
    def on_device_disconnected(self, vendor_id: int, product_id: int) -> None:
        # vendor_id and product_id are not available. Check if the current device is still plugged in.

        if self.usb_device is None or self.device_config is None:
            return

        current_usb_device: Device|None = None
        for product_id in self.device_config.product_ids:
            current_usb_devices = usb.core.find(idVendor=self.device_config.vendor_id, idProduct=product_id)
            if current_usb_devices is None:
                continue
            elif type(current_usb_devices) == Device:
                current_usb_device = current_usb_devices
                break
            else:
                current_usb_device = next((d for d in current_usb_devices if type(d) == Device), None)

            if current_usb_device is not None:
                break

        if current_usb_device is None:
            self.teardown()
    
    def reload_device_configurations(self) -> None:
        self.device_configurations = load_device_configurations()
        self.configure_virtual_sinks()
    
    def configure_virtual_sinks(self) -> None:
        usb_device: Device | Any | None = None
        device_config: DeviceConfiguration | None = None

        for device_config in self.device_configurations:
            for product_id in device_config.product_ids:
                usb_device = usb.core.find(idVendor=device_config.vendor_id,
                                           idProduct=product_id)
                if usb_device is not None:
                    break
            if usb_device is not None:
                break

        if not device_config or not usb_device:
            self.logger.warning("No supported device connected, skipping virtual sink setup")
            return
        
        if self.device_config is not None and self.device_config != device_config:
            # Reset the previous device first
            self.teardown()
        
        self.usb_device = cast(TypedDevice, usb_device)
        self.device_config = device_config
        self.device_status = self.new_device_status()
        self.device_settings = DeviceSettings(self.usb_device.idVendor, self.usb_device.idProduct)

        # Load defaults
        for _, section in self.device_config.settings.items():
            for setting in section:
                setattr(self.device_settings, setting.name, setting.default_value)
        # Load user settings
        self.device_settings.read_from_file()

        # Setup settings observer
        self.device_settings.settings.add_observer(self.on_setting_changed)


        if self.usb_device is not None:
            self.logger.info(f"Found device {self.usb_device.idProduct:04x}:{self.usb_device.idVendor:04x} ({self.device_config.name})")
            try:
                self.usb_device.reset()
            except usb.core.USBError as e:
                self.logger.warning(f"Error resetting USB device (non-fatal): {e}")
            self.kernel_detach(self.usb_device, self.device_config)

        # Configure the device
        self.init_device()

        self.pa_audio_manager.wait_for_physical_device(self.usb_device.idVendor, self.usb_device.idProduct)
        self.pa_audio_manager.sinks_setup(self.device_config.name, self.device_config.vendor_id, self.device_config.product_ids)

        self.redirect_to_media_sink()
    
    def init_device(self):
        self.logger.info("Initializing device...")
        if self.device_config and self.device_config.device_init:
            endpoint = self.get_command_endpoint_address()

            for bytes in self.device_config.device_init:
                self.send_command(self.translate_init_bytes(bytes), endpoint)
    
    def is_device_online(self) -> bool:
        if self.device_status is None or self.device_config is None:
            return False
        
        if (online_status_config := self.device_config.online_status) is None:
            return True
        
        parsed = parsed_status(self.device_status, self.device_config)

        return online_status_config is None or parsed.get(online_status_config.status_variable) == online_status_config.online_value
    
    def register_observer(self, observer: Callable[[dict[str, int]], None]):
        if observer not in self.device_status_observers:
            self.device_status_observers.append(observer)

    def on_device_status_changed(self, key: str, value: int):
        if self.device_config and self.device_config.online_status and key == self.device_config.online_status.status_variable:
            if self.is_device_online():
                self.redirect_to_media_sink()
            else:
                self.redirect_audio_on_disconnect()

        if self.device_status:
            for observer in self.device_status_observers:
                observer(self.device_status.to_dict())
    
    def redirect_to_media_sink(self):
        if not self.general_settings.redirect_audio_on_connect or not self.is_device_online():
            return

        self.pa_audio_manager.redirect_audio(PULSE_MEDIA_NODE_NAME)

    def redirect_audio_on_disconnect(self):
        redirect_device = self.general_settings.redirect_audio_on_disconnect_device if self.general_settings.redirect_audio_on_disconnect else None
        current_default_device = self.pa_audio_manager.get_default_device()

        if current_default_device and redirect_device and current_default_device.name in [PULSE_MEDIA_NODE_NAME, PULSE_CHAT_NODE_NAME]:
            self.pa_audio_manager.redirect_audio(redirect_device)
    
    def translate_init_bytes(self, data: list[int|str]) -> list[int]:
        result: list[int] = []

        for byte in data:
            if type(byte) == int:
                result.append(byte)
            elif type(byte) == str:
                uri = byte.split('.')
                if uri[0] == 'settings':
                    result.append(self.device_settings.get(uri[1]))
                elif byte == 'status.request':
                    if self.device_config is None:
                        raise Exception(f'Device configuration is not available, skipping {byte}')
                    if self.device_config.status is None:
                        self.logger.warning(f'Device status configuration is not available, skipping {byte}')
                    else:
                        result.append(self.device_config.status.request)

        return result
    
    def get_command_endpoint_address(self):
        if self.device_config is None:
            raise Exception('Device configuration is not available')
        if self.usb_device is None:
            raise Exception('USB device is not available')

        endpoint, _ = self.guess_interface_endpoint('out', self.device_config.command_interface_index[0], self.device_config.command_interface_index[1])
        if endpoint is None:
            raise Exception(f"Failed to find command interface endpoint for device: {self.usb_device.idProduct:04x}:{self.usb_device.idVendor:04x}")

        return endpoint
    
    def on_setting_changed(self, setting: str, value: int) -> None:
        if self.device_config is None:
            self.logger.warning('Attempted to change setting without a device configuration')
            return

        config = next((
            config
            for section in self.device_config.settings.keys()
            for config in self.device_config.settings[section] if config.name == setting
        ), None)

        if not config:
            self.logger.warning(f'Unknown setting: {setting}')
            return

        endpoint = self.get_command_endpoint_address()
        self.send_command(config.get_update_sequence(value), endpoint)

    def send_command(self, command: list[int], endpoint: int) -> None:
        if self.device_config is None:
            raise Exception('Device configuration is not available')
    
        if self.usb_device is None:
            raise Exception('USB device is not available')

        command_str = ''.join(f'{byte:02x}' for byte in command)
        if len(command_str) % 2 != 0:
            command_str = f'0{command_str}'

        filler = f'{self.device_config.command_padding.filler:02x}'
        if len(filler) % 2 != 0:
            filler = f'0{filler}'
        
        if len(command_str) < self.device_config.command_padding.length * 2:
            command_str = f'{command_str}{filler * (self.device_config.command_padding.length - len(command_str) // 2)}'

        command_lst = [int.from_bytes([int(command_str[i:i+2], 16)], 'big') for i in range(0, len(command_str), 2)]

        try:
            self.usb_device.write(endpoint, command_lst)
        except usb.core.USBError as e:
            self.logger.warning(f"Error sending command: {e}")

    def kernel_detach(self, usb_device: TypedDevice, config: DeviceConfiguration) -> None:
        self.logger.info(f"Detaching kernel driver for device: {usb_device.idVendor:04x}:{usb_device.idProduct:04x} ({config.name})")

        interfaces = list(set([config.command_interface_index[0], *config.listen_interface_indexes]))
        for interface in interfaces:
            if usb_device.is_kernel_driver_active(interface):
                self.logger.info(f"Kernel driver active on interface {interface}, detaching...")
                usb_device.detach_kernel_driver(interface)
            try:
                usb.util.claim_interface(usb_device, interface)
                self.logger.info(f"Claimed interface {interface}")
            except usb.core.USBError as e:
                self.logger.warning(f"Error claiming interface {interface}: {e}")
    
    def kernel_attach(self, usb_device: TypedDevice, config: DeviceConfiguration) -> None:
        self.logger.info(f"Re-attaching kernel driver for device: {usb_device.idProduct:04x}:{usb_device.idVendor:04x} ({config.name})")

        interfaces = list(set([config.command_interface_index[0], *config.listen_interface_indexes]))
        for interface in interfaces:
            if not usb_device.is_kernel_driver_active(interface):
                self.logger.info(f"Kernel driver inactive on interface {interface}, re-attaching...")
                usb_device.attach_kernel_driver(interface)
    
    def guess_interface_endpoint(self, direction: Literal['in', 'out'], interface_index: int, interface_alternate_setting: int = 0) -> tuple[int | None, int | None]:
        '''
        Returns the endpoint address and max packet size for the given interface index and alternate setting.
        '''
        if self.usb_device is None:
            return None, None

        directions = {'in': usb.util.ENDPOINT_IN, 'out': usb.util.ENDPOINT_OUT}

        interface: usb.core.Interface|None = next((
            config
            for config in self.usb_device.get_active_configuration()
            if config.bInterfaceNumber == interface_index and config.bAlternateSetting == interface_alternate_setting
        ), None)

        if interface is None:
            raise Exception(f"Failed to find interface for device: {self.usb_device.idProduct:04x}:{self.usb_device.idVendor:04x} (interface: {interface_index}, alternate setting: {interface_alternate_setting})")

        for endpoint in interface.endpoints():
            if usb.util.endpoint_direction(endpoint.bEndpointAddress) == directions[direction]:
                return endpoint.bEndpointAddress, endpoint.wMaxPacketSize

        return None, None

    def request_device_status(self):
        if not self.usb_device or not self.device_config or not self.device_config.status:
            return
        
        endpoint = self.get_command_endpoint_address()
        self.send_command([self.device_config.status.request], endpoint)

    def teardown(self) -> None:
        self.pa_audio_manager.sinks_teardown()
        if self.usb_device:
            try:
                if self.device_config is not None:
                    interfaces = list(set([self.device_config.command_interface_index[0], *self.device_config.listen_interface_indexes]))
                    for interface in interfaces:
                        try:
                            usb.util.release_interface(self.usb_device, interface)
                        except usb.core.USBError:
                            pass
                if self.device_config and usb.core.find(idVendor=self.device_config.vendor_id):
                    self.kernel_attach(self.usb_device, self.device_config)
            except usb.core.USBError as e:
                self.logger.warning(f"Error re-attaching kernel driver: {e}")
            finally:
                usb.util.dispose_resources(self.usb_device)
        self.redirect_audio_on_disconnect()
        
        self.usb_device = None
        self.device_config = None
        self.device_status = None
