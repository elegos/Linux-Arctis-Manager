import os
import sys

os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')

from PySide6.QtWidgets import QApplication, QLabel, QPushButton, QWidget

_app = QApplication.instance() or QApplication(sys.argv)

from linux_arctis_manager.gui.diagnostics_widget import QDiagnosticsWidget  # noqa: E402

DEVICE = {
    'node': 'hidraw3', 'vendor_id': '1038', 'product_id': '2202', 'manufacturer': 'SteelSeries',
    'product': 'Arctis Nova X', 'interface': 5, 'usb_audio': True, 'audio_accessory': False,
    'status': 'unsupported', 'config_name': None, 'report_descriptor': '06c0ff',
}


_parents: list[QWidget] = []  # keep the parents alive, or Qt deletes the child with them


def _widget() -> QDiagnosticsWidget:
    parent = QWidget()
    _parents.append(parent)
    return QDiagnosticsWidget(parent)


def _buttons(w: QWidget) -> list[str]:
    return [b.text() for b in w.findChildren(QPushButton)]


def _texts(w: QWidget) -> str:
    return ' '.join(label.text() for label in w.findChildren(QLabel))


def test_unsupported_device_offers_the_issue_button():
    w = _widget()
    w._on_report([DEVICE])
    assert 'Open a request on GitHub' in _buttons(w)
    assert '1038:2202' in _texts(w)


def test_supported_device_has_no_issue_button():
    w = _widget()
    w._on_report([{**DEVICE, 'status': 'supported', 'config_name': 'Arctis Nova X'}])
    assert 'Open a request on GitHub' not in _buttons(w)
    assert 'Copy details' in _buttons(w)


def test_empty_and_unreachable_states_show_a_message():
    w = _widget()
    w._on_report([])
    assert 'No SteelSeries device found' in _texts(w)
    w._on_report(None)
    assert 'not running' in _texts(w)


def test_report_replaces_previous_cards():
    w = _widget()
    w._on_report([DEVICE, {**DEVICE, 'node': 'hidraw4'}])
    w._on_report([DEVICE])
    assert w._list_layout.count() == 1
