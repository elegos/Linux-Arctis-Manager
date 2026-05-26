import asyncio
import json
import locale
import logging
from logging import Logger
from threading import Thread
from time import sleep

from dbus_next.aio.message_bus import MessageBus
from dbus_next.constants import MessageType
from dbus_next.message import Message
from PySide6.QtCore import Signal, Slot
from PySide6.QtGui import QAction, QIcon
from PySide6.QtWidgets import QApplication, QMenu, QSystemTrayIcon

from linux_arctis_manager.constants import (DBUS_BUS_NAME,
                                            DBUS_STATUS_INTERFACE_NAME,
                                            DBUS_STATUS_OBJECT_PATH)
from linux_arctis_manager.gui.base_app import QBaseDesktopApp
from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.main_app import QMainApp
from linux_arctis_manager.gui.ui_utils import get_icon_pixmap
from linux_arctis_manager.i18n import I18n


class QSystrayApp(QBaseDesktopApp):
    new_status = Signal(object)

    logger: Logger

    app: QApplication
    tray_icon: QSystemTrayIcon
    menu: QMenu
    dbus_wrapper: DbusWrapper

    last_device_status: dict[str, dict[str, dict[str, str|int]]]

    def __init__(self, app: QApplication, log_level: int):
        super().__init__(app)

        self.logger = logging.getLogger('SystrayApp')
        self.logger.setLevel(log_level)

        self.app = app

        pixmap = get_icon_pixmap()
        self.tray_icon = QSystemTrayIcon(QIcon(pixmap), parent=self.app)
        self.tray_icon.setToolTip('Arctis Manager')

        lang_code, _ = locale.getdefaultlocale()
        lang_code = lang_code.split('_')[0] if lang_code else 'en'

        self.last_device_status = {}
        self.last_device_settings = {}

        self.menu = QMenu()
        self.menu_setup()
        self.do_polling = False

        self.new_status.connect(self.on_new_status)

        self.dbus_wrapper = DbusWrapper()
        self.dbus_wrapper.sig_status.connect(lambda status: self.new_status.emit(status or {}))
        self.dbus_wrapper.sig_settings.connect(self.on_new_settings)

        self.tray_icon.setContextMenu(self.menu)
    
    def on_new_status(self, status: dict[str, dict[str, dict[str, str|int]]]):
        if self.last_device_status == status:
            return

        self.last_device_status = status
        self.menu_setup()

    def on_new_settings(self, settings: dict):
        if self.last_device_settings == settings:
            return

        self.last_device_settings = settings
        self.menu_setup()

    async def start(self):
        self.logger.info('Starting Systray app.')
        self.tray_icon.show()
        self.dbus_wrapper.start()

        self.app.exec()

    def menu_setup(self) -> None:
        self.menu.clear()
        self._menu_actions = {}

        self._menu_actions['open_app'] = QAction(I18n.translate('ui', 'open_app'))
        self._menu_actions['open_app'].triggered.connect(self.open_main_window)
        self.menu.addAction(self._menu_actions['open_app'])

        device_settings = self.last_device_settings.get('device', {})
        settings_config = self.last_device_settings.get('settings_config', {})

        if 'volume_limiter' in settings_config:
            volume_limiter_config = settings_config['volume_limiter']
            on_value = volume_limiter_config.get('values', {}).get('on', 1)
            off_value = volume_limiter_config.get('values', {}).get('off', 0)
            current_value = device_settings.get('volume_limiter', off_value)
            is_on = current_value == on_value

            self._menu_actions['volume_limiter'] = QAction(I18n.translate('settings', 'volume_limiter'))
            self._menu_actions['volume_limiter'].setCheckable(True)
            self._menu_actions['volume_limiter'].setChecked(is_on)

            def _toggle_volume_limiter(checked, on_val=on_value, off_val=off_value):
                new_value = on_val if checked else off_val
                self.last_device_settings.get('device', {})['volume_limiter'] = new_value
                DbusWrapper.change_setting('volume_limiter', new_value)

            self._menu_actions['volume_limiter'].triggered.connect(_toggle_volume_limiter)
            self.menu.addAction(self._menu_actions['volume_limiter'])

        sections = 0
        for _, status_obj in self.last_device_status.items():
            if not status_obj:
                continue

            self.menu.addSeparator()
            sections += 1

            for status, status_o in status_obj.items():
                self._menu_actions['status_' + status] = QAction(
                    f"{I18n.translate('status', status)}: "
                    f"{I18n.translate('status_values', status_o['value'])}"
                    f"{'%' if status_o['type'] == 'percentage' else ''}"
                )
                self.menu.addAction(self._menu_actions['status_' + status])

        if sections:
            self.menu.addSeparator()

        self._menu_actions['exit'] = QAction(I18n.translate('ui', 'exit'))
        self._menu_actions['exit'].triggered.connect(self.sig_stop)
        self.menu.addAction(self._menu_actions['exit'])
    
    def is_stopping(self):
        return hasattr(self, '_stopping') and self._stopping

    def open_main_window(self):
        if not hasattr(self, '_main_app'):
            self._main_app = QMainApp(self.app, self.logger.level)

        self._main_app.start_sync()

    @Slot()
    def sig_stop(self):
        if self.is_stopping():
            return
        
        if hasattr(self, '_main_app'):
            self._main_app.sig_stop()

        self._stopping = True
        self.dbus_wrapper.stop()

        self.logger.debug('Received shutdown signal, shutting down.')
        self.app.quit()
