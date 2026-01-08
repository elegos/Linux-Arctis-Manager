import logging
from typing import Any, Literal
import usb
from usb.core import Device

from linux_arctis_manager.config import DeviceConfiguration, load_device_configurations
from linux_arctis_manager.device_settings import DeviceSettings
from linux_arctis_manager.pactl import ONLY_PHYSICAL, PulseAudioManager
from linux_arctis_manager.usb_devices_monitor import USBDevicesMonitor


class CoreEngine:
    logger: logging.Logger
    device_configurations: list[DeviceConfiguration]
    pa_audio_manager: PulseAudioManager
    usb_devices_monitor: USBDevicesMonitor

    device_config: DeviceConfiguration | None = None
    usb_device: Device | None = None
    
    def __init__(self) -> None:
        self.logger = logging.getLogger('CoreEngine')
        self.pa_audio_manager = PulseAudioManager.get_instance()
        self.usb_devices_monitor = USBDevicesMonitor.get_instance()

        self.reload_device_configurations()
        self.usb_devices_monitor.register_on_connect(self.on_device_connected)
        self.usb_devices_monitor.register_on_disconnect(self.on_device_disconnected)
    
    def start(self):
        self.usb_devices_monitor.start()
    
    def stop(self):
        self.logger.info("Stopping CoreEngine...")
        self.usb_devices_monitor.stop()

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
            if current_usb_devices:
                current_usb_device = current_usb_devices if type(current_usb_devices) != list else current_usb_devices

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
        
        self.usb_device = usb_device
        self.device_config = device_config
        self.settings = DeviceSettings(self.usb_device.idVendor, self.usb_device.idProduct)

        # Load defaults
        for _, section in self.device_config.settings.items():
            for setting in section:
                setattr(self.settings, setting.name, setting.default_value)
        # Load user settings
        self.settings.read_from_file()

        if self.usb_device is not None:
            self.logger.info(f"Found device {self.usb_device.idProduct:04x}:{self.usb_device.idVendor:04x} ({self.device_config.name})")
            self.kernel_detach(self.usb_device, self.device_config)

        # TODO init the device

        self.pa_audio_manager.wait_for_physical_device(self.usb_device.idVendor, self.usb_device.idProduct)
        self.pa_audio_manager.sinks_setup(self.device_config.name)

    def kernel_detach(self, usb_device: Device, config: DeviceConfiguration) -> None:
        self.logger.info(f"Detaching kernel driver for device: {usb_device.idProduct:04x}:{usb_device.idVendor:04x} ({config.name})")

        interfaces = list(set([config.command_interface_index, *config.listen_interface_indexes]))
        for interface in interfaces:
            if usb_device.is_kernel_driver_active(interface):
                self.logger.info(f"Kernel driver active on interface {interface}, detaching...")
                usb_device.detach_kernel_driver(interface)
    
    def kernel_attach(self, usb_device: Device, config: DeviceConfiguration) -> None:
        self.logger.info(f"Re-attaching kernel driver for device: {usb_device.idProduct:04x}:{usb_device.idVendor:04x} ({config.name})")

        interfaces = list(set([config.command_interface_index, *config.listen_interface_indexes]))
        for interface in interfaces:
            if not usb_device.is_kernel_driver_active(interface):
                self.logger.info(f"Kernel driver inactive on interface {interface}, re-attaching...")
                usb_device.attach_kernel_driver(interface)
    
    def guess_interface_endpoint(self, interface_index: int, direction: Literal['in', 'out']) -> int | None:
        if self.usb_device is None:
            return None

        directions = {'in': usb.util.ENDPOINT_IN, 'out': usb.util.ENDPOINT_OUT}

        interface = self.usb_device[0][interface_index]
        for endpoint_index, endpoint in enumerate(interface.endpoints()):
            if usb.util.endpoint_direction(endpoint.bEndpointAddress) == directions[direction]:
                return endpoint_index

        return None

    def teardown(self) -> None:
        self.pa_audio_manager.sinks_teardown()
        if self.usb_device and self.device_config and usb.core.find(idVendor=self.device_config.vendor_id):
            try:
                self.kernel_attach(self.usb_device, self.device_config)
            except usb.core.USBError as e:
                self.logger.warning(f"Error re-attaching kernel driver: {e}")
        
        self.usb_device = None
        self.device_config = None
