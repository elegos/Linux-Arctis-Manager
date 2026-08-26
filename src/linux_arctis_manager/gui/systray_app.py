import locale
import logging
from logging import Logger

from PySide6.QtCore import Signal, Slot
from PySide6.QtWidgets import QApplication

from linux_arctis_manager.gui.base_app import QBaseDesktopApp
from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.main_app import QMainApp
from linux_arctis_manager.gui.sni_item import SniItem
from linux_arctis_manager.gui.tray_panel import QTrayPanel
from linux_arctis_manager.gui.ui_utils import get_icon_pixmap
from linux_arctis_manager.i18n import I18n


class QSystrayApp(QBaseDesktopApp):
    _sig_status = Signal(object)
    _sig_settings = Signal(object)
    _sig_device_connected = Signal(object)
    _sig_device_disconnected = Signal()
    _sig_nc_settings = Signal(object)

    logger: Logger

    app: QApplication
    _sni: SniItem
    dbus_wrapper: DbusWrapper

    def __init__(self, app: QApplication, log_level: int):
        super().__init__(app)

        self.logger = logging.getLogger('SystrayApp')
        self.logger.setLevel(log_level)

        self.app = app

        lang_code, _ = locale.getdefaultlocale()
        lang_code = lang_code.split('_')[0] if lang_code else 'en'

        pixmap = get_icon_pixmap()

        # ── Rich popup panel (left-click) ──────────────────────────────────────
        self._panel = QTrayPanel()
        self._panel.sig_open_main.connect(self.open_main_window)
        self._panel.sig_exit.connect(self.sig_stop)

        self._sig_status.connect(self._panel.on_status)
        self._sig_settings.connect(self._panel.on_settings)
        self._sig_device_connected.connect(self._panel.on_device_connected)
        self._sig_device_disconnected.connect(self._panel.on_device_disconnected)
        self._sig_nc_settings.connect(self._panel.on_nc_settings)

        # ── Native SNI tray icon + dbusmenu ────────────────────────────────────
        # SniItem registers org.kde.StatusNotifierItem on the session bus,
        # which gives us real Activate(x, y) cursor coordinates on Wayland.
        self._sni = SniItem(
            icon_pixmap=pixmap,
            open_label=I18n.translate('ui', 'open_app'),
            exit_label=I18n.translate('ui', 'exit'),
            parent=self.app,
        )
        self._sni.sig_activate.connect(self._on_activate)
        self._sni.sig_open_app.connect(self.open_main_window)
        self._sni.sig_exit.connect(self.sig_stop)

        # ── D-Bus wrapper ──────────────────────────────────────────────────────
        self.dbus_wrapper = DbusWrapper()
        self.dbus_wrapper.sig_status.connect(lambda s: self._sig_status.emit(s or {}))
        self.dbus_wrapper.sig_settings.connect(lambda s: self._sig_settings.emit(s or {}))
        self.dbus_wrapper.sig_device_connected.connect(self._sig_device_connected)
        self.dbus_wrapper.sig_device_disconnected.connect(self._sig_device_disconnected)

        DbusWrapper.request_current_device(self._sig_device_connected)
        DbusWrapper.request_nc_settings(self._sig_nc_settings)

    def _on_activate(self, x: int, y: int) -> None:
        self._panel.toggle_near(x, y)

    async def start(self):
        self.logger.info('Starting Systray app.')
        self._sni.start()
        self.dbus_wrapper.start()
        self.app.exec()

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
        self._panel.hide()
        self._sni.stop()
        self.dbus_wrapper.stop()

        self.logger.debug('Received shutdown signal, shutting down.')
        self.app.quit()
