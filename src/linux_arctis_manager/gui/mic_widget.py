from PySide6.QtWidgets import QTabWidget, QVBoxLayout, QWidget

from linux_arctis_manager.gui.nc_widget import QNCWidget
from linux_arctis_manager.gui.vc_widget import QVCWidget
from linux_arctis_manager.i18n import I18n


class QMicWidget(QWidget):
    def __init__(self, parent: QWidget) -> None:
        super().__init__(parent)

        layout = QVBoxLayout()
        layout.setContentsMargins(0, 0, 0, 0)
        self.setLayout(layout)

        self._tabs = QTabWidget(self)
        self.nc_widget = QNCWidget(self._tabs, show_title=False)
        self._tabs.addTab(self.nc_widget, I18n.translate('ui', 'nc'))
        self.vc_widget = QVCWidget(self._tabs, show_title=False)
        self._tabs.addTab(self.vc_widget, I18n.translate('ui', 'vc'))
        layout.addWidget(self._tabs)
