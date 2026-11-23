import shutil
import sys
import tempfile
from datetime import datetime
from pathlib import Path

from PySide6.QtCore import (
    QElapsedTimer,
    QProcess,
    QProcessEnvironment,
    Qt,
    QTimer,
    QUrl,
    Signal,
)
from PySide6.QtGui import QDesktopServices, QGuiApplication, QIcon
from PySide6.QtWidgets import (
    QFileDialog,
    QHBoxLayout,
    QLabel,
    QMessageBox,
    QProgressBar,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.debug_bundle import (
    DEFAULT_LISTEN_SECONDS,
    build_command,
    default_file_name,
    estimated_seconds,
    script_path,
)
from linux_arctis_manager.gui.diagnostics_logic import (
    ISSUE_CHOOSER_URL,
    describe_device,
    issue_url,
)
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
        self._report_button = QPushButton(_t('diag_report_bug'))
        self._report_button.setToolTip(_t('diag_report_bug_hint'))
        for icon_name in ('tools-report-bug', 'bug', 'dialog-warning'):
            icon = QIcon.fromTheme(icon_name)
            if not icon.isNull():
                self._report_button.setIcon(icon)
                break
        self._report_button.clicked.connect(self._report_bug)
        header.addWidget(self._report_button)
        outer.addLayout(header)

        hint = QLabel(_t('diag_hint'))
        hint.setWordWrap(True)
        outer.addWidget(hint)

        self._process: QProcess | None = None
        self._bundle_path: Path | None = None
        self._open_issues_after_bundle = False
        self._bundle_button = QPushButton(_t('diag_bundle'))
        self._bundle_button.setToolTip(_t('diag_bundle_hint'))
        self._bundle_button.clicked.connect(self._create_bundle)
        self._listen_spin = QSpinBox()
        self._listen_spin.setRange(0, 600)
        self._listen_spin.setValue(DEFAULT_LISTEN_SECONDS)
        self._listen_spin.setSuffix(f' {_t("diag_unit_seconds")}')
        self._listen_spin.setToolTip(_t('diag_bundle_listen_hint'))
        bundle_row = QHBoxLayout()
        bundle_row.addWidget(self._bundle_button)
        bundle_row.addWidget(QLabel(_t('diag_bundle_listen')))
        bundle_row.addWidget(self._listen_spin)
        bundle_row.addStretch()
        outer.addLayout(bundle_row)
        self._scratch: Path | None = None
        self._progress_total_ms = 1
        self._progress_clock = QElapsedTimer()
        self._progress_timer = QTimer(self)
        self._progress_timer.setInterval(200)
        self._progress_timer.timeout.connect(self._tick_progress)
        self._progress = QProgressBar()
        self._progress.setTextVisible(True)
        self._progress.setVisible(False)
        outer.addWidget(self._progress)

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

        details = QLabel(describe_device(
            device, node_label=_t('diag_node'), interface_label=_t('diag_interface'), include_status=False,
        ))
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

    def _report_bug(self) -> None:
        answer = QMessageBox.question(
            self, _t('diag_report_bug'), _t('diag_report_bug_ask'),
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No | QMessageBox.StandardButton.Cancel,
            QMessageBox.StandardButton.Yes,
        )
        if answer == QMessageBox.StandardButton.Yes:
            self._open_issues_after_bundle = True
            self._create_bundle()
        elif answer == QMessageBox.StandardButton.No:
            QDesktopServices.openUrl(QUrl(ISSUE_CHOOSER_URL))

    def _create_bundle(self) -> None:
        # Collect into a private scratch directory first: the save dialog only
        # opens once the (possibly long) run is over, and nothing is left behind.
        self._scratch = Path(tempfile.mkdtemp(prefix='lam-debug-'))
        listen = self._listen_spin.value()
        program, args = build_command(script_path(), self._scratch / 'bundle.tar.gz', listen)
        self._process = QProcess(self)
        recorder = Path(sys.executable).parent / 'hid-recorder'
        if recorder.is_file():
            env = QProcessEnvironment.systemEnvironment()
            env.insert('LAM_HID_RECORDER', str(recorder))
            self._process.setProcessEnvironment(env)
        self._process.finished.connect(self._on_bundle_finished)
        self._process.errorOccurred.connect(lambda _err: self._on_bundle_finished(-1))
        self._bundle_button.setEnabled(False)
        self._report_button.setEnabled(False)
        self._listen_spin.setEnabled(False)
        self._bundle_button.setText(_t('diag_bundle_running'))
        self._progress_total_ms = estimated_seconds(listen) * 1000
        self._progress_clock.start()
        self._progress.setRange(0, self._progress_total_ms)
        self._progress.setValue(0)
        self._progress.setVisible(True)
        self._progress_timer.start()
        if listen > 0:
            self._progress.setFormat(_t('diag_bundle_reproduce').replace('{seconds}', str(listen)))
        self._process.start(program, args)

    def _tick_progress(self) -> None:
        # Time-based estimate; never reaches 100% before the process really ends.
        self._progress.setValue(min(self._progress_clock.elapsed(), self._progress_total_ms - 1))

    def _on_bundle_finished(self, exit_code: int, *_ignored) -> None:
        open_issues, self._open_issues_after_bundle = self._open_issues_after_bundle, False
        try:
            self._finish_bundle(exit_code)
        finally:
            if open_issues:
                QDesktopServices.openUrl(QUrl(ISSUE_CHOOSER_URL))

    def _finish_bundle(self, exit_code: int) -> None:
        self._progress_timer.stop()
        self._progress.setVisible(False)
        self._bundle_button.setEnabled(True)
        self._report_button.setEnabled(True)
        self._listen_spin.setEnabled(True)
        self._bundle_button.setText(_t('diag_bundle'))
        scratch, self._scratch = self._scratch, None
        if scratch is None:
            return
        try:
            produced = scratch / 'bundle.tar.gz'
            if exit_code != 0 or not produced.is_file():
                QMessageBox.warning(self, _t('diag_bundle'), _t('diag_bundle_failed'))
                return
            path, _ = QFileDialog.getSaveFileName(
                self, _t('diag_bundle_save_title'),
                str(Path.home() / default_file_name(datetime.now())), 'tar.gz (*.tar.gz)',
            )
            if not path:
                return
            try:
                shutil.move(str(produced), path)
            except OSError:
                QMessageBox.warning(self, _t('diag_bundle'), _t('diag_bundle_failed'))
                return
            self._bundle_path = Path(path)
            QMessageBox.information(
                self, _t('diag_bundle'),
                _t('diag_bundle_done').replace('{path}', path),
            )
        finally:
            shutil.rmtree(scratch, ignore_errors=True)
