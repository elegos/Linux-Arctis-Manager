import asyncio
import logging
from typing import Any

from dbus_next.aio.message_bus import MessageBus
from dbus_next.constants import BusType
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

    async def start(self, core_engine: CoreEngine) -> asyncio.Future[Any]:
        self.log.info("Initializing service...")

        self.core_engine = core_engine

        bus = await MessageBus(bus_type=BusType.SYSTEM).connect()
        
        bus_name = 'org.freedesktop.login1'
        object_path = '/org/freedesktop/login1'

        try:
            introspection = await bus.introspect(bus_name, object_path)
            obj = bus.get_proxy_object(bus_name, object_path, introspection)
            manager = obj.get_interface('org.freedesktop.login1.Manager')

            manager.on_prepare_for_sleep(self.on_prepare_for_sleep)

            return asyncio.get_event_loop().create_future()
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
            try:
                self.core_engine.init_device()
            except Exception as e:
                self.log.warning("Device init on wake failed (loop will handle recovery): %s", e)
