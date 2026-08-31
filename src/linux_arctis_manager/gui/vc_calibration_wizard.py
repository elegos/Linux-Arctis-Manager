"""Guided voice calibration wizard for the RVC voice changer.

Two rounds, back to back: the user reads a short edge-case text once, then

  1. Pitch — a wide sweep of pitch-shift candidates (the model's trained
     register usually differs from the user's own, e.g. a bass-baritone
     voice against a soprano-trained model — see docs/v3-backlog.md's
     [E10-S6b] notes), picked first because it swamps every other tunable.
  2. Dynamics — three candidate tunings (drive/envelope mix) rendered at
     the pitch just picked.

"Refine" (available on both rounds) narrows the search around the current
pick instead of starting over. "Save" persists the combined result as the
model's tuning.
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

_DYNAMICS_HINTS = {
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
        self._original_path: str = ''
        self._play_proc: subprocess.Popen | None = None

        # Round state: 'pitch' while picking the register, 'dynamics' once
        # a pitch has been picked and the timbre round is under way.
        self._round: str = 'pitch'
        self._pitch_results: list[dict] = []
        self._picked_pitch_offset: float = 0.0
        self._dynamics_results: list[dict] = []

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
        self._pitch_page, self._pitch_list = self._page_listen(
            on_again=lambda: self._goto(0),
            again_label=_T('ui', 'vc_calib_read_again'),
            on_refine=self._refine_pitch,
            on_next=self._accept_pitch,
            next_label=_T('ui', 'vc_calib_next'),
        )
        self._stack.addWidget(self._pitch_page)         # 3
        self._dynamics_page, self._dynamics_list = self._page_listen(
            on_again=self._back_to_pitch,
            again_label=_T('ui', 'vc_calib_back'),
            on_refine=self._refine_dynamics,
            on_next=self._save_selected,
            next_label=_T('ui', 'vc_calib_save'),
        )
        self._stack.addWidget(self._dynamics_page)      # 4

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

    def _page_listen(self, *, on_again, again_label: str, on_refine, on_next, next_label: str):
        """One "listen to N variants and pick by ear" page. The variant
        rows are (re)built on demand by `_populate_variants` — the pitch
        round can render up to 6 candidates, the dynamics round always 3,
        and a refine pass may return fewer — so the row count can't be
        fixed at construction time the way it used to be.
        """
        page = QWidget()
        lay = QVBoxLayout(page)
        title = QLabel()
        title.setWordWrap(True)
        lay.addWidget(title)

        orig_row = QHBoxLayout()
        orig_name = QLabel(_T('ui', 'vc_calib_original'))
        orig_name.setMinimumWidth(260)
        orig_row.addWidget(orig_name)
        orig_play = QPushButton('▶')
        orig_play.setFixedWidth(44)
        orig_play.clicked.connect(lambda: self._play(self._original_path))
        orig_row.addWidget(orig_play)
        orig_row.addStretch()
        lay.addLayout(orig_row)

        group = QButtonGroup(page)
        variants_lay = QVBoxLayout()
        lay.addLayout(variants_lay)
        lay.addStretch()

        row = QHBoxLayout()
        cancel = QPushButton(_T('ui', 'vc_calib_cancel'))
        cancel.clicked.connect(self.reject)
        row.addWidget(cancel)
        row.addStretch()
        again = QPushButton(again_label)
        again.clicked.connect(on_again)
        row.addWidget(again)
        refine = QPushButton(_T('ui', 'vc_calib_refine'))
        refine.clicked.connect(on_refine)
        row.addWidget(refine)
        next_btn = QPushButton(next_label)
        next_btn.setDefault(True)
        next_btn.clicked.connect(on_next)
        row.addWidget(next_btn)
        lay.addLayout(row)

        state = {
            'title': title,
            'group': group,
            'variants_lay': variants_lay,
            'rows': [],   # list[(radio, play_btn, name_lbl)]
        }
        return page, state

    def _populate_variants(self, list_state: dict, results: list[dict],
                            title_text: str, hint_fn) -> None:
        list_state['title'].setText(title_text)
        variants_lay = list_state['variants_lay']
        group = list_state['group']
        for radio, _play, _name in list_state['rows']:
            group.removeButton(radio)
        while variants_lay.count():
            item = variants_lay.takeAt(0)
            widget = item.widget()
            if widget is not None:
                widget.deleteLater()

        rows: list[tuple[QRadioButton, QPushButton, QLabel]] = []
        for idx, result in enumerate(results):
            row_widget = QWidget()
            row = QHBoxLayout(row_widget)
            row.setContentsMargins(0, 0, 0, 0)
            radio = QRadioButton()
            group.addButton(radio, idx)
            row.addWidget(radio)
            name = QLabel(hint_fn(result))
            name.setMinimumWidth(240)
            row.addWidget(name)
            play = QPushButton('▶')
            play.setFixedWidth(44)
            play.clicked.connect(lambda _=False, p=result.get('path', ''): self._play(p))
            row.addWidget(play)
            row.addStretch()
            variants_lay.addWidget(row_widget)
            rows.append((radio, play, name))
            if idx == 0:
                radio.setChecked(True)
        list_state['rows'] = rows

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

    def _start_pitch_round(self, anchor: float, refine: bool) -> None:
        self._round = 'pitch'
        self._render_label.setText(_T('ui', 'vc_calib_rendering_pitch'))
        DbusWrapper.calibration_start_render(
            {'round': 'pitch', 'anchor': anchor, 'refine': refine},
            self.sig_render_started)

    def _start_dynamics_round(self, refine_params: dict | None) -> None:
        self._round = 'dynamics'
        self._render_label.setText(_T('ui', 'vc_calib_rendering_dynamics'))
        DbusWrapper.calibration_start_render(
            {'round': 'dynamics', 'pitch_offset': self._picked_pitch_offset,
             'refine_params': refine_params},
            self.sig_render_started)

    def _refine_pitch(self) -> None:
        sel = self._pitch_list['group'].checkedId()
        if sel < 0 or sel >= len(self._pitch_results):
            QMessageBox.information(self, self.windowTitle(), _T('ui', 'vc_calib_pick_first'))
            return
        anchor = float(self._pitch_results[sel].get('pitch_offset', 0.0))
        self._start_pitch_round(anchor, refine=True)

    def _accept_pitch(self) -> None:
        sel = self._pitch_list['group'].checkedId()
        if sel < 0 or sel >= len(self._pitch_results):
            QMessageBox.information(self, self.windowTitle(), _T('ui', 'vc_calib_pick_first'))
            return
        self._picked_pitch_offset = float(self._pitch_results[sel].get('pitch_offset', 0.0))
        self._start_dynamics_round(refine_params=None)

    def _back_to_pitch(self) -> None:
        # No re-render: the pitch round's own results/paths are still valid
        # (each round writes into its own subdirectory — see dbus.rs's
        # CalibrationStartRender doc comment), so just switch pages back.
        self._round = 'pitch'
        self._goto(3)

    def _refine_dynamics(self) -> None:
        sel = self._dynamics_list['group'].checkedId()
        if sel < 0 or sel >= len(self._dynamics_results):
            QMessageBox.information(self, self.windowTitle(), _T('ui', 'vc_calib_pick_first'))
            return
        self._start_dynamics_round(refine_params=self._dynamics_results[sel]['params'])

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
            # near-silence — warn instead of rendering mute variants.
            if float(status.get('peak', 0.0)) < 0.005:
                QMessageBox.warning(self, self.windowTitle(),
                                    _T('ui', 'vc_calib_err_silent'))
                self._goto(0)
            else:
                self._start_pitch_round(anchor=0.0, refine=False)
            return
        if state == 'done':
            self._poll.stop()
            results = list(status.get('results', []))
            if self._round == 'pitch':
                self._pitch_results = results
                self._populate_variants(
                    self._pitch_list, results,
                    _T('ui', 'vc_calib_listen_pitch'),
                    lambda r: f"{float(r.get('pitch_offset', 0.0)):+.1f} st")
                self._goto(3)
            else:
                self._dynamics_results = results
                self._populate_variants(
                    self._dynamics_list, results,
                    _T('ui', 'vc_calib_listen_dynamics'),
                    lambda r: (f"{_T('ui', 'vc_calib_variant')} "
                              f"{r.get('label', '?')} — "
                              f"{_DYNAMICS_HINTS.get(r.get('label', ''), '')}"))
                self._goto(4)
        elif state == 'error':
            self._poll.stop()
            QMessageBox.warning(self, self.windowTitle(),
                                f"{_T('ui', 'vc_calib_err_render')}\n{status.get('error', '')}")
            self._goto(0)

    def _save_selected(self) -> None:
        sel = self._dynamics_list['group'].checkedId()
        if sel < 0 or sel >= len(self._dynamics_results):
            QMessageBox.information(self, self.windowTitle(), _T('ui', 'vc_calib_pick_first'))
            return
        params = dict(self._dynamics_results[sel]['params'])
        params['pitch_offset'] = self._picked_pitch_offset
        self.chosen_params = params
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
