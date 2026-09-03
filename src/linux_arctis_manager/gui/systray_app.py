import locale
import logging
from logging import Logger

from PySide6.QtCore import Signal, Slot
from PySide6.QtGui import QAction, QIcon
from PySide6.QtWidgets import QApplication, QMenu, QSystemTrayIcon

from linux_arctis_manager.gui.base_app import QBaseDesktopApp
from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.main_app import QMainApp
from linux_arctis_manager.gui.status_widget import QStatusWidget
from linux_arctis_manager.gui.ui_utils import get_icon_pixmap
from linux_arctis_manager.i18n import I18n


class QSystrayApp(QBaseDesktopApp):
    _sig_status = Signal(object)
    _sig_settings = Signal(object)

    logger: Logger

    app: QApplication
    tray_icon: QSystemTrayIcon
    menu: QMenu
    dbus_wrapper: DbusWrapper

    last_device_status: dict[str, dict[str, dict[str, str|int]]]
    _settings_config: dict

    def __init__(self, app: QApplication, log_level: int):
        super().__init__(app)

        self.logger = logging.getLogger('SystrayApp')
        self.logger.setLevel(log_level)

        self.app = app

        lang_code, _ = locale.getdefaultlocale()
        lang_code = lang_code.split('_')[0] if lang_code else 'en'

        self.last_device_status = {}
        self._settings_config = {}

        pixmap = get_icon_pixmap()
        self.tray_icon = QSystemTrayIcon(QIcon(pixmap), parent=self.app)
        self.tray_icon.setToolTip('Arctis Manager')

        self.menu = QMenu()
        self.menu_setup()
        self.tray_icon.setContextMenu(self.menu)

        self._sig_status.connect(self.on_new_status)
        self._sig_settings.connect(self.on_new_settings)

        self.dbus_wrapper = DbusWrapper()
        self.dbus_wrapper.sig_status.connect(lambda s: self._sig_status.emit(s or {}))
        self.dbus_wrapper.sig_settings.connect(lambda s: self._sig_settings.emit(s or {}))

    def on_new_status(self, status: dict[str, dict[str, dict[str, str|int]]]):
        if self.last_device_status == status:
            return

        self.last_device_status = status
        self.menu_setup()

    def on_new_settings(self, settings: dict) -> None:
        self._settings_config = settings.get('settings_config', {})
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

        sections = 0
        for status_obj in self.last_device_status.values():
            if not status_obj:
                continue

            self.menu.addSeparator()
            sections += 1

            for status, status_o in status_obj.items():
                display = QStatusWidget.format_value(status, status_o, self._settings_config)
                action = QAction(f"{I18n.translate('status', status)}: {display}")
                action.setEnabled(False)
                self._menu_actions['status_' + status] = action
                self.menu.addAction(action)

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
        self.tray_icon.hide()
        self.dbus_wrapper.stop()

        self.logger.debug('Received shutdown signal, shutting down.')
        self.app.quit()
