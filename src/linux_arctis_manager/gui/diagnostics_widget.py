from PySide6.QtCore import Qt, QUrl, Signal
from PySide6.QtGui import QDesktopServices, QGuiApplication
from PySide6.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QPushButton,
    QScrollArea,
    QVBoxLayout,
    QWidget,
)

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.diagnostics_logic import describe_device, issue_url
from linux_arctis_manager.i18n import I18n
from linux_arctis_manager.utils import project_version

_STATUS_KEYS = {
    'supported': 'diag_status_supported',
    'unsupported': 'diag_status_unsupported',
    'ignored': 'diag_status_ignored',
}


def _t(key: str) -> str:
    return I18n.translate('ui', key)


class QDiagnosticsWidget(QWidget):
    """Lists the SteelSeries devices the system exposes and what the app makes of them."""

    # list[dict] on success, None when the daemon could not be queried.
    sig_report = Signal(object)

    def __init__(self, parent: QWidget):
        super().__init__(parent)
        self.sig_report.connect(self._on_report)

        outer = QVBoxLayout()
        outer.setContentsMargins(0, 0, 0, 0)
        self.setLayout(outer)

        header = QHBoxLayout()
        title = QLabel(f'<h2>{_t("diag_title")}</h2>')
        header.addWidget(title, 1)
        self._refresh_button = QPushButton(_t('diag_refresh'))
        self._refresh_button.clicked.connect(self.refresh)
        header.addWidget(self._refresh_button)
        outer.addLayout(header)

        hint = QLabel(_t('diag_hint'))
        hint.setWordWrap(True)
        outer.addWidget(hint)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QScrollArea.Shape.NoFrame)
        content = QWidget()
        self._list_layout = QVBoxLayout()
        self._list_layout.setAlignment(Qt.AlignmentFlag.AlignTop)
        content.setLayout(self._list_layout)
        scroll.setWidget(content)
        outer.addWidget(scroll, 1)

    def showEvent(self, event):
        super().showEvent(event)
        self.refresh()

    def refresh(self) -> None:
        self._refresh_button.setEnabled(False)
        DbusWrapper.request_diagnostics(self.sig_report)

    def _clear(self) -> None:
        while self._list_layout.count():
            widget = self._list_layout.takeAt(0).widget()
            if widget is not None:
                widget.deleteLater()

    def _on_report(self, devices: list[dict] | None) -> None:
        self._refresh_button.setEnabled(True)
        self._clear()
        if devices is None:
            self._add_message(_t('diag_service_unreachable'))
            return
        if not devices:
            self._add_message(_t('diag_no_devices'))
            return
        for device in devices:
            self._list_layout.addWidget(self._device_card(device))

    def _add_message(self, text: str) -> None:
        label = QLabel(text)
        label.setWordWrap(True)
        self._list_layout.addWidget(label)

    def _device_card(self, device: dict) -> QWidget:
        status = device.get('status', 'ignored')
        card = QWidget()
        layout = QVBoxLayout()
        card.setLayout(layout)

        name = device.get('config_name') or device.get('product') or _t('diag_unknown_device')
        ids = f"{device.get('vendor_id', '????')}:{device.get('product_id', '????')}"
        heading = QLabel(f'<b>{name}</b> &nbsp; <code>{ids}</code>')
        heading.setTextFormat(Qt.TextFormat.RichText)
        layout.addWidget(heading)

        state = QLabel(_t(_STATUS_KEYS.get(status, 'diag_status_ignored')))
        state.setWordWrap(True)
        layout.addWidget(state)

        details = QLabel(describe_device(device))
        details.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
        details.setWordWrap(True)
        layout.addWidget(details)

        buttons = QHBoxLayout()
        buttons.addStretch()
        copy = QPushButton(_t('diag_copy'))
        copy.clicked.connect(lambda _=False, d=device: QGuiApplication.clipboard().setText(describe_device(d)))
        buttons.addWidget(copy)
        if status == 'unsupported':
            report = QPushButton(_t('diag_open_issue'))
            report.clicked.connect(
                lambda _=False, d=device: QDesktopServices.openUrl(
                    QUrl(issue_url(d, f'Linux Arctis Manager {project_version()}'))
                )
            )
            buttons.addWidget(report)
        layout.addLayout(buttons)
        return card
