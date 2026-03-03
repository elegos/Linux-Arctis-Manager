import logging

from dbus_next.aio.message_bus import MessageBus
from dbus_next.errors import (DBusError, InvalidBusNameError,
                              InvalidObjectPathError)

from linux_arctis_manager.core import CoreEngine


class DbusAwake:
    _instance = None

    @staticmethod
    def get_instance() -> 'DbusAwake':
        if DbusAwake._instance is None:
            DbusAwake._instance = DbusAwake()

        return DbusAwake._instance

    def __init__(self):
        self.log = logging.getLogger('DbusAwake')

    async def start(self, core_engine: CoreEngine) -> None:
        self.log.info("Initializing service...")

        self.core_engine = core_engine

        bus = await MessageBus().connect()
        
        bus_name = 'org.freedesktop.login1'
        object_path = '/org/freedesktop/login1'

        try:
            introspection = await bus.introspect(bus_name, object_path)
            obj = bus.get_proxy_object(bus_name, object_path, introspection)
            manager = obj.get_interface('org.freedesktop.login1.Manager')

            manager.on_prepare_for_sleep(self.on_prepare_for_sleep)
        except InvalidObjectPathError:
            self.log.error('Failed to introspect org.freedesktop.login1 : /org/freedesktop/login1. Object path is invalid.')
            return
        except InvalidBusNameError:
            self.log.error('Failed to connect to org.freedesktop.login1 : /org/freedesktop/login1. Bus name is invalid.')
            return
        except DBusError as e:
            self.log.error(f'Failed to connect to org.freedesktop.login1 : /org/freedesktop/login1. DBus error: {e}')
            return
        except Exception as e:
            self.log.error(f'Failed to connect to org.freedesktop.login1 : /org/freedesktop/login1. Unexpected error: {e}')
            return

    def on_prepare_for_sleep(self, going_to_sleep: bool) -> None:
        if not going_to_sleep:
            self.core_engine.init_device()
