import asyncio
import logging

from dbus_next.aio import MessageBus
from dbus_next.service import ServiceInterface, method

from arctis_manager.systray_app import SystrayApp


class ArctisManagerInterface(ServiceInterface):
    systray_app: SystrayApp

    def __init__(self, systray_app: SystrayApp):
        super().__init__('name.giacomofurlan.ArctisManager')

        self.systray_app = systray_app

    @method('ShowSettings')
    def show_settings(self):
        self.systray_app.open_settings_window()

    @method('GetHeadsetBatteryCharge')
    def get_headset_battery_charge(self) -> "x":
        if self.systray_app.last_device_status is None or self.systray_app.last_device_status.headset_battery_charge is None:
            return 0
        return int(self.systray_app.last_device_status.headset_battery_charge * 100)


class DBusManager:
    log: logging.Logger
    systray_app: SystrayApp

    def __init__(self, systray_app: SystrayApp):
        self.log = logging.getLogger('DBusManager')
        self.systray_app = systray_app

    async def start(self):
        bus = await MessageBus().connect()
        interface = ArctisManagerInterface(self.systray_app)
        bus.export('/name/giacomofurlan/ArctisManager', interface)
        await bus.request_name('name.giacomofurlan.ArctisManager')

        while not getattr(self, '_stopping', False):
            await asyncio.sleep(1)

    def stop(self):
        if hasattr(self, '_stopping') and self._stopping:
            return
        self._stopping = True
