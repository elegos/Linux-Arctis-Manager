from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QFormLayout,
    QFrame,
    QLabel,
    QScrollArea,
    QVBoxLayout,
    QWidget,
)

from linux_arctis_manager.i18n import I18n

_T = I18n.translate


class QTrayStatusTab(QWidget):
    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)

        self.setAttribute(Qt.WidgetAttribute.WA_NoSystemBackground)
        self.setStyleSheet('QTrayStatusTab { background: transparent; }')

        outer = QVBoxLayout()
        outer.setContentsMargins(0, 0, 0, 0)
        self.setLayout(outer)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QScrollArea.Shape.NoFrame)
        scroll.setStyleSheet('QScrollArea { background: transparent; border: none; }')
        scroll.viewport().setStyleSheet('background: transparent;')
        self._content = QWidget()
        self._content.setStyleSheet('background: transparent;')
        self._content_layout = QVBoxLayout()
        self._content_layout.setContentsMargins(8, 8, 8, 8)
        self._content_layout.setAlignment(Qt.AlignmentFlag.AlignTop)
        self._content_layout.setSpacing(4)
        self._content.setLayout(self._content_layout)
        scroll.setWidget(self._content)
        outer.addWidget(scroll)

        self._placeholder = QLabel(_T('ui', 'no_device_detected'))
        self._placeholder.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._placeholder.setStyleSheet('color: gray;')
        outer.addWidget(self._placeholder)
        self._placeholder.hide()

        self._last_status: dict = {}

    def update_status(self, status: dict) -> None:
        if status == self._last_status:
            return
        self._last_status = status

        while self._content_layout.count():
            item = self._content_layout.takeAt(0)
            if w := item.widget():
                w.deleteLater()

        if not status:
            self._content.hide()
            self._placeholder.show()
            return

        self._placeholder.hide()
        self._content.show()

        first = True
        for category, fields in status.items():
            if not fields:
                continue

            if not first:
                sep = QFrame()
                sep.setFrameShape(QFrame.Shape.HLine)
                sep.setStyleSheet('color: #555;')
                self._content_layout.addWidget(sep)
            first = False

            cat_lbl = QLabel(_T('status', category))
            cat_lbl.setStyleSheet('font-weight: bold; font-size: 11px;')
            self._content_layout.addWidget(cat_lbl)

            form_container = QWidget()
            form_container.setAutoFillBackground(False)
            form = QFormLayout()
            form.setContentsMargins(8, 0, 0, 4)
            form.setSpacing(2)
            form.setLabelAlignment(Qt.AlignmentFlag.AlignLeft)
            form_container.setLayout(form)

            for field_name, field_obj in fields.items():
                raw = field_obj.get('value', '')
                kind = field_obj.get('type', '')
                if kind == 'percentage':
                    value_text = f"{raw}%"
                else:
                    translated = _T('status_values', str(raw))
                    value_text = translated if translated != str(raw) else str(raw)

                key_lbl = QLabel(_T('status', field_name))
                key_lbl.setStyleSheet('font-size: 11px;')
                val_lbl = QLabel(value_text)
                val_lbl.setAlignment(Qt.AlignmentFlag.AlignRight)
                val_lbl.setStyleSheet('font-size: 11px;')
                form.addRow(key_lbl, val_lbl)

            self._content_layout.addWidget(form_container)
