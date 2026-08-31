from __future__ import annotations

import logging
import threading

import pulsectl
import subprocess

from PySide6.QtCore import Qt, QTimer, Signal
from PySide6.QtWidgets import (QCheckBox, QComboBox, QFrame, QGroupBox,
                               QHBoxLayout, QLabel, QLineEdit, QListWidget,
                               QListWidgetItem, QMessageBox, QPushButton,
                               QScrollArea, QSizePolicy, QSlider,
                               QStackedWidget, QVBoxLayout, QWidget)

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.onnxruntime_install_dialog import OnnxRuntimeInstallDialog
from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('QVCWidget')

_T = I18n.translate   # shorthand


def _slider_row(
    label: str, minimum: int, maximum: int, default: int,
    fmt_fn, step: int = 1,
) -> 'tuple[QHBoxLayout, QSlider, QLabel]':
    row = QHBoxLayout()
    lbl = QLabel(label)
    lbl.setFixedWidth(140)
    row.addWidget(lbl)
    sl = QSlider(Qt.Orientation.Horizontal)
    sl.setMinimum(minimum)
    sl.setMaximum(maximum)
    sl.setSingleStep(step)
    sl.setValue(default)
    sl.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
    row.addWidget(sl)
    val = QLabel(fmt_fn(default))
    val.setFixedWidth(70)
    val.setAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
    row.addWidget(val)
    return row, sl, val


def _fslider_row(
    label: str, minimum: float, maximum: float, default: float,
    fmt_fn, steps: int = 100,
) -> 'tuple[QHBoxLayout, QSlider, QLabel]':
    """Float slider backed by an integer slider (minimum=0, maximum=steps)."""
    row = QHBoxLayout()
    lbl = QLabel(label)
    lbl.setFixedWidth(140)
    row.addWidget(lbl)
    sl = QSlider(Qt.Orientation.Horizontal)
    sl.setMinimum(0)
    sl.setMaximum(steps)
    sl.setSingleStep(1)
    # Map default to int
    frac = (default - minimum) / (maximum - minimum) if maximum != minimum else 0
    sl.setValue(round(frac * steps))
    sl.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
    row.addWidget(sl)
    val = QLabel(fmt_fn(default))
    val.setFixedWidth(70)
    val.setAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
    row.addWidget(val)
    return row, sl, val


def _fval(sl: QSlider, minimum: float, maximum: float) -> float:
    return minimum + (sl.value() / sl.maximum()) * (maximum - minimum)


class QVCWidget(QWidget):
    sig_vc_capabilities  = Signal(object)
    sig_vc_settings      = Signal(object)
    sig_rvc_models       = Signal(object)
    sig_hf_results       = Signal(object)
    sig_hf_repo_files    = Signal(object)
    sig_delete_result    = Signal(object)
    sig_hf_token         = Signal(object)
    sig_hf_token_saved   = Signal(object)
    sig_rvc_metrics      = Signal(object)
    _sig_sources_loaded  = Signal(object)
    # Emitted after settings are pushed to the daemon (chain may rebuild).
    sig_applied          = Signal()

    def __init__(self, parent: QWidget, show_title: bool = True) -> None:
        super().__init__(parent)

        self._sources:  list[dict] = []
        self._loading   = False
        self._ladspa_caps: dict[str, bool] = {}
        self._rvc_caps:    dict = {}
        self._pending_model_select: str | None = None
        self._hf_results: list[dict] = []

        self.sig_vc_capabilities.connect(self._on_vc_capabilities)
        self.sig_vc_settings.connect(self._on_vc_settings)
        self.sig_rvc_models.connect(self._on_rvc_models)
        self.sig_hf_results.connect(self._on_hf_results)
        self.sig_hf_repo_files.connect(self._on_hf_repo_files)
        self.sig_delete_result.connect(self._on_delete_result)
        self.sig_hf_token.connect(self._on_hf_token_loaded)
        self.sig_hf_token_saved.connect(self._on_hf_token_saved)
        self.sig_rvc_metrics.connect(self._on_rvc_metrics)
        self._sig_sources_loaded.connect(self._on_sources_loaded)

        self._apply_timer = QTimer(self)
        self._apply_timer.setSingleShot(True)
        self._apply_timer.setInterval(500)

        # Auto-tune controller state
        self._tune_timer = QTimer(self)
        self._tune_timer.setInterval(800)
        self._tune_timer.timeout.connect(
            lambda: DbusWrapper.request_rvc_metrics(self.sig_rvc_metrics))
        self._tune_params: dict = {}
        self._tune_clean_polls = 0
        self._tune_steps_down = 0
        self._tune_skip_first = False
        self._apply_timer.timeout.connect(self._apply)

        outer = QVBoxLayout()
        outer.setContentsMargins(0, 0, 0, 4)
        self.setLayout(outer)

        if show_title:
            title = QLabel(_T('ui', 'vc_title'))
            font = title.font()
            font.setBold(True)
            font.setPointSize(16)
            title.setFont(font)
            outer.addWidget(title)

        # ── Unavailable banner ─────────────────────────────────────────
        self._unavail_frame = QFrame()
        self._unavail_frame.setFrameShape(QFrame.Shape.StyledPanel)
        uf = QVBoxLayout()
        uf.setContentsMargins(10, 8, 10, 8)
        uf.setSpacing(6)
        self._unavail_frame.setLayout(uf)
        self._unavail_label = QLabel()
        self._unavail_label.setWordWrap(True)
        uf.addWidget(self._unavail_label)
        retry_row = QHBoxLayout()
        retry_btn = QPushButton(_T('ui', 'vc_retry'))
        retry_btn.clicked.connect(self._retry_check)
        retry_row.addWidget(retry_btn)
        retry_row.addStretch()
        uf.addLayout(retry_row)
        self._unavail_frame.setVisible(False)
        outer.addWidget(self._unavail_frame)

        # ── Global controls (enable + source + mode) ───────────────────
        global_box = QWidget()
        gl = QVBoxLayout()
        gl.setContentsMargins(0, 0, 0, 0)
        global_box.setLayout(gl)

        en_row = QHBoxLayout()
        en_lbl = QLabel(_T('ui', 'vc_enable'))
        en_lbl.setFixedWidth(110)
        en_row.addWidget(en_lbl)
        self._enable_check = QCheckBox()
        self._enable_check.stateChanged.connect(self._on_enable_changed)
        en_row.addWidget(self._enable_check)
        en_row.addStretch()
        gl.addLayout(en_row)

        dev_row = QHBoxLayout()
        dev_lbl = QLabel(_T('ui', 'vc_input_device'))
        dev_lbl.setFixedWidth(110)
        dev_row.addWidget(dev_lbl)
        self._source_combo = QComboBox()
        self._source_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self._source_combo.currentIndexChanged.connect(lambda _: self._apply())
        dev_row.addWidget(self._source_combo)
        gl.addLayout(dev_row)

        mode_row = QHBoxLayout()
        mode_lbl = QLabel(_T('ui', 'vc_mode'))
        mode_lbl.setFixedWidth(110)
        mode_row.addWidget(mode_lbl)
        self._mode_combo = QComboBox()
        self._mode_combo.addItem(_T('ui', 'vc_mode_ladspa'), 'ladspa')
        self._mode_combo.addItem(_T('ui', 'vc_mode_rvc'),    'rvc')
        self._mode_combo.currentIndexChanged.connect(self._on_mode_changed)
        mode_row.addWidget(self._mode_combo)
        mode_row.addStretch()
        gl.addLayout(mode_row)

        outer.addWidget(global_box)

        # ── Mode stacked widget ────────────────────────────────────────
        self._stack = QStackedWidget()
        self._stack.addWidget(self._build_ladspa_panel())   # index 0
        self._stack.addWidget(self._build_rvc_panel())      # index 1
        outer.addWidget(self._stack, 1)

        self._update_global_state()

    # ── Build LADSPA panel ─────────────────────────────────────────────

    def _build_ladspa_panel(self) -> QWidget:
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        content = QWidget()
        cl = QVBoxLayout()
        cl.setAlignment(Qt.AlignmentFlag.AlignTop)
        content.setLayout(cl)
        scroll.setWidget(content)
        self._ladspa_content = content

        # ── Pitch ──────────────────────────────────────────────────────
        self._pitch_box = QGroupBox(_T('ui', 'vc_pitch_section'))
        pb = QVBoxLayout()
        self._pitch_box.setLayout(pb)

        pe_row = QHBoxLayout()
        pe_row.addWidget(QLabel(_T('ui', 'vc_stage_enable')))
        self._pitch_enable = QCheckBox()
        self._pitch_enable.stateChanged.connect(self._on_pitch_enable)
        pe_row.addWidget(self._pitch_enable)
        pe_row.addStretch()
        pb.addLayout(pe_row)

        row, self._pitch_sl, self._pitch_lbl = _slider_row(
            _T('ui', 'vc_pitch_semitones'), -24, 24, 0,
            lambda v: f'{v:+d} st' if v != 0 else '0 st')
        self._pitch_sl.valueChanged.connect(
            lambda v: (self._pitch_lbl.setText(f'{v:+d} st' if v != 0 else '0 st'),
                       self._apply_timer.start()))
        pb.addLayout(row)

        reset_row = QHBoxLayout()
        reset_row.addSpacing(144)
        rb = QPushButton(_T('ui', 'vc_reset'))
        rb.setFixedWidth(70)
        rb.clicked.connect(lambda: self._pitch_sl.setValue(0))
        reset_row.addWidget(rb)
        reset_row.addStretch()
        pb.addLayout(reset_row)

        self._pitch_sl.setEnabled(False)
        cl.addWidget(self._pitch_box)

        # ── Chorus ─────────────────────────────────────────────────────
        self._chorus_box = QGroupBox(_T('ui', 'vc_chorus_section'))
        chb = QVBoxLayout()
        self._chorus_box.setLayout(chb)

        che_row = QHBoxLayout()
        che_row.addWidget(QLabel(_T('ui', 'vc_stage_enable')))
        self._chorus_enable = QCheckBox()
        self._chorus_enable.stateChanged.connect(self._on_chorus_enable)
        che_row.addWidget(self._chorus_enable)
        che_row.addStretch()
        chb.addLayout(che_row)

        self._chorus_adv = QWidget()
        ca = QVBoxLayout()
        ca.setContentsMargins(0, 4, 0, 0)
        self._chorus_adv.setLayout(ca)

        row, self._ch_voices_sl, self._ch_voices_lbl = _slider_row(
            _T('ui', 'vc_chorus_voices'), 1, 8, 3, str)
        self._ch_voices_sl.valueChanged.connect(
            lambda v: (self._ch_voices_lbl.setText(str(v)), self._apply_timer.start()))
        ca.addLayout(row)

        row, self._ch_delay_sl, self._ch_delay_lbl = _slider_row(
            _T('ui', 'vc_chorus_delay'), 10, 40, 20, lambda v: f'{v} ms')
        self._ch_delay_sl.valueChanged.connect(
            lambda v: (self._ch_delay_lbl.setText(f'{v} ms'), self._apply_timer.start()))
        ca.addLayout(row)

        row, self._ch_sep_sl, self._ch_sep_lbl = _fslider_row(
            _T('ui', 'vc_chorus_sep'), 0.0, 2.0, 0.5, lambda v: f'{v:.2f} ms', steps=200)
        self._ch_sep_sl.valueChanged.connect(
            lambda v: (self._ch_sep_lbl.setText(
                f'{_fval(self._ch_sep_sl, 0.0, 2.0):.2f} ms'), self._apply_timer.start()))
        ca.addLayout(row)

        row, self._ch_detune_sl, self._ch_detune_lbl = _fslider_row(
            _T('ui', 'vc_chorus_detune'), 0.0, 5.0, 1.0, lambda v: f'{v:.1f}%', steps=50)
        self._ch_detune_sl.valueChanged.connect(
            lambda v: (self._ch_detune_lbl.setText(
                f'{_fval(self._ch_detune_sl, 0.0, 5.0):.1f}%'), self._apply_timer.start()))
        ca.addLayout(row)

        row, self._ch_lfo_sl, self._ch_lfo_lbl = _slider_row(
            _T('ui', 'vc_chorus_lfo'), 2, 30, 4, lambda v: f'{v} Hz')
        self._ch_lfo_sl.valueChanged.connect(
            lambda v: (self._ch_lfo_lbl.setText(f'{v} Hz'), self._apply_timer.start()))
        ca.addLayout(row)

        row, self._ch_atten_sl, self._ch_atten_lbl = _slider_row(
            _T('ui', 'vc_chorus_atten'), -20, 0, -3, lambda v: f'{v} dB')
        self._ch_atten_sl.valueChanged.connect(
            lambda v: (self._ch_atten_lbl.setText(f'{v} dB'), self._apply_timer.start()))
        ca.addLayout(row)

        chb.addWidget(self._chorus_adv)
        self._chorus_adv.setVisible(False)
        cl.addWidget(self._chorus_box)

        # ── Delay ──────────────────────────────────────────────────────
        self._delay_box = QGroupBox(_T('ui', 'vc_delay_section'))
        db = QVBoxLayout()
        self._delay_box.setLayout(db)

        de_row = QHBoxLayout()
        de_row.addWidget(QLabel(_T('ui', 'vc_stage_enable')))
        self._delay_enable = QCheckBox()
        self._delay_enable.stateChanged.connect(self._on_delay_enable)
        de_row.addWidget(self._delay_enable)
        de_row.addStretch()
        db.addLayout(de_row)

        self._delay_adv = QWidget()
        da = QVBoxLayout()
        da.setContentsMargins(0, 4, 0, 0)
        self._delay_adv.setLayout(da)

        row, self._delay_sl, self._delay_lbl = _fslider_row(
            _T('ui', 'vc_delay_time'), 0.0, 5.0, 0.3, lambda v: f'{v:.2f} s', steps=500)
        self._delay_sl.valueChanged.connect(
            lambda v: (self._delay_lbl.setText(
                f'{_fval(self._delay_sl, 0.0, 5.0):.2f} s'), self._apply_timer.start()))
        da.addLayout(row)

        db.addWidget(self._delay_adv)
        self._delay_adv.setVisible(False)
        cl.addWidget(self._delay_box)

        # ── Distortion ─────────────────────────────────────────────────
        self._dist_box = QGroupBox(_T('ui', 'vc_distortion_section'))
        disb = QVBoxLayout()
        self._dist_box.setLayout(disb)

        dise_row = QHBoxLayout()
        dise_row.addWidget(QLabel(_T('ui', 'vc_stage_enable')))
        self._dist_enable = QCheckBox()
        self._dist_enable.stateChanged.connect(self._on_dist_enable)
        dise_row.addWidget(self._dist_enable)
        dise_row.addStretch()
        disb.addLayout(dise_row)

        self._dist_adv = QWidget()
        disa = QVBoxLayout()
        disa.setContentsMargins(0, 4, 0, 0)
        self._dist_adv.setLayout(disa)

        row, self._dist_level_sl, self._dist_level_lbl = _fslider_row(
            _T('ui', 'vc_distortion_level'), 0.0, 1.0, 0.3,
            lambda v: f'{v:.2f}', steps=100)
        self._dist_level_sl.valueChanged.connect(
            lambda v: (self._dist_level_lbl.setText(
                f'{_fval(self._dist_level_sl, 0.0, 1.0):.2f}'), self._apply_timer.start()))
        disa.addLayout(row)

        row, self._dist_char_sl, self._dist_char_lbl = _fslider_row(
            _T('ui', 'vc_distortion_char'), 0.0, 1.0, 0.5,
            lambda v: f'{v:.2f}', steps=100)
        self._dist_char_sl.valueChanged.connect(
            lambda v: (self._dist_char_lbl.setText(
                f'{_fval(self._dist_char_sl, 0.0, 1.0):.2f}'), self._apply_timer.start()))
        disa.addLayout(row)

        disb.addWidget(self._dist_adv)
        self._dist_adv.setVisible(False)
        cl.addWidget(self._dist_box)

        # ── Reverb ─────────────────────────────────────────────────────
        self._reverb_box = QGroupBox(_T('ui', 'vc_reverb_section'))
        rb = QVBoxLayout()
        self._reverb_box.setLayout(rb)

        re_row = QHBoxLayout()
        re_row.addWidget(QLabel(_T('ui', 'vc_stage_enable')))
        self._reverb_enable = QCheckBox()
        self._reverb_enable.stateChanged.connect(self._on_reverb_enable)
        re_row.addWidget(self._reverb_enable)
        re_row.addStretch()
        rb.addLayout(re_row)

        self._reverb_adv = QWidget()
        ra = QVBoxLayout()
        ra.setContentsMargins(0, 4, 0, 0)
        self._reverb_adv.setLayout(ra)

        row, self._rev_room_sl, self._rev_room_lbl = _slider_row(
            _T('ui', 'vc_reverb_roomsize'), 1, 300, 30, lambda v: f'{v} m', step=5)
        self._rev_room_sl.valueChanged.connect(
            lambda v: (self._rev_room_lbl.setText(f'{v} m'), self._apply_timer.start()))
        ra.addLayout(row)

        row, self._rev_time_sl, self._rev_time_lbl = _fslider_row(
            _T('ui', 'vc_reverb_time'), 0.1, 30.0, 2.0,
            lambda v: f'{v:.1f} s', steps=299)
        self._rev_time_sl.valueChanged.connect(
            lambda v: (self._rev_time_lbl.setText(
                f'{_fval(self._rev_time_sl, 0.1, 30.0):.1f} s'), self._apply_timer.start()))
        ra.addLayout(row)

        row, self._rev_damp_sl, self._rev_damp_lbl = _fslider_row(
            _T('ui', 'vc_reverb_damping'), 0.0, 1.0, 0.5,
            lambda v: f'{v:.2f}', steps=100)
        self._rev_damp_sl.valueChanged.connect(
            lambda v: (self._rev_damp_lbl.setText(
                f'{_fval(self._rev_damp_sl, 0.0, 1.0):.2f}'), self._apply_timer.start()))
        ra.addLayout(row)

        row, self._rev_bw_sl, self._rev_bw_lbl = _fslider_row(
            _T('ui', 'vc_reverb_bandwidth'), 0.0, 1.0, 0.75,
            lambda v: f'{v:.2f}', steps=100)
        self._rev_bw_sl.valueChanged.connect(
            lambda v: (self._rev_bw_lbl.setText(
                f'{_fval(self._rev_bw_sl, 0.0, 1.0):.2f}'), self._apply_timer.start()))
        ra.addLayout(row)

        row, self._rev_dry_sl, self._rev_dry_lbl = _slider_row(
            _T('ui', 'vc_reverb_dry'), -70, 0, -3, lambda v: f'{v} dB')
        self._rev_dry_sl.valueChanged.connect(
            lambda v: (self._rev_dry_lbl.setText(f'{v} dB'), self._apply_timer.start()))
        ra.addLayout(row)

        row, self._rev_early_sl, self._rev_early_lbl = _slider_row(
            _T('ui', 'vc_reverb_early'), -70, 0, -9, lambda v: f'{v} dB')
        self._rev_early_sl.valueChanged.connect(
            lambda v: (self._rev_early_lbl.setText(f'{v} dB'), self._apply_timer.start()))
        ra.addLayout(row)

        row, self._rev_tail_sl, self._rev_tail_lbl = _slider_row(
            _T('ui', 'vc_reverb_tail'), -70, 0, -12, lambda v: f'{v} dB')
        self._rev_tail_sl.valueChanged.connect(
            lambda v: (self._rev_tail_lbl.setText(f'{v} dB'), self._apply_timer.start()))
        ra.addLayout(row)

        rb.addWidget(self._reverb_adv)
        self._reverb_adv.setVisible(False)
        cl.addWidget(self._reverb_box)

        cl.addStretch()
        return scroll

    # ── Build RVC panel ────────────────────────────────────────────────

    def _build_rvc_panel(self) -> QWidget:
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        content = QWidget()
        cl = QVBoxLayout()
        cl.setAlignment(Qt.AlignmentFlag.AlignTop)
        content.setLayout(cl)
        scroll.setWidget(content)

        # Backend info
        self._rvc_backend_lbl = QLabel()
        self._rvc_backend_lbl.setWordWrap(True)
        cl.addWidget(self._rvc_backend_lbl)

        # No-backend warning + install controls
        self._rvc_no_backend_frame = QFrame()
        self._rvc_no_backend_frame.setFrameShape(QFrame.Shape.StyledPanel)
        nb = QVBoxLayout()
        nb.setContentsMargins(10, 8, 10, 8)
        nb.setSpacing(8)
        self._rvc_no_backend_frame.setLayout(nb)
        self._rvc_no_backend_lbl = QLabel(_T('ui', 'vc_rvc_no_backend'))
        self._rvc_no_backend_lbl.setWordWrap(True)
        nb.addWidget(self._rvc_no_backend_lbl)

        install_row = QHBoxLayout()
        install_row.addStretch(1)
        self._rvc_install_btn = QPushButton(_T('ui', 'vc_rvc_install_ai'))
        self._rvc_install_btn.clicked.connect(self._open_onnxruntime_install_dialog)
        install_row.addWidget(self._rvc_install_btn)
        nb.addLayout(install_row)

        self._rvc_no_backend_frame.setVisible(False)
        cl.addWidget(self._rvc_no_backend_frame)

        # ── Base AI Models ─────────────────────────────────────────────
        self._rvc_base_frame = QFrame()
        self._rvc_base_frame.setFrameShape(QFrame.Shape.StyledPanel)
        bm = QVBoxLayout()
        bm.setContentsMargins(10, 8, 10, 8)
        bm.setSpacing(6)
        self._rvc_base_frame.setLayout(bm)
        base_title = QLabel(_T('ui', 'vc_rvc_base_models'))
        base_title.setStyleSheet('font-weight: bold;')
        bm.addWidget(base_title)
        self._rvc_rmvpe_lbl = QLabel()
        bm.addWidget(self._rvc_rmvpe_lbl)
        self._rvc_contentvec_lbl = QLabel()
        bm.addWidget(self._rvc_contentvec_lbl)
        base_btn_row = QHBoxLayout()
        self._rvc_base_download_btn = QPushButton(_T('ui', 'vc_rvc_base_download'))
        self._rvc_base_download_btn.clicked.connect(self._download_base_models)
        base_btn_row.addWidget(self._rvc_base_download_btn)
        base_btn_row.addStretch()
        bm.addLayout(base_btn_row)
        base_note = QLabel(_T('ui', 'vc_rvc_base_note'))
        base_note.setStyleSheet('font-size: 10px; color: gray;')
        base_note.setWordWrap(True)
        bm.addWidget(base_note)
        cl.addWidget(self._rvc_base_frame)

        # Everything below is disabled until base models are present.
        self._rvc_model_section = QWidget()
        ms = QVBoxLayout()
        ms.setContentsMargins(0, 0, 0, 0)
        ms.setSpacing(cl.spacing())
        self._rvc_model_section.setLayout(ms)
        cl.addWidget(self._rvc_model_section)

        # ── HuBERT encoder ─────────────────────────────────────────────
        hubert_sep = QLabel('HuBERT encoder')
        hubert_sep.setStyleSheet('font-weight: bold; margin-top: 6px;')
        ms.addWidget(hubert_sep)

        hubert_row = QHBoxLayout()
        hubert_lbl = QLabel('Encoder model')
        hubert_lbl.setFixedWidth(110)
        hubert_row.addWidget(hubert_lbl)
        self._rvc_hubert_combo = QComboBox()
        self._rvc_hubert_combo.addItem('HuBERT Base (torchaudio)', 'torchaudio')
        self._rvc_hubert_combo.addItem('ContentVec 500 (speaker-independent)', 'contentvec')
        self._rvc_hubert_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self._rvc_hubert_combo.currentIndexChanged.connect(lambda _: self._apply())
        hubert_row.addWidget(self._rvc_hubert_combo)
        ms.addLayout(hubert_row)

        hubert_hint = QLabel(
            'Try ContentVec if words sound garbled — some models were trained with it.'
        )
        hubert_hint.setStyleSheet('font-size: 10px; color: gray;')
        hubert_hint.setWordWrap(True)
        ms.addWidget(hubert_hint)

        # ── Local models ───────────────────────────────────────────────
        local_sep = QLabel(_T('ui', 'vc_rvc_local_models'))
        local_sep.setStyleSheet('font-weight: bold; margin-top: 6px;')
        ms.addWidget(local_sep)

        model_row = QHBoxLayout()
        model_lbl = QLabel(_T('ui', 'vc_rvc_model'))
        model_lbl.setFixedWidth(80)
        model_row.addWidget(model_lbl)
        self._rvc_model_params: dict = {}   # per-model tunable snapshots
        self._rvc_model_combo = QComboBox()
        self._rvc_model_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self._rvc_model_combo.currentIndexChanged.connect(self._on_rvc_model_changed)
        model_row.addWidget(self._rvc_model_combo)
        refresh_btn = QPushButton('↻')
        refresh_btn.setFixedWidth(28)
        refresh_btn.setToolTip(_T('ui', 'vc_rvc_refresh_models'))
        refresh_btn.clicked.connect(self.refresh_models)
        model_row.addWidget(refresh_btn)
        self._rvc_delete_btn = QPushButton(_T('ui', 'vc_rvc_delete_model'))
        self._rvc_delete_btn.setEnabled(False)
        self._rvc_delete_btn.clicked.connect(self._delete_model)
        model_row.addWidget(self._rvc_delete_btn)
        ms.addLayout(model_row)

        self._rvc_no_models_lbl = QLabel()
        self._rvc_no_models_lbl.setWordWrap(True)
        _italic_font = self._rvc_no_models_lbl.font()
        _italic_font.setItalic(True)
        self._rvc_no_models_lbl.setFont(_italic_font)
        self._rvc_no_models_lbl.setVisible(False)
        ms.addWidget(self._rvc_no_models_lbl)

        row, self._rvc_pitch_sl, self._rvc_pitch_lbl = _slider_row(
            _T('ui', 'vc_rvc_pitch'), -12, 12, 0,
            lambda v: f'{v:+d} st' if v != 0 else '0 st')
        self._rvc_pitch_sl.valueChanged.connect(
            lambda v: (self._rvc_pitch_lbl.setText(f'{v:+d} st' if v != 0 else '0 st'),
                       self._apply_timer.start()))
        ms.addLayout(row)

        # ── Advanced tuning (persisted per model) ──────────────────────
        adv_sep = QLabel('Advanced tuning (saved per model)')
        adv_sep.setStyleSheet('font-weight: bold; margin-top: 6px;')
        ms.addWidget(adv_sep)

        # VTLN warp: <1 shifts formants up (male voice → female-trained model)
        row, self._rvc_vtln_sl, self._rvc_vtln_lbl = _slider_row(
            'VTLN warp α', 65, 100, 100, lambda v: f'{v/100:.2f}')
        self._rvc_vtln_sl.setToolTip(
            'Vocal tract length normalization: shifts formant frequencies in the\n'
            'HuBERT input so the model hears your voice as if from a shorter vocal\n'
            'tract. Use when your voice is male and the model was trained on female\n'
            'audio (try 0.80; lower = stronger shift). Pitch is unaffected.\n'
            '1.00 = disabled.'
        )
        self._rvc_vtln_sl.valueChanged.connect(
            lambda v: (self._rvc_vtln_lbl.setText(f'{v/100:.2f}'),
                       self._apply_timer.start()))
        ms.addLayout(row)

        # Envelope mix: 0 % = follow input dynamics, 100 % = model's own envelope
        row, self._rvc_rms_sl, self._rvc_rms_lbl = _slider_row(
            'Envelope mix', 0, 100, 25, lambda v: f'{v} %')
        self._rvc_rms_sl.setToolTip(
            'How much of the model\'s own volume envelope to keep.\n'
            'Lower values make the output follow your speaking dynamics,\n'
            'reducing the sustained hot levels that sound like clipping.'
        )
        self._rvc_rms_sl.valueChanged.connect(
            lambda v: (self._rvc_rms_lbl.setText(f'{v} %'),
                       self._apply_timer.start()))
        ms.addLayout(row)

        # F0 median filter: 0 = off, 3/5/7 = radius
        f0f_row = QHBoxLayout()
        f0f_lbl = QLabel('F0 smoothing')
        f0f_lbl.setFixedWidth(140)
        f0f_row.addWidget(f0f_lbl)
        self._rvc_f0filt_combo = QComboBox()
        self._rvc_f0filt_combo.addItem('Off', 0)
        self._rvc_f0filt_combo.addItem('Light (3)', 3)
        self._rvc_f0filt_combo.addItem('Medium (5)', 5)
        self._rvc_f0filt_combo.addItem('Strong (7)', 7)
        self._rvc_f0filt_combo.setCurrentIndex(1)
        self._rvc_f0filt_combo.setToolTip(
            'Median filter on the detected pitch curve. Removes single-frame\n'
            'pitch spikes that cause crackle/glottal bursts in the output.'
        )
        self._rvc_f0filt_combo.currentIndexChanged.connect(lambda _: self._apply())
        f0f_row.addWidget(self._rvc_f0filt_combo)
        f0f_row.addStretch()
        ms.addLayout(f0f_row)

        # Input drive: RMS level fed to the model (×100)
        row, self._rvc_drive_sl, self._rvc_drive_lbl = _slider_row(
            'Input drive', 2, 15, 6, lambda v: f'{v/100:.2f}')
        self._rvc_drive_sl.setToolTip(
            'Input level fed to the model. Higher = louder, fuller output but\n'
            'risks saturating the synthesizer (harmonic distortion / clipping).\n'
            'Lower this if you hear clipping on accented syllables.'
        )
        self._rvc_drive_sl.valueChanged.connect(
            lambda v: (self._rvc_drive_lbl.setText(f'{v/100:.2f}'),
                       self._apply_timer.start()))
        ms.addLayout(row)

        # FAISS feature-retrieval blend (×100); 0 = off, needs a .index file
        row, self._rvc_index_sl, self._rvc_index_lbl = _slider_row(
            'Index rate', 0, 100, 0,
            lambda v: 'Off' if v == 0 else f'{v/100:.2f}')
        self._rvc_index_sl.setToolTip(
            'Blends each voice feature with its nearest neighbours from the\n'
            "model's training set (needs a .index file next to the .pth).\n"
            'Higher values pull the timbre toward the training voice and\n'
            'stabilise out-of-distribution input like creaky word endings.\n'
            '0 disables retrieval; typical values are 0.30–0.75.'
        )
        self._rvc_index_sl.valueChanged.connect(
            lambda v: (self._rvc_index_lbl.setText('Off' if v == 0 else f'{v/100:.2f}'),
                       self._apply_timer.start()))
        ms.addLayout(row)

        # Output soft limiter knee (×100); 1.00 = off
        row, self._rvc_lim_sl, self._rvc_lim_lbl = _slider_row(
            'Limiter knee', 50, 100, 80,
            lambda v: 'Off' if v >= 100 else f'{v/100:.2f}')
        self._rvc_lim_sl.setToolTip(
            'Output peaks above this level are softly compressed.\n'
            '1.00 disables the limiter entirely.'
        )
        self._rvc_lim_sl.valueChanged.connect(
            lambda v: (self._rvc_lim_lbl.setText('Off' if v >= 100 else f'{v/100:.2f}'),
                       self._apply_timer.start()))
        ms.addLayout(row)

        # Auto-tune: closed-loop drive calibration against live saturation metrics
        tune_row = QHBoxLayout()
        self._rvc_tune_btn = QPushButton('Auto-tune (speak normally)')
        self._rvc_tune_btn.setCheckable(True)
        self._rvc_tune_btn.setToolTip(
            'Monitors the model output while you speak and lowers the input\n'
            'drive until saturation (clipping) disappears, then saves the\n'
            'result for this model. Click again to stop early.'
        )
        self._rvc_tune_btn.toggled.connect(self._on_tune_toggled)
        tune_row.addWidget(self._rvc_tune_btn)
        self._rvc_tune_status = QLabel('')
        self._rvc_tune_status.setStyleSheet('font-size: 10px; color: gray;')
        tune_row.addWidget(self._rvc_tune_status, 1)
        ms.addLayout(tune_row)

        # Guided calibration: read a short text, hear 3 tunings, pick by ear.
        # Gated on rvc_avail in _on_vc_capabilities — recording alone works
        # without a backend, but the wizard's render step does not.
        calib_row = QHBoxLayout()
        self._calib_btn = QPushButton(_T('ui', 'vc_calib_button'))
        self._calib_btn.setToolTip(
            'Read a short text once; the daemon renders it through three\n'
            'candidate tunings. Listen (original included), pick the best,\n'
            'optionally refine, then save it for this model.'
        )
        self._calib_btn.clicked.connect(self._open_calibration_wizard)
        calib_row.addWidget(self._calib_btn, 1)
        reset_btn = QPushButton(_T('ui', 'vc_reset_params'))
        reset_btn.setToolTip('Revert all tuning for this model to the defaults.')
        reset_btn.clicked.connect(self._reset_model_params)
        calib_row.addWidget(reset_btn)
        ms.addLayout(calib_row)

        open_folder_btn = QPushButton(_T('ui', 'vc_rvc_open_folder'))
        open_folder_btn.clicked.connect(self._open_models_folder)
        ms.addWidget(open_folder_btn)

        # ── HuggingFace search ─────────────────────────────────────────
        hf_sep = QLabel(_T('ui', 'vc_rvc_hf_title'))
        hf_sep.setStyleSheet('font-weight: bold; margin-top: 10px;')
        ms.addWidget(hf_sep)

        search_row = QHBoxLayout()
        self._hf_search_input = QLineEdit()
        self._hf_search_input.setPlaceholderText(_T('ui', 'vc_rvc_hf_search_placeholder'))
        self._hf_search_input.returnPressed.connect(self._hf_search)
        search_row.addWidget(self._hf_search_input, 1)
        self._hf_sort_combo = QComboBox()
        self._hf_sort_combo.addItem(_T('ui', 'vc_rvc_hf_sort_trending'), 'trendingScore')
        self._hf_sort_combo.addItem(_T('ui', 'vc_rvc_hf_sort_downloads'), 'downloads')
        self._hf_sort_combo.addItem(_T('ui', 'vc_rvc_hf_sort_likes'), 'likes')
        search_row.addWidget(self._hf_sort_combo)
        self._hf_search_btn = QPushButton(_T('ui', 'vc_rvc_hf_search_btn'))
        self._hf_search_btn.clicked.connect(self._hf_search)
        search_row.addWidget(self._hf_search_btn)
        ms.addLayout(search_row)

        self._hf_status_lbl = QLabel()
        self._hf_status_lbl.setVisible(False)
        ms.addWidget(self._hf_status_lbl)

        self._hf_results_list = QListWidget()
        self._hf_results_list.setMaximumHeight(160)
        self._hf_results_list.setVisible(False)
        self._hf_results_list.currentRowChanged.connect(self._on_hf_result_selected)
        ms.addWidget(self._hf_results_list)

        dl_row = QHBoxLayout()
        dl_lbl = QLabel(_T('ui', 'vc_rvc_hf_file'))
        dl_lbl.setFixedWidth(80)
        dl_row.addWidget(dl_lbl)
        self._hf_file_combo = QComboBox()
        self._hf_file_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self._hf_file_combo.setEnabled(False)
        dl_row.addWidget(self._hf_file_combo, 1)
        self._hf_download_btn = QPushButton(_T('ui', 'vc_rvc_hf_download'))
        self._hf_download_btn.setEnabled(False)
        self._hf_download_btn.clicked.connect(self._hf_download)
        dl_row.addWidget(self._hf_download_btn)
        self._dl_row_widget = QWidget()
        self._dl_row_widget.setLayout(dl_row)
        self._dl_row_widget.setVisible(False)
        ms.addWidget(self._dl_row_widget)

        # ── HuggingFace token ──────────────────────────────────────────
        tok_sep = QLabel(_T('ui', 'vc_rvc_hf_token_section'))
        tok_sep.setStyleSheet('font-weight: bold; margin-top: 8px;')
        ms.addWidget(tok_sep)

        tok_row = QHBoxLayout()
        self._hf_token_input = QLineEdit()
        self._hf_token_input.setEchoMode(QLineEdit.EchoMode.Password)
        self._hf_token_input.setPlaceholderText(_T('ui', 'vc_rvc_hf_token_placeholder'))
        tok_row.addWidget(self._hf_token_input, 1)
        self._hf_token_save_btn = QPushButton(_T('ui', 'vc_rvc_hf_token_save'))
        self._hf_token_save_btn.clicked.connect(self._save_hf_token)
        tok_row.addWidget(self._hf_token_save_btn)
        ms.addLayout(tok_row)

        self._hf_token_hint = QLabel(_T('ui', 'vc_rvc_hf_token_hint'))
        self._hf_token_hint.setStyleSheet('font-size: 10px; color: gray;')
        self._hf_token_hint.setOpenExternalLinks(True)
        ms.addWidget(self._hf_token_hint)

        cl.addStretch()
        return scroll

    # ── Lifecycle ──────────────────────────────────────────────────────

    def showEvent(self, event) -> None:  # noqa: N802
        super().showEvent(event)
        self.refresh()

    def refresh(self) -> None:
        threading.Thread(target=self._load_sources_thread, daemon=True).start()
        DbusWrapper.request_vc_capabilities(self.sig_vc_capabilities)
        DbusWrapper.request_vc_settings(self.sig_vc_settings)
        DbusWrapper.get_hf_token(self.sig_hf_token)

    # ── Source loading ─────────────────────────────────────────────────

    def _load_sources_thread(self) -> None:
        try:
            with pulsectl.Pulse('lam-vc-sources') as pulse:
                default_name = pulse.server_info().default_source_name
                sources = [s for s in pulse.source_list() if not s.name.endswith('.monitor')]
            self._sig_sources_loaded.emit({
                'sources': [{'id': s.name, 'name': s.description} for s in sources],
                'default': default_name,
            })
        except Exception as e:
            logger.warning('Failed to load PulseAudio sources: %s', e)

    def _on_sources_loaded(self, data: dict) -> None:
        self._populate_sources(data.get('sources', []), prefer_id=data.get('default', ''))

    def _populate_sources(self, sources: list[dict], prefer_id: str = '') -> None:
        self._sources = sources
        self._source_combo.blockSignals(True)
        self._source_combo.clear()
        for src in sources:
            self._source_combo.addItem(src['name'], src['id'])
        idx = next((i for i, s in enumerate(sources) if s['id'] == prefer_id), 0)
        self._source_combo.setCurrentIndex(idx)
        self._source_combo.blockSignals(False)

    # ── D-Bus handlers ─────────────────────────────────────────────────

    def _on_vc_capabilities(self, caps: dict) -> None:
        self._ladspa_caps = caps.get('ladspa', {})
        self._rvc_caps    = caps.get('rvc', {})

        # Show unavailable banner if any LADSPA effects are missing
        missing = [k for k, v in self._ladspa_caps.items() if not v]
        self._unavail_frame.setVisible(bool(missing))
        if missing:
            self._unavail_label.setText('\n'.join([
                _T('ui', 'vc_ladspa_unavailable'),
                '',
                _T('ui', 'vc_plugin_install_hint'),
                '  ' + _T('ui', 'vc_ladspa_install_fedora'),
                '  ' + _T('ui', 'vc_ladspa_install_debian'),
                '  ' + _T('ui', 'vc_ladspa_install_arch'),
            ]))

        self._pitch_box.setEnabled(self._ladspa_caps.get('pitch', False))
        self._chorus_box.setEnabled(self._ladspa_caps.get('chorus', False))
        self._delay_box.setEnabled(self._ladspa_caps.get('delay', False))
        self._dist_box.setEnabled(self._ladspa_caps.get('distortion', False))
        self._reverb_box.setEnabled(self._ladspa_caps.get('reverb', False))

        # RVC panel — base models
        base = self._rvc_caps.get('base_models', {})
        rmvpe_ok     = bool(base.get('rmvpe', False))
        contentvec_ok = bool(base.get('contentvec', False))
        base_ok = rmvpe_ok and contentvec_ok
        ok_mark, fail_mark = '✔', '✘'
        self._rvc_rmvpe_lbl.setText(
            f'{ok_mark if rmvpe_ok else fail_mark}  RMVPE (~180 MB)'
        )
        self._rvc_contentvec_lbl.setText(
            f'{ok_mark if contentvec_ok else fail_mark}  ContentVec (~360 MB)'
        )
        self._rvc_base_download_btn.setEnabled(not base_ok)
        self._rvc_base_download_btn.setText(
            _T('ui', 'vc_rvc_base_download_done') if base_ok
            else _T('ui', 'vc_rvc_base_download')
        )
        self._rvc_model_section.setEnabled(base_ok)

        # RVC panel — AI backend
        rvc_avail      = bool(self._rvc_caps.get('available', False))
        backends       = self._rvc_caps.get('backends', [])
        self._rvc_no_backend_frame.setVisible(not rvc_avail)
        if not rvc_avail:
            self._rvc_no_backend_lbl.setText(_T('ui', 'vc_rvc_no_backend'))
        self._rvc_backend_lbl.setText(
            _T('ui', 'vc_rvc_backend') + ': ' + (', '.join(backends) if backends else '—')
        )
        self._rvc_model_combo.setEnabled(rvc_avail)
        self._calib_btn.setEnabled(rvc_avail)

        models: list[dict] = self._rvc_caps.get('models', [])
        self._refresh_rvc_models(models, self._rvc_caps.get('models_folder', ''))

        sources: list[dict] = caps.get('sources', [])
        if sources and not self._sources:
            self._populate_sources(sources)

    def refresh_models(self) -> None:
        DbusWrapper.request_rvc_models(self.sig_rvc_models)

    def _on_rvc_models(self, models: list) -> None:
        folder = self._rvc_caps.get('models_folder', '')
        self._refresh_rvc_models(models, folder)

    def _refresh_rvc_models(self, models: list[dict], folder: str) -> None:
        current = self._rvc_model_combo.currentData()
        self._rvc_model_combo.blockSignals(True)
        self._rvc_model_combo.clear()
        for m in models:
            label = m['name'] + (' (with index)' if m.get('has_index') else '')
            self._rvc_model_combo.addItem(label, m['name'])

        # Honour a post-download pending selection, then fall back to previous selection
        target = self._pending_model_select or current
        self._pending_model_select = None
        if target:
            idx = self._rvc_model_combo.findData(target)
            if idx >= 0:
                self._rvc_model_combo.setCurrentIndex(idx)
        self._rvc_model_combo.blockSignals(False)

        has_models = bool(models)
        self._rvc_no_models_lbl.setVisible(not has_models)
        self._rvc_delete_btn.setEnabled(has_models)
        if not has_models and folder:
            self._rvc_no_models_lbl.setText(_T('ui', 'vc_rvc_no_models') + f'\n{folder}')

    def _on_vc_settings(self, settings: dict) -> None:
        self._loading = True

        self._enable_check.blockSignals(True)
        self._enable_check.setChecked(bool(settings.get('enabled', False)))
        self._enable_check.blockSignals(False)

        source_id = settings.get('source_id', '')
        if source_id:
            idx = next((i for i, s in enumerate(self._sources) if s['id'] == source_id), None)
            if idx is not None:
                self._source_combo.blockSignals(True)
                self._source_combo.setCurrentIndex(idx)
                self._source_combo.blockSignals(False)

        mode = settings.get('mode', 'ladspa')
        mode_idx = self._mode_combo.findData(mode)
        if mode_idx >= 0:
            self._mode_combo.blockSignals(True)
            self._mode_combo.setCurrentIndex(mode_idx)
            self._mode_combo.blockSignals(False)
            self._stack.setCurrentIndex(mode_idx)

        # Pitch
        p = settings.get('pitch', {})
        self._pitch_enable.setChecked(bool(p.get('enabled', False)))
        self._pitch_sl.blockSignals(True)
        self._pitch_sl.setValue(round(float(p.get('semitones', 0.0))))
        self._pitch_sl.blockSignals(False)
        v = self._pitch_sl.value()
        self._pitch_lbl.setText(f'{v:+d} st' if v != 0 else '0 st')

        # Chorus
        c = settings.get('chorus', {})
        self._chorus_enable.setChecked(bool(c.get('enabled', False)))
        self._ch_voices_sl.setValue(int(c.get('voices', 3)))
        self._ch_delay_sl.setValue(int(c.get('delay_ms', 20)))
        self._ch_atten_sl.setValue(int(c.get('atten_db', -3)))
        self._ch_lfo_sl.setValue(int(c.get('lfo_hz', 4)))
        self._set_fslider(self._ch_sep_sl, float(c.get('sep_ms', 0.5)), 0.0, 2.0)
        self._set_fslider(self._ch_detune_sl, float(c.get('detune_pct', 1.0)), 0.0, 5.0)

        # Delay
        d = settings.get('delay', {})
        self._delay_enable.setChecked(bool(d.get('enabled', False)))
        self._set_fslider(self._delay_sl, float(d.get('delay_s', 0.3)), 0.0, 5.0)

        # Distortion
        dist = settings.get('distortion', {})
        self._dist_enable.setChecked(bool(dist.get('enabled', False)))
        self._set_fslider(self._dist_level_sl, float(dist.get('level', 0.3)), 0.0, 1.0)
        self._set_fslider(self._dist_char_sl,  float(dist.get('character', 0.5)), 0.0, 1.0)

        # Reverb
        r = settings.get('reverb', {})
        self._reverb_enable.setChecked(bool(r.get('enabled', False)))
        self._rev_room_sl.setValue(int(r.get('roomsize_m', 30)))
        self._set_fslider(self._rev_time_sl, float(r.get('time_s', 2.0)), 0.1, 30.0)
        self._set_fslider(self._rev_damp_sl, float(r.get('damping', 0.5)), 0.0, 1.0)
        self._set_fslider(self._rev_bw_sl,   float(r.get('bandwidth', 0.75)), 0.0, 1.0)
        self._rev_dry_sl.setValue(int(r.get('dry_db', -3)))
        self._rev_early_sl.setValue(int(r.get('early_db', -9)))
        self._rev_tail_sl.setValue(int(r.get('tail_db', -12)))

        # RVC
        rv = settings.get('rvc', {})
        self._rvc_model_params = dict(rv.get('model_params', {}) or {})
        rvc_model = str(rv.get('model', ''))
        if rvc_model:
            idx = self._rvc_model_combo.findData(rvc_model)
            if idx >= 0:
                self._rvc_model_combo.setCurrentIndex(idx)
        # The flat rvc keys carry the active model's tunables
        self._restore_model_params(rv)

        self._loading = False
        self._update_global_state()

    # ── RVC per-model params ───────────────────────────────────────────

    def _on_rvc_model_changed(self, _: int) -> None:
        if self._tune_timer.isActive():
            self._finish_tune(persist=False, message='Model changed — tuning aborted.')
        if not self._loading:
            name = self._rvc_model_combo.currentData()
            saved = self._rvc_model_params.get(name) if name else None
            if saved:
                self._restore_model_params(saved)
        self._apply()

    def _restore_model_params(self, mp: dict) -> None:
        """Set the per-model tunable controls from a saved snapshot (no signals)."""
        if not mp:
            return

        def _sl(sl: QSlider, lbl: QLabel, value: int, fmt) -> None:
            sl.blockSignals(True)
            sl.setValue(value)
            sl.blockSignals(False)
            lbl.setText(fmt(value))

        v = int(round(float(mp.get('pitch_offset', 0.0))))
        _sl(self._rvc_pitch_sl, self._rvc_pitch_lbl, v,
            lambda x: f'{x:+d} st' if x != 0 else '0 st')
        hidx = self._rvc_hubert_combo.findData(str(mp.get('hubert_model', 'torchaudio')))
        if hidx >= 0:
            self._rvc_hubert_combo.blockSignals(True)
            self._rvc_hubert_combo.setCurrentIndex(hidx)
            self._rvc_hubert_combo.blockSignals(False)
        v = int(round(float(mp.get('vtln_alpha', 1.0)) * 100))
        _sl(self._rvc_vtln_sl, self._rvc_vtln_lbl, v, lambda x: f'{x/100:.2f}')
        v = int(round(float(mp.get('rms_mix_rate', 0.25)) * 100))
        _sl(self._rvc_rms_sl, self._rvc_rms_lbl, v, lambda x: f'{x} %')
        fidx = self._rvc_f0filt_combo.findData(int(mp.get('filter_radius', 3)))
        if fidx >= 0:
            self._rvc_f0filt_combo.blockSignals(True)
            self._rvc_f0filt_combo.setCurrentIndex(fidx)
            self._rvc_f0filt_combo.blockSignals(False)
        v = int(round(float(mp.get('target_rms', 0.06)) * 100))
        _sl(self._rvc_drive_sl, self._rvc_drive_lbl, v, lambda x: f'{x/100:.2f}')
        v = int(round(float(mp.get('limiter_thr', 0.80)) * 100))
        _sl(self._rvc_lim_sl, self._rvc_lim_lbl, v,
            lambda x: 'Off' if x >= 100 else f'{x/100:.2f}')
        v = int(round(float(mp.get('index_rate', 0.0)) * 100))
        _sl(self._rvc_index_sl, self._rvc_index_lbl, v,
            lambda x: 'Off' if x == 0 else f'{x/100:.2f}')

    def _current_model_params(self) -> dict:
        return {
            'pitch_offset':  float(self._rvc_pitch_sl.value()),
            'hubert_model':  self._rvc_hubert_combo.currentData() or 'torchaudio',
            'vtln_alpha':    self._rvc_vtln_sl.value() / 100.0,
            'rms_mix_rate':  self._rvc_rms_sl.value() / 100.0,
            'filter_radius': int(self._rvc_f0filt_combo.currentData() or 0),
            'target_rms':    self._rvc_drive_sl.value() / 100.0,
            'limiter_thr':   self._rvc_lim_sl.value() / 100.0,
            'index_rate':    self._rvc_index_sl.value() / 100.0,
        }

    # ── Auto-tune ──────────────────────────────────────────────────────
    #
    # Closed loop against the daemon's per-hop saturation metrics:
    #   sat_ratio = fraction of output samples riding the generator's tanh
    #   rail while speaking.  > 2 % → step the input drive down (live, no
    #   chain rebuild); at the drive floor, pull the envelope mix down.
    #   4 consecutive clean polls → converged: push values into the sliders
    #   and persist per model.

    def _on_tune_toggled(self, checked: bool) -> None:
        if checked:
            self._tune_params = self._current_model_params()
            self._tune_clean_polls = 0
            self._tune_steps_down = 0
            self._tune_skip_first = True
            # First response drains hops recorded under the old params — ignored.
            DbusWrapper.request_rvc_metrics(self.sig_rvc_metrics)
            self._rvc_tune_status.setText('Listening… speak normally.')
            self._tune_timer.start()
        else:
            self._finish_tune(persist=self._tune_steps_down > 0)

    def _reset_model_params(self) -> None:
        """Revert the current model's tuning to the RVCParams defaults."""
        reply = QMessageBox.question(
            self, _T('ui', 'vc_reset_params'),
            _T('ui', 'vc_reset_params_confirm'))
        if reply != QMessageBox.StandardButton.Yes:
            return
        from dataclasses import asdict

        from linux_arctis_manager.voice_changer.rvc.backend import RVCParams
        defaults = {'pitch_offset': 0.0, **asdict(RVCParams())}
        self._restore_model_params(defaults)
        self._apply()
        self._rvc_tune_status.setText(_T('ui', 'vc_reset_params_done'))

    def _open_calibration_wizard(self) -> None:
        from linux_arctis_manager.gui.vc_calibration_wizard import QVCCalibrationWizard
        wiz = QVCCalibrationWizard(self)
        if wiz.exec() and wiz.chosen_params:
            p = wiz.chosen_params
            # The wizard's params dict has no pitch_offset (it tunes timbre/
            # dynamics only); keep the current slider value for it.
            p.setdefault('pitch_offset', float(self._rvc_pitch_sl.value()))
            self._restore_model_params(p)
            self._apply()
            self._rvc_tune_status.setText(_T('ui', 'vc_calib_saved'))

    def _finish_tune(self, persist: bool, message: str = '') -> None:
        self._tune_timer.stop()
        self._rvc_tune_btn.blockSignals(True)
        self._rvc_tune_btn.setChecked(False)
        self._rvc_tune_btn.blockSignals(False)
        if persist and self._tune_params:
            self._restore_model_params(self._tune_params)
            self._apply()
            message = message or (
                f'Saved: drive {self._tune_params["target_rms"]:.3f}, '
                f'mix {self._tune_params["rms_mix_rate"]:.2f}')
        self._rvc_tune_status.setText(message or 'Stopped.')

    def _on_rvc_metrics(self, m: object) -> None:
        if not self._tune_timer.isActive() or not isinstance(m, dict) or not m:
            return
        if self._tune_skip_first:
            self._tune_skip_first = False
            return
        if int(m.get('speaking_hops', 0)) < 3:
            self._rvc_tune_status.setText('Waiting for speech…')
            return
        sat = float(m.get('sat_ratio', 0.0))
        drive = float(self._tune_params.get('target_rms', 0.06))
        if sat > 0.02:
            self._tune_clean_polls = 0
            if drive > 0.021:
                self._tune_params['target_rms'] = round(max(drive - 0.005, 0.02), 3)
            else:
                mix = float(self._tune_params.get('rms_mix_rate', 0.25))
                if mix > 0.06:
                    self._tune_params['rms_mix_rate'] = round(mix - 0.05, 2)
                else:
                    self._finish_tune(
                        persist=True,
                        message='Saturation persists at the floor — saved best effort.')
                    return
            self._tune_steps_down += 1
            DbusWrapper.set_rvc_live_params(self._tune_params)
            self._rvc_tune_status.setText(
                f'sat {sat*100:.1f} % → drive {self._tune_params["target_rms"]:.3f}, '
                f'mix {self._tune_params["rms_mix_rate"]:.2f}')
        elif sat < 0.002:
            self._tune_clean_polls += 1
            self._rvc_tune_status.setText(
                f'clean {self._tune_clean_polls}/4 (sat {sat*100:.2f} %)')
            if self._tune_clean_polls >= 4:
                if self._tune_steps_down == 0:
                    self._finish_tune(persist=False,
                                      message='Already clean — nothing to adjust.')
                else:
                    self._finish_tune(persist=True)
        else:
            self._tune_clean_polls = 0
            self._rvc_tune_status.setText(f'borderline (sat {sat*100:.2f} %)')

    # ── Effect enable toggles ──────────────────────────────────────────

    def _on_pitch_enable(self, _: int) -> None:
        self._pitch_sl.setEnabled(self._pitch_enable.isChecked())
        self._apply()

    def _on_chorus_enable(self, _: int) -> None:
        self._chorus_adv.setVisible(self._chorus_enable.isChecked())
        self._apply()

    def _on_delay_enable(self, _: int) -> None:
        self._delay_adv.setVisible(self._delay_enable.isChecked())
        self._apply()

    def _on_dist_enable(self, _: int) -> None:
        self._dist_adv.setVisible(self._dist_enable.isChecked())
        self._apply()

    def _on_reverb_enable(self, _: int) -> None:
        self._reverb_adv.setVisible(self._reverb_enable.isChecked())
        self._apply()

    # ── Mode / global helpers ──────────────────────────────────────────

    def _on_mode_changed(self, idx: int) -> None:
        self._stack.setCurrentIndex(idx)
        self._apply()

    def _on_enable_changed(self, _: int) -> None:
        self._update_global_state()
        self._apply()

    def _update_global_state(self) -> None:
        enabled = self._enable_check.isChecked()
        self._source_combo.setEnabled(enabled)
        self._mode_combo.setEnabled(enabled)
        self._stack.setEnabled(enabled)

    @staticmethod
    def _set_fslider(sl: QSlider, value: float, minimum: float, maximum: float) -> None:
        sl.blockSignals(True)
        frac = (value - minimum) / (maximum - minimum) if maximum != minimum else 0
        sl.setValue(round(frac * sl.maximum()))
        sl.blockSignals(False)

    # ── Apply / retry ──────────────────────────────────────────────────

    def _apply(self) -> None:
        if self._loading:
            return
        rvc_model = self._rvc_model_combo.currentData() or ''
        rvc_params = self._current_model_params()
        if rvc_model:
            self._rvc_model_params[rvc_model] = dict(rvc_params)
        DbusWrapper.set_vc_settings({
            'enabled':   self._enable_check.isChecked(),
            'mode':      self._mode_combo.currentData() or 'ladspa',
            'source_id': self._source_combo.currentData() or '',
            'pitch': {
                'enabled':   self._pitch_enable.isChecked(),
                'semitones': float(self._pitch_sl.value()),
            },
            'chorus': {
                'enabled':    self._chorus_enable.isChecked(),
                'voices':     self._ch_voices_sl.value(),
                'delay_ms':   float(self._ch_delay_sl.value()),
                'sep_ms':     _fval(self._ch_sep_sl, 0.0, 2.0),
                'detune_pct': _fval(self._ch_detune_sl, 0.0, 5.0),
                'lfo_hz':     float(self._ch_lfo_sl.value()),
                'atten_db':   float(self._ch_atten_sl.value()),
            },
            'delay': {
                'enabled': self._delay_enable.isChecked(),
                'delay_s': _fval(self._delay_sl, 0.0, 5.0),
            },
            'distortion': {
                'enabled':   self._dist_enable.isChecked(),
                'level':     _fval(self._dist_level_sl, 0.0, 1.0),
                'character': _fval(self._dist_char_sl, 0.0, 1.0),
            },
            'reverb': {
                'enabled':    self._reverb_enable.isChecked(),
                'roomsize_m': float(self._rev_room_sl.value()),
                'time_s':     _fval(self._rev_time_sl, 0.1, 30.0),
                'damping':    _fval(self._rev_damp_sl, 0.0, 1.0),
                'bandwidth':  _fval(self._rev_bw_sl, 0.0, 1.0),
                'dry_db':     float(self._rev_dry_sl.value()),
                'early_db':   float(self._rev_early_sl.value()),
                'tail_db':    float(self._rev_tail_sl.value()),
            },
            'rvc': {
                'model': rvc_model,
                **rvc_params,
                'model_params': self._rvc_model_params,
            },
        })
        self.sig_applied.emit()

    # ── Model management ───────────────────────────────────────────────

    def _delete_model(self) -> None:
        name = self._rvc_model_combo.currentData()
        if not name:
            return
        reply = QMessageBox.question(
            self,
            _T('ui', 'vc_rvc_delete_confirm_title'),
            _T('ui', 'vc_rvc_delete_confirm_msg').format(name=name),
        )
        if reply == QMessageBox.StandardButton.Yes:
            DbusWrapper.delete_rvc_model(name, self.sig_delete_result)

    def _on_delete_result(self, success) -> None:
        if success:
            self.refresh_models()

    def _open_models_folder(self) -> None:
        folder = self._rvc_caps.get('models_folder', '')
        if folder:
            subprocess.Popen(['xdg-open', folder])

    # ── HuggingFace search ─────────────────────────────────────────────

    def _hf_search(self) -> None:
        query   = self._hf_search_input.text().strip()
        sort_by = self._hf_sort_combo.currentData() or 'downloads'
        self._hf_search_btn.setEnabled(False)
        self._hf_status_lbl.setText(_T('ui', 'vc_rvc_hf_searching'))
        self._hf_status_lbl.setVisible(True)
        self._hf_results_list.setVisible(False)
        self._dl_row_widget.setVisible(False)
        DbusWrapper.search_hf_models(query, sort_by, self.sig_hf_results)

    def _on_hf_results(self, results: list) -> None:
        self._hf_search_btn.setEnabled(True)
        self._hf_results = results
        self._hf_results_list.clear()

        if not results:
            self._hf_status_lbl.setText(_T('ui', 'vc_rvc_hf_no_results'))
            self._hf_status_lbl.setVisible(True)
            self._hf_results_list.setVisible(False)
            return

        self._hf_status_lbl.setVisible(False)
        sort_field = self._hf_sort_combo.currentData() or 'trendingScore'
        for r in results:
            downloads = r.get('downloads', 0) or 0
            likes     = r.get('likes', 0) or 0
            if sort_field == 'downloads':
                count, suffix = downloads, '↓'
            elif sort_field == 'likes':
                count, suffix = likes, '♥'
            else:
                count, suffix = downloads, '↓'  # trending: show downloads as context
            count_str = f'{count:,} {suffix}' if count else ''
            label = f"{r['repo_id']}   {count_str}"
            item = QListWidgetItem(label)
            item.setData(Qt.ItemDataRole.UserRole, r['repo_id'])
            self._hf_results_list.addItem(item)
        self._hf_results_list.setVisible(True)

    def _on_hf_result_selected(self, row: int) -> None:
        if row < 0 or row >= len(self._hf_results):
            return
        repo_id = self._hf_results[row]['repo_id']
        self._hf_file_combo.clear()
        self._hf_file_combo.setEnabled(False)
        self._hf_download_btn.setEnabled(False)
        self._dl_row_widget.setVisible(True)
        self._hf_file_combo.addItem(_T('ui', 'vc_rvc_hf_loading_files'))
        DbusWrapper.list_repo_model_files(repo_id, self.sig_hf_repo_files)

    def _on_hf_repo_files(self, files: list) -> None:
        self._hf_file_combo.clear()
        if not files:
            self._hf_file_combo.addItem(_T('ui', 'vc_rvc_hf_no_model_files'))
            self._hf_file_combo.setEnabled(False)
            self._hf_download_btn.setEnabled(False)
        else:
            for f in files:
                self._hf_file_combo.addItem(f)
            self._hf_file_combo.setEnabled(True)
            self._hf_download_btn.setEnabled(True)

    def _hf_download(self) -> None:
        row = self._hf_results_list.currentRow()
        if row < 0 or row >= len(self._hf_results):
            return
        repo_id  = self._hf_results[row]['repo_id']
        if not self._hf_file_combo.isEnabled():
            return
        filename = self._hf_file_combo.currentText()
        if not filename:
            return
        self._hf_download_btn.setEnabled(False)
        DbusWrapper.download_hf_model(repo_id, filename)
        # Re-enable button when download completes (sig_download_complete → main_app →
        # refresh_models, which is enough; but we also re-enable via _on_rvc_models).

    def on_download_done(self) -> None:
        """Called by main_app after download completes to re-enable the download button."""
        if self._hf_file_combo.count() and self._hf_file_combo.isEnabled():
            self._hf_download_btn.setEnabled(True)

    def _save_hf_token(self) -> None:
        token = self._hf_token_input.text().strip()
        self._hf_token_save_btn.setEnabled(False)
        DbusWrapper.set_hf_token(token, self.sig_hf_token_saved)

    def _on_hf_token_loaded(self, token) -> None:
        if isinstance(token, str) and token:
            self._hf_token_input.setText(token)

    def _on_hf_token_saved(self, success) -> None:
        self._hf_token_save_btn.setEnabled(True)
        if success:
            self._hf_token_hint.setText(_T('ui', 'vc_rvc_hf_token_saved'))
            QTimer.singleShot(3000, lambda: self._hf_token_hint.setText(_T('ui', 'vc_rvc_hf_token_hint')))

    def _open_onnxruntime_install_dialog(self) -> None:
        dialog = OnnxRuntimeInstallDialog(self)
        dialog.exec()
        # The user may have installed it and clicked "Verify" inside the
        # dialog, or installed it and just closed the dialog — either way,
        # re-check capabilities so the panel updates without a manual retry.
        DbusWrapper.request_vc_capabilities(self.sig_vc_capabilities)

    def _download_base_models(self) -> None:
        reply = QMessageBox.question(
            self,
            _T('ui', 'vc_rvc_base_consent_title'),
            _T('ui', 'vc_rvc_base_consent_msg'),
            QMessageBox.StandardButton.Ok | QMessageBox.StandardButton.Cancel,
        )
        if reply != QMessageBox.StandardButton.Ok:
            return
        self._rvc_base_download_btn.setEnabled(False)
        DbusWrapper.download_base_models()

    def on_base_model_progress(self, message: str) -> None:
        self._rvc_base_download_btn.setEnabled(False)

    def on_base_model_complete(self, success: bool, _message: str) -> None:
        if success:
            DbusWrapper.request_vc_capabilities(self.sig_vc_capabilities)
        else:
            self._rvc_base_download_btn.setEnabled(True)

    def _retry_check(self) -> None:
        DbusWrapper.request_vc_capabilities(self.sig_vc_capabilities)
