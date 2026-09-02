"""Guided `libonnxruntime` install helper ([E10-S7]).

Shows a distro/GPU-matched mini-tutorial (chosen server-side by
`vc_onnxruntime_detect.rs`) and a "Verify" button that re-probes for the
library after the user has followed it. Never installs anything itself —
every command shown is meant to be read, reviewed, and run by the user in
their own terminal, the same trust model as this assistant asking before
running a shell command.
"""
from __future__ import annotations

import logging

from PySide6.QtCore import Signal
from PySide6.QtGui import QGuiApplication
from PySide6.QtWidgets import (
    QDialog,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QTextEdit,
    QVBoxLayout,
)

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('QOnnxRuntimeInstallDialog')

_T = I18n.translate


class OnnxRuntimeInstallDialog(QDialog):
    sig_instructions = Signal(object)
    sig_detect_result = Signal(object)

    def __init__(self, parent=None) -> None:
        super().__init__(parent)
        self.setWindowTitle(_T('ui', 'onnxrt_dialog_title'))
        self.setMinimumSize(520, 420)

        layout = QVBoxLayout()
        self.setLayout(layout)

        self._incipit = QLabel(_T('ui', 'onnxrt_detecting'))
        self._incipit.setWordWrap(True)
        self._incipit.setStyleSheet('font-weight: bold;')
        layout.addWidget(self._incipit)

        self._instructions = QTextEdit()
        self._instructions.setReadOnly(True)
        layout.addWidget(self._instructions, 1)

        self._verify_status = QLabel('')
        self._verify_status.setWordWrap(True)
        layout.addWidget(self._verify_status)

        btn_row = QHBoxLayout()
        self._copy_btn = QPushButton(_T('ui', 'onnxrt_copy'))
        self._copy_btn.clicked.connect(self._copy)
        btn_row.addWidget(self._copy_btn)
        btn_row.addStretch(1)
        self._verify_btn = QPushButton(_T('ui', 'onnxrt_verify'))
        self._verify_btn.clicked.connect(self._verify)
        btn_row.addWidget(self._verify_btn)
        close_btn = QPushButton(_T('ui', 'onnxrt_close'))
        close_btn.clicked.connect(self.accept)
        btn_row.addWidget(close_btn)
        layout.addLayout(btn_row)

        self.sig_instructions.connect(self._on_instructions)
        self.sig_detect_result.connect(self._on_detect_result)

        DbusWrapper.get_onnxruntime_install_instructions(self.sig_instructions)

    def _on_instructions(self, data: dict) -> None:
        vendor = data.get('vendor', 'unknown')
        self._incipit.setText(_T('ui', 'onnxrt_incipit').format(vendor=vendor))
        self._instructions.setPlainText(data.get('instructions', ''))

    def _copy(self) -> None:
        QGuiApplication.clipboard().setText(self._instructions.toPlainText())

    def _verify(self) -> None:
        self._verify_btn.setEnabled(False)
        self._verify_status.setText(_T('ui', 'onnxrt_verifying'))
        DbusWrapper.detect_onnxruntime(self.sig_detect_result)

    def _on_detect_result(self, data: dict) -> None:
        self._verify_btn.setEnabled(True)
        if data.get('found'):
            path = data.get('path', '')
            text = f"{_T('ui', 'onnxrt_found')} {path}"
            color = 'green'
            if data.get('cudnn_missing'):
                hint = data.get('cudnn_hint', '')
                text += '\n' + _T('ui', 'onnxrt_cudnn_missing').format(hint=hint)
                color = 'orange'
            self._verify_status.setText(text)
            self._verify_status.setStyleSheet(f'color: {color};')
        else:
            self._verify_status.setText(_T('ui', 'onnxrt_not_found'))
            self._verify_status.setStyleSheet('color: red;')
