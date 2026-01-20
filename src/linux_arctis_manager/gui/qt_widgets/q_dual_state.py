from typing import Literal

from PySide6.QtWidgets import QWidget, QHBoxLayout, QLabel
from PySide6.QtCore import Qt, Signal

from linux_arctis_manager.gui.qt_widgets.q_toggle import QToggle

class QDualState(QWidget):
    checkStateChanged = Signal(Qt.CheckState)

    def __init__(self, left_text: str, right_text: str, init_state: Literal['left', 'right'], parent: QWidget|None = None):
        super().__init__(parent)

        self.main_layout = QHBoxLayout()
        self.main_layout.setAlignment(Qt.AlignmentFlag.AlignLeft)
        self.left_text = left_text
        self.right_text = right_text

        left_label = QLabel(self.left_text)
        right_label = QLabel(self.right_text)
        self.toggle = QToggle(parent=self, is_checkbox=(not right_text))
        self.toggle.setChecked(init_state == 'right')
        self.toggle.checkStateChanged.connect(self.checkStateChanged)

        self.main_layout.addWidget(left_label)
        self.main_layout.addWidget(self.toggle)
        self.main_layout.addWidget(right_label)

        self.setLayout(self.main_layout)
