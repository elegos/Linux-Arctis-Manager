from PySide6.QtGui import QPalette
from PySide6.QtWidgets import QVBoxLayout, QWidget, QLabel, QApplication
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
            item = self.main_layout.takeAt(0)
            widget = item.widget()
            if widget is not None:
                widget.deleteLater()

    def update_status(self, new_status: dict[str, dict[str, dict[str, str|int]]]):
        if hasattr(self, 'status') and new_status == self.status:
            return

        self.status = new_status

        self.clean_layout()
        if not self.status:
            label = QLabel(I18n.get_instance().translate('ui', 'no_device_detected'))
            label.font().setBold(True)
            self.main_layout.addWidget(label)

            return

        index = 0
        for category, status_obj in self.status.items():
            if index > 0:
                line_separator = QWidget()
                line_separator.setFixedHeight(2)
                self.main_layout.addWidget(line_separator)
            index += 1

            category_label = QLabel(I18n.get_instance().translate('status', category))
            category_font = category_label.font()
            category_font.setBold(True)
            category_font.setPointSize(16)
            category_label.setFont(category_font)
            self.main_layout.addWidget(category_label)

            for status, status_o in status_obj.items():
                label = QLabel(
                    f'{I18n.translate('status', status)}: '
                    f'{I18n.translate('status_values', status_o['value'])}'
                    f'{'%' if status_o['type'] == 'percentage' else ''}'
                )
                self.main_layout.addWidget(label)
