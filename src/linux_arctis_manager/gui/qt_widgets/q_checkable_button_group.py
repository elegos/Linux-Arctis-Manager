from pathlib import Path
from typing import Callable, Union

from PySide6.QtCore import Signal
from PySide6.QtWidgets import QButtonGroup, QHBoxLayout, QPushButton, QWidget

from linux_arctis_manager.i18n import I18n


class QCheckableButtonGroup(QWidget):
    new_value = Signal(int)

    def __init__(self, parent: QWidget|None = None):
        super().__init__(parent)

        self.widget_layout = QHBoxLayout()
        self.widget_layout.setSpacing(0)
        self.widget_layout.setContentsMargins(0, 0, 0, 0)
        self.setLayout(self.widget_layout)

        self.setStyleSheet(Path(__file__).parent.joinpath('q_checkable_button_group.css').read_text('utf-8'))

        self.group = QButtonGroup(exclusive=True)
        self.buttons: list[QPushButton] = []
        self.mapping: dict[str, int]

    def _on_button_clicked_builder(self, value: int) -> Callable[[], None]:
        def callback() -> None:
            self.new_value.emit(value)

        return callback
    
    def addButton(self, value: int, label: str, selected: bool, i18n_section: str = 'settings_values') -> None:
        label_str = I18n.get_instance().translate(i18n_section, label)

        btn = QPushButton(label_str)
        btn.setCheckable(True)
        btn.setProperty('value', value)
        if selected:
            btn.setChecked(True)

        btn.clicked.connect(self._on_button_clicked_builder(value))

        self.group.addButton(btn)
        self.widget_layout.addWidget(btn)
        self.buttons.append(btn)
    
    def removeButton(self, value: int) -> None:
        btn = next((btn for btn in self.buttons if btn.property('value') == value), None)
        if not btn:
            return

        self.group.removeButton(btn)
        self.widget_layout.removeWidget(btn)
        self.buttons.remove(btn)
