from typing import Literal

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import QHBoxLayout, QLabel, QWidget

from linux_arctis_manager.gui.qt_widgets.q_toggle import QToggle


class QDualState(QWidget):
    checkStateChanged = Signal(Qt.CheckState)

    def __init__(self, off_text: str, on_text: str, init_state: Literal['left', 'right'], parent: QWidget|None = None):
        super().__init__(parent)

        self.main_layout = QHBoxLayout()
        self.main_layout.setAlignment(Qt.AlignmentFlag.AlignLeft)
        self.off_text = off_text
        self.on_text = on_text

        self.status_label = QLabel(self.on_text if init_state == 'right' else self.off_text)
        self.toggle = QToggle(parent=self, is_checkbox=(not on_text))
        self.toggle.setChecked(init_state == 'right')
        self.toggle.checkStateChanged.connect(self.checkStateChanged)
        self.toggle.checkStateChanged.connect(self._on_state_changed)

        self.main_layout.addWidget(self.toggle)
        self.main_layout.addWidget(self.status_label)

        self.setLayout(self.main_layout)
    
    def _on_state_changed(self, state: Qt.CheckState):
        self.status_label.setText(self.on_text if state == Qt.CheckState.Checked else self.off_text)
