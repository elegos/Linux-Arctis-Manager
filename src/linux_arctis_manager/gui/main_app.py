import logging
from typing import Literal

from PySide6.QtCore import Qt, Slot
from PySide6.QtGui import QIcon
from PySide6.QtWidgets import QApplication, QHBoxLayout, QLabel, QListWidget, QListWidgetItem, QVBoxLayout, QWidget

from linux_arctis_manager.gui.base_app import QBaseDesktopApp
from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.status_widget import QStatusWidget
from linux_arctis_manager.gui.ui_utils import get_icon_pixmap
from linux_arctis_manager.i18n import I18n

class QMainApp(QBaseDesktopApp):
    app: QApplication
    main_window: QWidget

    side_panel: QListWidget
    main_panel: QWidget
    status_widget: QStatusWidget
 
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
        self.dbus_wrapper.sig_status.connect(self.status_widget.update_status)
        self.switch_panel('status')

        # Pollers
        self.dbus_wrapper.request_settings()
        self.dbus_wrapper.request_status()

        self.destroyed.connect(self.sig_stop)
    
    def main_window_setup(self) -> QWidget:
        window = QWidget()

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
        window.resize(min(800, available_geometry.width()), min(600, available_geometry.height()))

        # SIDE PANEL
        self.side_panel = QListWidget()
        self.side_panel_items = [
            ('status', I18n.get_instance().translate('ui', 'status')),
            ('general', I18n.get_instance().translate('ui', 'general')),
            ('device', I18n.get_instance().translate('ui', 'device')),
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

        return window
    
    def switch_panel(self, panel: Literal['status', 'general', 'device']) -> None:
        while self.main_panel_layout.count():
            self.main_panel_layout.removeItem(self.main_panel_layout.itemAt(0))

        if panel == 'status':
            self.main_panel_layout.addWidget(self.status_widget)
        elif panel == 'general':
            raise NotImplementedError
        elif panel == 'device':
            raise NotImplementedError
    
    async def start(self):
        self.logger.info('Starting Main Window app.')
        self.main_window.show()

        self.app.exec()
    
    def on_settings_received(self, settings):
        if settings == self.settings:
            return
        
        print(settings)
        self.settings = settings

    def on_status_received(self, status):
        if status == self.status:
            return
        
        print(status)
        self.status = status

    @Slot()
    def sig_stop(self):
        if hasattr(self, '_stopping') and self._stopping:
            return
        self._stopping = True

        self.dbus_wrapper.stop()

        self.logger.debug('Received shutdown signal, shutting down.')
        self.app.quit()
