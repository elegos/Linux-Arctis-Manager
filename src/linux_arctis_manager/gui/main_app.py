import logging
import threading
from typing import Literal

from PySide6.QtCore import QSize, Qt, QTimer, Signal, Slot
from PySide6.QtGui import QIcon
from PySide6.QtWidgets import (QApplication, QButtonGroup, QHBoxLayout, QLabel,
                               QMessageBox, QProgressBar, QSizePolicy, QToolButton,
                               QVBoxLayout, QWidget)

from linux_arctis_manager.constants import SYSTEMD_SERVICE_NAME
from linux_arctis_manager.gui.base_app import QBaseDesktopApp
from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.eq_widget import QEQWidget
from linux_arctis_manager.gui.main_app_proto_widget import QMainAppProtoWidget
from linux_arctis_manager.gui.mic_widget import QMicWidget
from linux_arctis_manager.gui.settings_widget import QSettingsWidget
from linux_arctis_manager.gui.status_widget import QStatusWidget
from linux_arctis_manager.gui.ui_utils import get_icon_pixmap
from linux_arctis_manager.i18n import I18n
from linux_arctis_manager.utils import compare_versions, project_version


class QMainApp(QBaseDesktopApp):
    app: QApplication
    main_window: QMainAppProtoWidget

    side_panel: QWidget
    _nav_buttons: dict[str, QToolButton]
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
        self.mic_widget = QMicWidget(self.main_panel)

        self.main_panel_widgets: dict[str, QWidget] = {
            'status': self.status_widget,
            'general': self.general_settings_widget,
            'device': self.device_settings_widget,
            'eq': self.eq_widget,
            'mic': self.mic_widget,
        }

        for widget in self.main_panel_widgets.values():
            widget.hide()
            self.main_panel_layout.addWidget(widget)

        self.dbus_wrapper.sig_status.connect(self.status_widget.update_status)
        self.dbus_wrapper.sig_settings.connect(self.general_settings_widget.update_settings)
        self.dbus_wrapper.sig_settings.connect(self.device_settings_widget.update_settings)
        self.dbus_wrapper.sig_device_connected.connect(lambda _: self.eq_widget.refresh())

        self.sig_service_version.connect(self._on_service_version)
        self._sig_set_label.connect(self._version_label.setText)
        self._sig_show_error.connect(self._show_error_dialog)
        self.dbus_wrapper.sig_ai_progress.connect(self._on_ai_progress)
        self.dbus_wrapper.sig_ai_complete.connect(self._on_ai_complete)
        self.dbus_wrapper.sig_download_progress.connect(self._on_download_progress)
        self.dbus_wrapper.sig_download_complete.connect(self._on_download_complete)
        self.dbus_wrapper.sig_base_model_progress.connect(self._on_base_model_progress)
        self.dbus_wrapper.sig_base_model_complete.connect(self._on_base_model_complete)

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
        self.side_panel = QWidget()
        side_layout = QVBoxLayout()
        side_layout.setContentsMargins(4, 4, 4, 4)
        side_layout.setSpacing(2)
        self.side_panel.setLayout(side_layout)

        self._nav_buttons: dict[str, QToolButton] = {}
        btn_group = QButtonGroup(self.side_panel)
        btn_group.setExclusive(True)

        # (key, label, icon candidates in priority order)
        nav_items = [
            ('status',  I18n.get_instance().translate('ui', 'status'),  ['dialog-information-symbolic', 'audio-headset', 'computer']),
            ('general', I18n.get_instance().translate('ui', 'general'), ['itmages-settings', 'preferences-system',     'configure']),
            ('device',  I18n.get_instance().translate('ui', 'device'),  ['audio-headset-symbolic',  'input-gaming', 'audio-headset']),
            ('eq',      I18n.get_instance().translate('ui', 'eq'),      ['adjustrgb', 'multimedia-equalizer', 'audio-card']),
            ('mic',     I18n.get_instance().translate('ui', 'mic'),     ['audio-input-microphone-medium-symbolic', 'audio-input-microphone', 'microphone']),
        ]

        def _first_icon(names: list[str]) -> QIcon:
            for name in names:
                icon = QIcon.fromTheme(name)
                if not icon.isNull():
                    return icon
            return QIcon()

        for key, label, icon_names in nav_items:
            btn = QToolButton()
            btn.setText(label)
            btn.setToolButtonStyle(Qt.ToolButtonStyle.ToolButtonTextUnderIcon)
            btn.setIconSize(QSize(36, 36))
            btn.setCheckable(True)
            btn.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
            btn.setStyleSheet("""
                QToolButton {
                    border: none;
                    border-radius: 8px;
                    padding: 10px 4px 6px 4px;
                    font-size: 11px;
                }
                QToolButton:checked {
                    background-color: palette(highlight);
                    color: palette(highlighted-text);
                }
                QToolButton:hover:!checked {
                    background-color: palette(midlight);
                }
            """)
            btn.setIcon(_first_icon(icon_names))
            btn.clicked.connect(lambda checked, k=key: self.switch_panel(k))
            btn_group.addButton(btn)
            side_layout.addWidget(btn)
            self._nav_buttons[key] = btn

        side_layout.addStretch()
        self.side_panel.setFixedWidth(110)
        main_layout.addWidget(self.side_panel)

        # MAIN PANEL
        self.main_panel = QWidget()
        self.main_panel_layout = QVBoxLayout()
        self.main_panel.setLayout(self.main_panel_layout)

        main_layout.addWidget(self.main_panel)

        # FOOTER
        footer = QHBoxLayout()
        footer.setContentsMargins(4, 0, 4, 2)

        self._base_dl_filename = QLabel()
        self._base_dl_filename.setAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter)
        self._base_dl_filename.setVisible(False)
        footer.addWidget(self._base_dl_filename)

        self._base_dl_bar = QProgressBar()
        self._base_dl_bar.setMinimum(0)
        self._base_dl_bar.setMaximum(100)
        self._base_dl_bar.setFixedWidth(160)
        self._base_dl_bar.setVisible(False)
        footer.addWidget(self._base_dl_bar)

        self._ai_status_label = QLabel()
        self._ai_status_label.setAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter)
        _italic = self._ai_status_label.font()
        _italic.setItalic(True)
        self._ai_status_label.setFont(_italic)
        self._ai_status_label.setVisible(False)
        footer.addWidget(self._ai_status_label, 1)

        self._version_label = QLabel(
            I18n.translate('ui', 'version_footer').format(version=project_version())
        )
        self._version_label.setAlignment(Qt.AlignmentFlag.AlignRight)
        footer.addWidget(self._version_label)
        window_layout.addLayout(footer)

        return window
    
    def switch_panel(self, panel: Literal['status', 'general', 'device', 'eq', 'mic']) -> None:
        if not self.main_panel_widgets[panel].isHidden():
            return

        for name, widget in self.main_panel_widgets.items():
            if name == panel:
                widget.show()
            else:
                widget.hide()

        if panel in self._nav_buttons:
            self._nav_buttons[panel].setChecked(True)
    
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

    @Slot(str)
    def _on_ai_progress(self, message: str) -> None:
        self._ai_status_label.setVisible(True)
        self._ai_status_label.setText(f'AI install: {message}')

    @Slot(bool, str)
    def _on_ai_complete(self, success: bool, message: str) -> None:
        self._ai_status_label.setText(f'AI install: {message}')
        QTimer.singleShot(5000, lambda: self._ai_status_label.setVisible(False))
        QTimer.singleShot(1000, self.mic_widget.vc_widget.refresh)

    @Slot(str)
    def _on_download_progress(self, message: str) -> None:
        self._ai_status_label.setVisible(True)
        self._ai_status_label.setText(f'Download: {message}')

    @Slot(bool, str, str)
    def _on_download_complete(self, success: bool, message: str, name: str) -> None:
        self._ai_status_label.setText(f'Download: {message}')
        QTimer.singleShot(5000, lambda: self._ai_status_label.setVisible(False))
        if success and name:
            self.mic_widget.vc_widget._pending_model_select = name
        QTimer.singleShot(500, self.mic_widget.vc_widget.refresh_models)
        QTimer.singleShot(600, self.mic_widget.vc_widget.on_download_done)

    @Slot(str)
    def _on_base_model_progress(self, message: str) -> None:
        # Parse "filename: 42%" to update the progress bar, or show plain text.
        import re
        self._base_dl_filename.setVisible(True)
        self._base_dl_bar.setVisible(True)
        m = re.match(r'^(.+?):\s*(\d+)%$', message)
        if m:
            self._base_dl_filename.setText(m.group(1))
            self._base_dl_bar.setValue(int(m.group(2)))
        else:
            self._base_dl_filename.setText(message)
            self._base_dl_bar.setValue(0)
        self.mic_widget.vc_widget.on_base_model_progress(message)

    @Slot(bool, str)
    def _on_base_model_complete(self, success: bool, message: str) -> None:
        self._base_dl_bar.setValue(100 if success else 0)
        QTimer.singleShot(3000, lambda: (
            self._base_dl_filename.setVisible(False),
            self._base_dl_bar.setVisible(False),
        ))
        self.mic_widget.vc_widget.on_base_model_complete(success, message)

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
