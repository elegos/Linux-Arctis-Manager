import logging
import threading
from typing import Literal

from PySide6.QtCore import Qt, Signal, Slot
from PySide6.QtGui import QIcon
from PySide6.QtWidgets import (QApplication, QHBoxLayout, QLabel, QListWidget,
                               QListWidgetItem, QMessageBox, QVBoxLayout,
                               QWidget)

from linux_arctis_manager.constants import SYSTEMD_SERVICE_NAME
from linux_arctis_manager.gui.base_app import QBaseDesktopApp
from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.eq_widget import QEQWidget
from linux_arctis_manager.gui.main_app_proto_widget import QMainAppProtoWidget
from linux_arctis_manager.gui.settings_widget import QSettingsWidget
from linux_arctis_manager.gui.status_widget import QStatusWidget
from linux_arctis_manager.gui.ui_utils import get_icon_pixmap
from linux_arctis_manager.i18n import I18n
from linux_arctis_manager.utils import compare_versions, project_version


class QMainApp(QBaseDesktopApp):
    app: QApplication
    main_window: QMainAppProtoWidget

    side_panel: QListWidget
    main_panel: QWidget
    status_widget: QStatusWidget

    sig_service_version  = Signal(str)
    # Used by the background restart thread to update the UI safely.
    _sig_set_label       = Signal(str)
    _sig_show_error      = Signal(str, str)   # (title, message)

    def __init__(self, app: QApplication, log_level: int):
        super().__init__(parent=app)

        self.logger = logging.getLogger('QMainApp')
        self.logger.setLevel(log_level)

        self.app = app
        self.settings = {}
        self.status = {}

        # Dbus wrapper
        self.dbus_wrapper = DbusWrapper()
        self.dbus_wrapper.sig_settings.connect(self.on_settings_received)
        self.dbus_wrapper.sig_status.connect(self.on_status_received)

        # Qt stuff
        self.main_window = self.main_window_setup()

        self.status_widget = QStatusWidget(self.main_panel)
        self.general_settings_widget = QSettingsWidget(self.main_panel, 'general', 'general')
        self.device_settings_widget = QSettingsWidget(self.main_panel, 'device', 'device')
        self.eq_widget = QEQWidget(self.main_panel)

        self.main_panel_widgets: dict[str, QWidget] = {
            'status': self.status_widget,
            'general': self.general_settings_widget,
            'device': self.device_settings_widget,
            'eq': self.eq_widget,
        }

        for widget in self.main_panel_widgets.values():
            widget.hide()
            self.main_panel_layout.addWidget(widget)

        self.dbus_wrapper.sig_status.connect(self.status_widget.update_status)
        self.dbus_wrapper.sig_settings.connect(self.general_settings_widget.update_settings)
        self.dbus_wrapper.sig_settings.connect(self.device_settings_widget.update_settings)

        self.sig_service_version.connect(self._on_service_version)
        self._sig_set_label.connect(self._version_label.setText)
        self._sig_show_error.connect(self._show_error_dialog)

        self._ui_version = project_version()
        self._service_restart_attempted = False

        self.switch_panel('status')
        self.dbus_wrapper.start()
        DbusWrapper.request_service_version(self.sig_service_version)

        self.destroyed.connect(self.sig_stop)
    
    def main_window_setup(self) -> QMainAppProtoWidget:
        window = QMainAppProtoWidget()

        window.setWindowFlags(Qt.WindowType.Window)
        window.setWindowTitle('Arctis Manager')
        window.setWindowIcon(QIcon(get_icon_pixmap()))

        window_layout = QVBoxLayout()
        window.setLayout(window_layout)

        # TOP LABEL
        top_label = QLabel(I18n.get_instance().translate('ui', 'app_name'))
        top_label.setAlignment(Qt.AlignmentFlag.AlignLeft)
        font = top_label.font()
        font.setBold(True)
        font.setPointSize(20)
        top_label.setFont(font)
        window_layout.addWidget(top_label)

        # MAIN AREA
        main_widget = QWidget()
        main_layout = QHBoxLayout()
        main_widget.setLayout(main_layout)
        window_layout.addWidget(main_widget)

        window.setMinimumSize(800, 600)
        available_geometry = window.screen().availableGeometry()
        window.resize(min(960, available_geometry.width()), min(600, available_geometry.height()))

        # SIDE PANEL
        self.side_panel = QListWidget()
        self.side_panel_items = [
            ('status', I18n.get_instance().translate('ui', 'status')),
            ('general', I18n.get_instance().translate('ui', 'general')),
            ('device', I18n.get_instance().translate('ui', 'device')),
            ('eq', I18n.get_instance().translate('ui', 'eq')),
        ]

        for value, text in self.side_panel_items:
            item = QListWidgetItem(text)
            item.setData(Qt.ItemDataRole.UserRole, value)
            self.side_panel.addItem(item)
        self.side_panel.setFixedWidth(max(self.side_panel.sizeHintForColumn(0), 200))
        self.side_panel.itemClicked.connect(lambda item: self.switch_panel(item.data(Qt.ItemDataRole.UserRole)))
        main_layout.addWidget(self.side_panel)

        # MAIN PANEL
        self.main_panel = QWidget()
        self.main_panel_layout = QVBoxLayout()
        self.main_panel.setLayout(self.main_panel_layout)

        main_layout.addWidget(self.main_panel)

        # FOOTER
        footer = QHBoxLayout()
        footer.setContentsMargins(4, 0, 4, 2)
        footer.addStretch()
        self._version_label = QLabel(
            I18n.translate('ui', 'version_footer').format(version=project_version())
        )
        self._version_label.setAlignment(Qt.AlignmentFlag.AlignRight)
        footer.addWidget(self._version_label)
        window_layout.addLayout(footer)

        return window
    
    def switch_panel(self, panel: Literal['status', 'general', 'device', 'eq']) -> None:
        if not self.main_panel_widgets[panel].isHidden():
            return

        for name, widget in self.main_panel_widgets.items():
            if name == panel:
                widget.show()
            else:
                widget.hide()
    
    @Slot(str)
    def _on_service_version(self, service_version: str) -> None:
        ui = getattr(self, '_ui_version', project_version())
        svc = service_version
        # Empty string means the service doesn't expose GetVersion (old version) — treat as behind.
        cmp = 1 if not svc else compare_versions(ui, svc)

        if cmp == 0:
            self._version_label.setText(
                I18n.translate('ui', 'version_footer').format(version=ui)
            )
        else:
            self._version_label.setText(f'UI: v{ui}  |  Service: v{svc or "?"}')

        if cmp > 0 and not getattr(self, '_service_restart_attempted', False):
            self._service_restart_attempted = True
            self._version_label.setText(
                I18n.translate('ui', 'version_mismatch_service_restarting').format(
                    service=svc or '?', ui=ui)
            )
            threading.Thread(target=self._restart_service_and_recheck, daemon=True).start()

        elif cmp < 0:
            QMessageBox.warning(
                self.main_window,
                I18n.translate('ui', 'version_mismatch_ui_behind_title'),
                I18n.translate('ui', 'version_mismatch_ui_behind_message').format(
                    service=svc, ui=ui),
            )

    def _restart_service_and_recheck(self) -> None:
        """Background thread — must only touch Qt via signals."""
        from linux_arctis_manager.scripts.gui import _wait_for_dbus_service
        from linux_arctis_manager.systemd import ensure_systemd_unit
        try:
            # ensure_systemd_unit rewrites the service file with the current
            # binary path before restarting, so the new version is actually used.
            ensure_systemd_unit(enable=True, restart=True)
        except Exception as e:
            self.logger.warning('Service restart failed: %s', e)
            self._sig_show_error.emit(
                I18n.translate('ui', 'version_mismatch_service_restart_failed_title'),
                I18n.translate('ui', 'version_mismatch_service_restart_failed_message').format(
                    command=f'systemctl --user restart {SYSTEMD_SERVICE_NAME}'),
            )
            return
        if _wait_for_dbus_service():
            DbusWrapper.request_service_version(self.sig_service_version)
        else:
            self.logger.warning('Service did not come back after restart')
            self._sig_show_error.emit(
                I18n.translate('ui', 'version_mismatch_service_restart_failed_title'),
                I18n.translate('ui', 'version_mismatch_service_restart_failed_message').format(
                    command=f'systemctl --user restart {SYSTEMD_SERVICE_NAME}'),
            )

    @Slot(str, str)
    def _show_error_dialog(self, title: str, message: str) -> None:
        QMessageBox.warning(self.main_window, title, message)

    def start_sync(self):
        self.logger.info('Starting Main Window app.')
        self.main_window.show()

        self.app.exec()
    
    async def start(self):
        self.start_sync()
    
    def on_settings_received(self, settings):
        if settings == self.settings:
            return
        
        self.settings = settings

    def on_status_received(self, status):
        if status == self.status:
            return
        
        self.status = status

    @Slot()
    def sig_stop(self):
        if hasattr(self, '_stopping') and self._stopping:
            return
        self._stopping = True

        self.dbus_wrapper.stop()

        self.logger.debug('Received shutdown signal, shutting down.')
        self.app.quit()
