import asyncio
import json
import logging
from threading import Thread
from time import sleep

from dbus_next.aio.message_bus import MessageBus
from dbus_next.aio.proxy_object import ProxyInterface
from dbus_next.constants import MessageType
from dbus_next.message import Message
from PySide6.QtCore import QObject, Signal, SignalInstance

from linux_arctis_manager.constants import (DBUS_BUS_NAME,
                                            DBUS_SETTINGS_INTERFACE_NAME,
                                            DBUS_SETTINGS_OBJECT_PATH,
                                            DBUS_STATUS_INTERFACE_NAME,
                                            DBUS_STATUS_OBJECT_PATH)


class DbusWrapper(QObject):
    sig_status = Signal(object)
    sig_settings = Signal(object)

    logger = logging.getLogger('DbusWrapper')

    def __init__(self, parent: QObject|None = None):
        super().__init__(parent)

        self._dbus: MessageBus|None = None
        self._stopping = False

        self._status_signal_loop: asyncio.AbstractEventLoop|None = None
        self._stop_status_signal_future: asyncio.Future|None = None

        self._status_iface: ProxyInterface|None = None

    async def status_iface(self):
        if not self._status_iface:
            bus = await MessageBus().connect()
            introspection = await bus.introspect(DBUS_BUS_NAME, DBUS_STATUS_OBJECT_PATH)
            obj = bus.get_proxy_object(DBUS_BUS_NAME, DBUS_STATUS_OBJECT_PATH, introspection)
            self._status_iface = obj.get_interface(DBUS_STATUS_INTERFACE_NAME)

        return self._status_iface

    async def settings_iface(self):
        if not self._status_iface:
            bus = await MessageBus().connect()
            introspection = await bus.introspect(DBUS_BUS_NAME, DBUS_SETTINGS_OBJECT_PATH)
            obj = bus.get_proxy_object(DBUS_BUS_NAME, DBUS_SETTINGS_OBJECT_PATH, introspection)
            self._status_iface = obj.get_interface(DBUS_SETTINGS_INTERFACE_NAME)

        return self._status_iface

    def start(self):
        self.request_status()
        self.request_settings()

        status_signal_thread = Thread(target=lambda: asyncio.run(self._register_status_dbus_signal()))
        status_signal_thread.start()
    
    async def _register_status_dbus_signal(self):
        def callback(status: str) -> None:
            self.sig_status.emit(json.loads(status) or {})

        (await self.status_iface()).on_status_changed(callback) # type: ignore

        self._status_signal_loop = asyncio.get_running_loop()
        self._stop_status_signal_future = self._status_signal_loop.create_future()
        await self._stop_status_signal_future

    def stop(self):
        self.logger.info("Stopping D-Bus wrapper...")
        self._stopping = True
        if self._status_signal_loop and self._stop_status_signal_future:
            self._status_signal_loop.call_soon_threadsafe(self._stop_status_signal_future.set_result, None)

    def request_status(self) -> None:
        request_thread = Thread(target=lambda: asyncio.run(self._request_status_async()))
        request_thread.start()

    async def _request_status_async(self):
        iface = await self.status_iface()
        result = await iface.call_get_status() # type: ignore

        self.sig_status.emit(json.loads(result) or {})

    def request_settings(self) -> None:
        request_thread = Thread(target=lambda: asyncio.run(self._request_settings_async()))
        request_thread.start()
    
    async def _request_settings_async(self):
        iface = await self.settings_iface()
        result = await iface.call_get_settings() # type: ignore

        self.sig_settings.emit(json.loads(result) or {})
    
    @staticmethod
    def request_list_options(list_name: str, qt_signal: SignalInstance):
        request_thread = Thread(target=DbusWrapper.request_list_options_thread, kwargs={'list_name': list_name, 'qt_signal': qt_signal})
        request_thread.start()
    
    @staticmethod
    def request_list_options_thread(list_name: str, qt_signal: SignalInstance):
        asyncio.run(DbusWrapper.request_list_options_async(list_name, qt_signal))
    
    @staticmethod
    async def request_list_options_async(list_name: str, qt_signal: SignalInstance):
        dbus_bus = await MessageBus().connect()
        reply = await dbus_bus.call(Message(
            destination=DBUS_BUS_NAME,
            path=DBUS_SETTINGS_OBJECT_PATH,
            interface=DBUS_SETTINGS_INTERFACE_NAME,
            member='GetListOptions',
            message_type=MessageType.METHOD_CALL,
            signature='s',
            body=[list_name],
        ))

        if reply is None:
            DbusWrapper.logger.error('Error getting settings: no reply')

        elif reply.message_type == MessageType.ERROR:
            DbusWrapper.logger.error('Error getting settings: %s', reply.body)

        else:
            obj = {'name': list_name, 'list': json.loads(reply.body[0]) or []}
            qt_signal.emit(obj)

    @staticmethod
    def change_setting(name: str, value: int|bool|str) -> None:
        request_thread = Thread(target=DbusWrapper.change_setting_thread, kwargs={'name': name, 'value': value})
        request_thread.start()
    
    @staticmethod
    def change_setting_thread(name: str, value: int|bool|str):
        asyncio.run(DbusWrapper.change_setting_async(name, value))
    
    @staticmethod
    async def change_setting_async(name: str, value: int|bool|str):
        dbus_bus = await MessageBus().connect()
        await dbus_bus.call(Message(
            destination=DBUS_BUS_NAME,
            path=DBUS_SETTINGS_OBJECT_PATH,
            interface=DBUS_SETTINGS_INTERFACE_NAME,
            member='SetSetting',
            message_type=MessageType.METHOD_CALL,
            signature='ss',
            body=[name, json.dumps(value)],
        ))
    