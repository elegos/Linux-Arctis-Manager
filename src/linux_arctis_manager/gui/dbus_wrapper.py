import asyncio
import json
import logging
from threading import Thread
from time import sleep

from dbus_next.aio.message_bus import MessageBus
from dbus_next.constants import MessageType
from dbus_next.message import Message
from PySide6.QtCore import QObject, Signal, SignalInstance

from linux_arctis_manager.constants import DBUS_BUS_NAME, DBUS_SETTINGS_INTERFACE_NAME, DBUS_SETTINGS_OBJECT_PATH, DBUS_STATUS_INTERFACE_NAME, DBUS_STATUS_OBJECT_PATH

class DbusWrapper(QObject):
    sig_status = Signal(object)
    sig_settings = Signal(object)

    def __init__(self, parent: QObject|None = None):
        super().__init__(parent)

        self.logger = logging.getLogger('DbusWrapper')

    def stop(self):
        self.logger.info("Stopping D-Bus wrapper...")
        self._stopping = True

    def request_status(self, one_time = False, frequency_seconds: int = 1) -> None:
        request_thread = Thread(target=self.request_status_thread, kwargs={'frequency_seconds': 0 if one_time else frequency_seconds})
        request_thread.start()

    def request_settings(self, one_time = False, frequency_seconds: int = 1) -> None:
        request_thread = Thread(target=self.request_settings_thread, kwargs={'frequency_seconds': 0 if one_time else frequency_seconds})
        request_thread.start()
    
    def request_status_thread(self, frequency_seconds: int):
        asyncio.run(self.dbus_request_async(
            self.sig_status,
            frequency_seconds,
            DBUS_BUS_NAME,
            DBUS_STATUS_OBJECT_PATH,
            DBUS_STATUS_INTERFACE_NAME,
            'GetStatus',
        ))
    
    def request_settings_thread(self, frequency_seconds: int):
        asyncio.run(self.dbus_request_async(
            self.sig_settings,
            frequency_seconds,
            DBUS_BUS_NAME,
            DBUS_SETTINGS_OBJECT_PATH,
            DBUS_SETTINGS_INTERFACE_NAME,
            'GetSettings',
        ))
    
    async def dbus_request_async(self, sig: SignalInstance, freq: int,destination: str, path: str, interface: str, member: str):
        while not hasattr(self, '_stopping'):
            dbus_bus = await MessageBus().connect()
            reply = await dbus_bus.call(Message(
                destination=destination,
                path=path,
                interface=interface,
                member=member,
                message_type=MessageType.METHOD_CALL
            ))

            if reply is None:
                self.logger.error('Error getting settings: no reply')

            elif reply.message_type == MessageType.ERROR:
                self.logger.error('Error getting settings: %s', reply.body)

            else:
                sig.emit(json.loads(reply.body[0]) or {})

            if freq == 0:
                return
            
            sleep(freq)

