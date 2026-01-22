import asyncio
import logging
from typing import Callable

import pyudev


class USBDevicesMonitor:
    _instance: 'USBDevicesMonitor|None' = None

    @staticmethod
    def get_instance() -> 'USBDevicesMonitor':
        if USBDevicesMonitor._instance is None:
            USBDevicesMonitor._instance = USBDevicesMonitor()

        return USBDevicesMonitor._instance

    def __init__(self):
        self.logger = logging.getLogger('USBDevicesMonitor')

        self.context = pyudev.Context()
        self.monitor = pyudev.Monitor.from_netlink(self.context)
        self.monitor.filter_by(subsystem='usb')

        self._stopping = False
        self._on_connect_callbacks: list[Callable[[int, int], None]] = []
        self._on_disconnect_callbacks: list[Callable[[int, int], None]] = []
    
    def register_on_connect(self, callback: Callable[[int, int], None]):
        if callback not in self._on_connect_callbacks:
            self._on_connect_callbacks.append(callback)
    
    def register_on_disconnect(self, callback: Callable[[int, int], None]):
        if callback not in self._on_disconnect_callbacks:
            self._on_disconnect_callbacks.append(callback)

    def start(self):
        self.logger.info("Starting USB devices monitor...")
        observer = pyudev.MonitorObserver(
            self.monitor,
            callback=self._on_event,
            name='usb-monitor'
        )
        observer.start()
    
    async def wait_for_stop(self):
        while not self._stopping:
            await asyncio.sleep(1)
    
    def stop(self):
        self.logger.info("Stopping USB devices monitor...")
        self._stopping = True
    
    def _on_event(self, device: pyudev.Device):
        if device.device_type != 'usb_device':
            return

        vid: int = int(device.get('ID_VENDOR_ID', '0'), 16)
        pid: int = int(device.get('ID_MODEL_ID', '0'), 16)

        if device.action == 'add':
            self._on_connect(vid, pid)

        elif device.action == 'remove':
            self._on_disconnect(vid, pid)

    def _on_connect(self, vendor_id: int, product_id: int):
        for callback in self._on_connect_callbacks:
            callback(vendor_id, product_id)

    def _on_disconnect(self, vendor_id: int, product_id: int):
        for callback in self._on_disconnect_callbacks:
            callback(vendor_id, product_id)
