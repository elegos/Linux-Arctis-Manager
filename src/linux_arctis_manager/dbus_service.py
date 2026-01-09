import asyncio
import json
import logging

from dbus_next.aio.message_bus import MessageBus
from dbus_next.service import ServiceInterface, method

from linux_arctis_manager.config import parsed_status
from linux_arctis_manager.constants import DBUS_INTERFACE_PATH, DBUS_MESSAGE_BUS_NAME
from linux_arctis_manager.core import CoreEngine

class ArctisManagerDbusService(ServiceInterface):
    def __init__(self, core: CoreEngine):
        super().__init__(DBUS_MESSAGE_BUS_NAME)
        self.core_engine = core

    @method('ReloadConfigs')
    def reload_configs(self) -> 'b': # type: ignore
        self.core_engine.reload_device_configurations()

        return True

    @method('GetStatus')
    def get_status(self) -> 's': # type: ignore
        return json.dumps(parsed_status(self.core_engine.device_status, self.core_engine.device_config)) if self.core_engine.device_status else ''


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
    
    async def start(self, core_engine: CoreEngine):
        self.log.info("Initializing service...")

        self.core_engine = core_engine

        bus = await MessageBus().connect()
        interface = ArctisManagerDbusService(self.core_engine)
        bus.export(DBUS_INTERFACE_PATH, interface)

        await bus.request_name(DBUS_MESSAGE_BUS_NAME)

    async def wait_for_stop(self) -> None:
        while not getattr(self, '_stopping', False):
            await asyncio.sleep(1)
        
        self.core_engine.stop()
        self.core_engine.teardown()

    def stop(self):
        self.log.info("Stopping D-Bus service...")
        self._stopping = True
