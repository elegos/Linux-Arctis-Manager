"""Guided voice calibration wizard for the RVC voice changer.

The user reads a short edge-case text, the daemon renders it through three
candidate tunings, and the user picks by ear — with the original recording
available for reference.  "Refine" iterates with narrower steps around the
chosen candidate; "Save" persists it as the model's tuning.
"""
from __future__ import annotations

import logging
import subprocess

from PySide6.QtCore import QTimer, Signal
from PySide6.QtWidgets import (QButtonGroup, QDialog, QHBoxLayout, QLabel,
                               QMessageBox, QPushButton, QRadioButton,
                               QStackedWidget, QVBoxLayout, QWidget)

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('QVCCalibrationWizard')

_T = I18n.translate

# Each sentence targets a failure mode observed in the field: phrase-final
# vowels sliding into vocal fry, nasal word endings, sibilant endings, plosive
# bursts, rising question intonation, and a deliberate quiet trail-off.
_DEFAULT_TEXT = (
    'Hello, my name is Ginny.\n'
    'Nine long mornings running, I remained alone.\n'
    'Yes, this is as simple as it seems.\n'
    'A perfectly packed paper cup popped at the top.\n'
    'Could you keep it up until the very end?\n'
    'And in the end… it slowly fades away.'
)

_VARIANT_HINTS = {
    'A': 'current tuning',
    'B': 'follows your voice dynamics more closely',
    'C': 'lets the model character through more',
}


class QVCCalibrationWizard(QDialog):
    sig_rec_started    = Signal(object)
    sig_rec_stopped    = Signal(object)
    sig_render_started = Signal(object)
    sig_status         = Signal(object)

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle(_T('ui', 'vc_calib_title'))
        self.setMinimumWidth(560)

        self.chosen_params: dict | None = None   # set on Save
        self._results: list[dict] = []
        self._original_path: str = ''
        self._play_proc: subprocess.Popen | None = None

        self._poll = QTimer(self)
        self._poll.setInterval(500)
        self._poll.timeout.connect(lambda: DbusWrapper.calibration_get_status(self.sig_status))

        self._rec_secs = 0
        self._rec_timer = QTimer(self)
        self._rec_timer.setInterval(1000)
        self._rec_timer.timeout.connect(self._tick_recording)

        self.sig_rec_started.connect(self._on_rec_started)
        self.sig_rec_stopped.connect(self._on_rec_stopped)
        self.sig_render_started.connect(self._on_render_started)
        self.sig_status.connect(self._on_status)

        root = QVBoxLayout(self)
        self._stack = QStackedWidget(self)
        root.addWidget(self._stack)

        self._stack.addWidget(self._page_intro())      # 0
        self._stack.addWidget(self._page_recording())  # 1
        self._stack.addWidget(self._page_rendering())  # 2
        self._stack.addWidget(self._page_listen())     # 3

    # ── Pages ─────────────────────────────────────────────────────────

    def _reading_text_label(self) -> QLabel:
        text = _T('ui', 'vc_calib_text')
        if text == 'vc_calib_text':   # i18n key missing → built-in default
            text = _DEFAULT_TEXT
        lbl = QLabel(text)
        lbl.setWordWrap(True)
        lbl.setStyleSheet('font-size: 13px; padding: 12px; '
                          'border: 1px solid palette(mid); border-radius: 6px;')
        return lbl

    def _page_intro(self) -> QWidget:
        page = QWidget()
        lay = QVBoxLayout(page)
        intro = QLabel(_T('ui', 'vc_calib_intro'))
        intro.setWordWrap(True)
        lay.addWidget(intro)
        lay.addWidget(self._reading_text_label())
        hint = QLabel(_T('ui', 'vc_calib_intro_hint'))
        hint.setWordWrap(True)
        hint.setStyleSheet('color: gray;')
        lay.addWidget(hint)
        lay.addStretch()
        row = QHBoxLayout()
        row.addStretch()
        cancel = QPushButton(_T('ui', 'vc_calib_cancel'))
        cancel.clicked.connect(self.reject)
        row.addWidget(cancel)
        start = QPushButton(_T('ui', 'vc_calib_start_recording'))
        start.setDefault(True)
        start.clicked.connect(lambda: DbusWrapper.calibration_start_recording(self.sig_rec_started))
        row.addWidget(start)
        lay.addLayout(row)
        return page

    def _page_recording(self) -> QWidget:
        page = QWidget()
        lay = QVBoxLayout(page)
        self._rec_label = QLabel(_T('ui', 'vc_calib_recording'))
        self._rec_label.setStyleSheet('font-weight: bold; color: #c33;')
        lay.addWidget(self._rec_label)
        lay.addWidget(self._reading_text_label())
        lay.addStretch()
        row = QHBoxLayout()
        row.addStretch()
        stop = QPushButton(_T('ui', 'vc_calib_stop_recording'))
        stop.setDefault(True)
        stop.clicked.connect(lambda: DbusWrapper.calibration_stop_recording(self.sig_rec_stopped))
        row.addWidget(stop)
        lay.addLayout(row)
        return page

    def _page_rendering(self) -> QWidget:
        page = QWidget()
        lay = QVBoxLayout(page)
        lay.addStretch()
        self._render_label = QLabel(_T('ui', 'vc_calib_rendering'))
        self._render_label.setWordWrap(True)
        lay.addWidget(self._render_label)
        lay.addStretch()
        return page

    def _page_listen(self) -> QWidget:
        page = QWidget()
        lay = QVBoxLayout(page)
        lbl = QLabel(_T('ui', 'vc_calib_listen'))
        lbl.setWordWrap(True)
        lay.addWidget(lbl)

        self._orig_row = self._make_play_row(_T('ui', 'vc_calib_original'), None)
        lay.addLayout(self._orig_row[0])

        self._variant_group = QButtonGroup(self)
        self._variant_rows: list[tuple[QHBoxLayout, QRadioButton, QPushButton, QLabel]] = []
        for i in range(3):
            row, radio, play, name = self._make_variant_row(i)
            lay.addLayout(row)
            self._variant_rows.append((row, radio, play, name))

        lay.addStretch()
        row = QHBoxLayout()
        cancel = QPushButton(_T('ui', 'vc_calib_cancel'))
        cancel.clicked.connect(self.reject)
        row.addWidget(cancel)
        row.addStretch()
        again = QPushButton(_T('ui', 'vc_calib_read_again'))
        again.clicked.connect(lambda: self._goto(0))
        row.addWidget(again)
        refine = QPushButton(_T('ui', 'vc_calib_refine'))
        refine.clicked.connect(self._refine_selected)
        row.addWidget(refine)
        save = QPushButton(_T('ui', 'vc_calib_save'))
        save.setDefault(True)
        save.clicked.connect(self._save_selected)
        row.addWidget(save)
        lay.addLayout(row)
        return page

    def _make_play_row(self, label: str, path: str | None):
        row = QHBoxLayout()
        name = QLabel(label)
        name.setMinimumWidth(260)
        row.addWidget(name)
        play = QPushButton('▶')
        play.setFixedWidth(44)
        play.clicked.connect(lambda: self._play(path or self._original_path))
        row.addWidget(play)
        row.addStretch()
        return row, name, play

    def _make_variant_row(self, idx: int):
        row = QHBoxLayout()
        radio = QRadioButton()
        self._variant_group.addButton(radio, idx)
        row.addWidget(radio)
        name = QLabel('—')
        name.setMinimumWidth(240)
        row.addWidget(name)
        play = QPushButton('▶')
        play.setFixedWidth(44)
        play.clicked.connect(lambda: self._play_variant(idx))
        row.addWidget(play)
        row.addStretch()
        return row, radio, play, name

    # ── Flow ──────────────────────────────────────────────────────────

    def _goto(self, page: int) -> None:
        self._stack.setCurrentIndex(page)

    def _on_rec_started(self, ok: object) -> None:
        if not ok:
            QMessageBox.warning(self, self.windowTitle(), _T('ui', 'vc_calib_err_record'))
            return
        self._rec_secs = 0
        self._rec_timer.start()
        self._goto(1)

    def _tick_recording(self) -> None:
        self._rec_secs += 1
        self._rec_label.setText(f"{_T('ui', 'vc_calib_recording')}  {self._rec_secs}s")

    def _on_rec_stopped(self, path: object) -> None:
        self._rec_timer.stop()
        if not path:
            QMessageBox.warning(self, self.windowTitle(), _T('ui', 'vc_calib_err_record'))
            self._goto(0)
            return
        self._original_path = str(path)
        # Fetch status once to check the recording level before rendering.
        DbusWrapper.calibration_get_status(self.sig_status)

    def _refine_selected(self) -> None:
        sel = self._variant_group.checkedId()
        if sel < 0 or sel >= len(self._results):
            QMessageBox.information(self, self.windowTitle(), _T('ui', 'vc_calib_pick_first'))
            return
        DbusWrapper.calibration_start_render(
            self._results[sel]['params'], self.sig_render_started)

    def _on_render_started(self, ok: object) -> None:
        if not ok:
            QMessageBox.warning(self, self.windowTitle(), _T('ui', 'vc_calib_err_render'))
            return
        self._goto(2)
        self._poll.start()

    def _on_status(self, status: object) -> None:
        if not isinstance(status, dict):
            return
        state = status.get('state', '')
        if state == 'recorded':
            # Post-recording level gate: a wrong/dead input records digital
            # near-silence — warn instead of rendering three mute variants.
            if float(status.get('peak', 0.0)) < 0.005:
                QMessageBox.warning(self, self.windowTitle(),
                                    _T('ui', 'vc_calib_err_silent'))
                self._goto(0)
            else:
                DbusWrapper.calibration_start_render(None, self.sig_render_started)
            return
        if state == 'done':
            self._poll.stop()
            self._results = list(status.get('results', []))
            for i, (_, radio, _, name) in enumerate(self._variant_rows):
                if i < len(self._results):
                    label = self._results[i].get('label', '?')
                    hint = _VARIANT_HINTS.get(label, '')
                    name.setText(f"{_T('ui', 'vc_calib_variant')} {label} — {hint}")
                    radio.setChecked(label == 'A')
            self._goto(3)
        elif state == 'error':
            self._poll.stop()
            QMessageBox.warning(self, self.windowTitle(),
                                f"{_T('ui', 'vc_calib_err_render')}\n{status.get('error', '')}")
            self._goto(0)

    def _save_selected(self) -> None:
        sel = self._variant_group.checkedId()
        if sel < 0 or sel >= len(self._results):
            QMessageBox.information(self, self.windowTitle(), _T('ui', 'vc_calib_pick_first'))
            return
        self.chosen_params = dict(self._results[sel]['params'])
        self.accept()

    # ── Playback (paplay, one at a time) ──────────────────────────────

    def _play(self, path: str) -> None:
        if not path:
            return
        self._stop_playback()
        try:
            self._play_proc = subprocess.Popen(
                ['paplay', path],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception as e:
            logger.warning('playback failed: %s', e)

    def _play_variant(self, idx: int) -> None:
        if idx < len(self._results):
            self._play(self._results[idx].get('path', ''))

    def _stop_playback(self) -> None:
        if self._play_proc is not None and self._play_proc.poll() is None:
            try:
                self._play_proc.terminate()
            except Exception:
                pass
        self._play_proc = None

    # ── Cleanup ───────────────────────────────────────────────────────

    def done(self, result: int) -> None:
        self._poll.stop()
        self._rec_timer.stop()
        self._stop_playback()
        # Best-effort: stop a recording left running (user closed mid-record)
        if self._stack.currentIndex() == 1:
            DbusWrapper.calibration_stop_recording(self.sig_rec_stopped)
        super().done(result)
