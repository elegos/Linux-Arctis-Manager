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


def test_listen_time_defaults_to_sixty_seconds():
    assert _widget()._listen_spin.value() == 60


def test_progress_bar_is_hidden_until_a_bundle_runs():
    w = _widget()
    assert w._progress.isHidden()


def test_cancelling_the_save_dialog_discards_the_bundle_and_the_scratch(monkeypatch, tmp_path):
    from pathlib import Path

    from PySide6.QtWidgets import QFileDialog
    monkeypatch.setattr(QFileDialog, 'getSaveFileName', lambda *a, **k: ('', ''))
    w = _widget()
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / 'bundle.tar.gz').write_bytes(b'x')
    w._scratch = Path(scratch)
    w._on_bundle_finished(0)
    assert not scratch.exists()
    assert w._bundle_button.isEnabled()


def test_the_save_dialog_only_opens_after_the_run_ends(monkeypatch):
    from PySide6.QtWidgets import QFileDialog
    calls: list[int] = []
    monkeypatch.setattr(QFileDialog, 'getSaveFileName', lambda *a, **k: calls.append(1) or ('', ''))
    w = _widget()
    w._listen_spin.setValue(0)
    w._create_bundle()
    assert calls == [], 'dialog must not open before the script finishes'
    assert w._process is not None
    assert w._process.waitForFinished(120000)
    _app.processEvents()
    assert calls == [1]
    assert not w._progress_timer.isActive()


def test_failed_run_warns_and_cleans_up(monkeypatch, tmp_path):
    from pathlib import Path

    from PySide6.QtWidgets import QMessageBox
    shown: list[str] = []
    monkeypatch.setattr(QMessageBox, 'warning', lambda _p, _t, msg: shown.append(msg))
    w = _widget()
    scratch = tmp_path / 's'
    scratch.mkdir()
    w._scratch = Path(scratch)
    w._on_bundle_finished(1)
    assert shown and not scratch.exists()


def test_a_real_bundle_is_moved_to_the_chosen_file(monkeypatch, tmp_path):
    from PySide6.QtWidgets import QFileDialog, QMessageBox
    out = tmp_path / 'b.tar.gz'
    monkeypatch.setattr(QFileDialog, 'getSaveFileName', lambda *a, **k: (str(out), ''))
    monkeypatch.setattr(QMessageBox, 'information', lambda *a, **k: None)
    monkeypatch.setattr(QMessageBox, 'warning', lambda *a, **k: None)
    w = _widget()
    w._listen_spin.setValue(0)
    w._create_bundle()
    scratch = w._scratch
    assert w._process is not None
    assert w._process.waitForFinished(120000)
    _app.processEvents()
    assert out.is_file() and out.stat().st_size > 0
    assert scratch is not None and not scratch.exists()


def _stub_ask(monkeypatch, answer):
    from PySide6.QtWidgets import QMessageBox
    monkeypatch.setattr(QMessageBox, 'question', lambda *a, **k: answer)


def _stub_open_url(monkeypatch) -> list[str]:
    from PySide6.QtGui import QDesktopServices
    opened: list[str] = []
    monkeypatch.setattr(QDesktopServices, 'openUrl', lambda url: opened.append(url.toString()) or True)
    return opened


def test_report_bug_button_is_always_offered():
    w = _widget()
    assert 'Report bug' in _buttons(w)
    w._on_report([])
    assert 'Report bug' in _buttons(w)


def test_report_bug_button_text_has_no_emoji_glyph():
    # Emoji glyphs may be missing from the button font: the icon comes from the theme instead.
    w = _widget()
    assert w._report_button.text() == 'Report bug'


def test_report_bug_without_a_bundle_opens_the_issue_chooser(monkeypatch):
    from PySide6.QtWidgets import QMessageBox

    from linux_arctis_manager.gui.diagnostics_logic import ISSUE_CHOOSER_URL
    _stub_ask(monkeypatch, QMessageBox.StandardButton.No)
    opened = _stub_open_url(monkeypatch)
    w = _widget()
    w._report_bug()
    assert opened == [ISSUE_CHOOSER_URL]
    assert w._process is None


def test_report_bug_cancelled_does_nothing(monkeypatch):
    from PySide6.QtWidgets import QMessageBox
    _stub_ask(monkeypatch, QMessageBox.StandardButton.Cancel)
    opened = _stub_open_url(monkeypatch)
    w = _widget()
    w._report_bug()
    assert opened == [] and w._process is None


def test_report_bug_with_a_bundle_opens_the_chooser_only_after_the_run(monkeypatch, tmp_path):
    from PySide6.QtWidgets import QFileDialog, QMessageBox

    from linux_arctis_manager.gui.diagnostics_logic import ISSUE_CHOOSER_URL
    _stub_ask(monkeypatch, QMessageBox.StandardButton.Yes)
    opened = _stub_open_url(monkeypatch)
    monkeypatch.setattr(QFileDialog, 'getSaveFileName', lambda *a, **k: (str(tmp_path / 'b.tar.gz'), ''))
    monkeypatch.setattr(QMessageBox, 'information', lambda *a, **k: None)
    monkeypatch.setattr(QMessageBox, 'warning', lambda *a, **k: None)
    w = _widget()
    w._listen_spin.setValue(0)
    w._report_bug()
    assert opened == [], 'the issue page must wait for the bundle'
    assert not w._report_button.isEnabled()
    assert w._process is not None
    assert w._process.waitForFinished(120000)
    _app.processEvents()
    assert opened == [ISSUE_CHOOSER_URL]
    assert w._report_button.isEnabled()


def test_the_issue_chooser_opens_even_when_the_bundle_fails(monkeypatch):
    from PySide6.QtWidgets import QMessageBox

    from linux_arctis_manager.gui.diagnostics_logic import ISSUE_CHOOSER_URL
    monkeypatch.setattr(QMessageBox, 'warning', lambda *a, **k: None)
    opened = _stub_open_url(monkeypatch)
    w = _widget()
    w._open_issues_after_bundle = True
    w._on_bundle_finished(1)
    assert opened == [ISSUE_CHOOSER_URL]
    assert not w._open_issues_after_bundle


def test_every_diagnostics_string_comes_from_the_language_file():
    import re
    from pathlib import Path

    from linux_arctis_manager.i18n import I18n
    source = Path(__file__).parents[3].joinpath(
        'src/linux_arctis_manager/gui/diagnostics_widget.py').read_text(encoding='utf-8')
    used = set(re.findall(r"""_t\(\s*['"]([a-z_]+)['"]""", source)) | set(re.findall(r"""['"](diag_[a-z_]+)['"]""", source))
    ui = I18n.get_instance().translations['ui']
    assert used, 'no translation keys found: the scan is broken'
    assert not [k for k in used if k not in ui], 'missing from the language file'
    assert not [k for k in ui if k.startswith('diag_') and k not in used], 'unused diag_ keys'


def test_the_device_card_uses_translated_labels(monkeypatch):
    from linux_arctis_manager.i18n import I18n
    translations = I18n.get_instance().translations
    monkeypatch.setitem(translations['ui'], 'diag_node', 'NODO')
    monkeypatch.setitem(translations['ui'], 'diag_interface', 'INTERFACCIA')
    w = _widget()
    w._on_report([DEVICE])
    text = _texts(w)
    assert 'NODO hidraw3' in text and 'INTERFACCIA 5' in text
    assert 'status:' not in text


def test_the_listen_time_unit_is_translated(monkeypatch):
    from linux_arctis_manager.i18n import I18n
    monkeypatch.setitem(I18n.get_instance().translations['ui'], 'diag_unit_seconds', 'sec')
    assert _widget()._listen_spin.suffix() == ' sec'
