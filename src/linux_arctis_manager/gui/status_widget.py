from PySide6.QtWidgets import QVBoxLayout, QWidget, QLabel
from PySide6.QtCore import Qt

from linux_arctis_manager.i18n import I18n

class QStatusWidget(QWidget):
    main_layout: QVBoxLayout

    def __init__(self, parent: QWidget):
        super().__init__(parent)

        self.main_layout = QVBoxLayout()
        self.main_layout.setAlignment(Qt.AlignmentFlag.AlignTop)
        self.setLayout(self.main_layout)
    
    def clean_layout(self):
        while self.main_layout.count():
            self.main_layout.removeItem(self.main_layout.itemAt(0))

    def update_status(self, status: dict):
        if hasattr(self, 'status') and status == self.status:
            return

        self.status = status

        if not self.status:
            self.clean_layout()
            label = QLabel(I18n.get_instance().translate('ui', 'no_device_detected'))
            label.font().setBold(True)
            self.main_layout.addWidget(label)

            return
