import asyncio
import logging

from dbus_next.aio.message_bus import MessageBus
from dbus_next.service import ServiceInterface, method

from linux_arctis_manager.constants import DBUS_INTERFACE_PATH, DBUS_MESSAGE_BUS_NAME
from linux_arctis_manager.pactl import PulseAudioManager

class ArctisManagerDbusService(ServiceInterface):
    def __init__(self, device_manager: PulseAudioManager):
        super().__init__(DBUS_MESSAGE_BUS_NAME)
        self.device_manager = device_manager

    @method('Ping')
    def ping(self) -> 's': # type: ignore
        return 'Pong'


class DbusManager:
    _instance: 'DbusManager|None' = None

    @staticmethod
    def getInstance() -> 'DbusManager':
        if DbusManager._instance is None:
            DbusManager._instance = DbusManager()

        return DbusManager._instance

    def __init__(self):
        self.log = logging.getLogger('DbusManager')
    
    def setup_sinks(self):
        pass
    
    async def start(self):
        self.log.info("Initializing D-Bus service...")

        device_manager = PulseAudioManager.get_instance()
        device_manager.sinks_setup()
        
        bus = await MessageBus().connect()
        interface = ArctisManagerDbusService(device_manager)
        bus.export(DBUS_INTERFACE_PATH, interface)
        await bus.request_name(DBUS_MESSAGE_BUS_NAME)

        while not getattr(self, '_stopping', False):
            await asyncio.sleep(1)
        
        device_manager.sinks_teardown()

    def stop(self):
        self.log.info("Stopping D-Bus service...")
        self._stopping = True
