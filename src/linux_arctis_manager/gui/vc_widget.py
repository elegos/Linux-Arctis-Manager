from __future__ import annotations

import logging
import math
import threading

import pulsectl
from PySide6.QtCore import Qt, QTimer, Signal
from PySide6.QtWidgets import (QCheckBox, QComboBox, QDoubleSpinBox, QFrame,
                               QGroupBox, QHBoxLayout, QLabel, QPushButton,
                               QScrollArea, QSizePolicy, QSlider, QSpinBox,
                               QStackedWidget, QVBoxLayout, QWidget)

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
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
    sig_vc_capabilities = Signal(object)
    sig_vc_settings     = Signal(object)
    sig_rvc_models      = Signal(object)
    _sig_sources_loaded = Signal(object)

    def __init__(self, parent: QWidget, show_title: bool = True) -> None:
        super().__init__(parent)

        self._sources:  list[dict] = []
        self._loading   = False
        self._ladspa_caps: dict[str, bool] = {}
        self._rvc_caps:    dict = {}

        self.sig_vc_capabilities.connect(self._on_vc_capabilities)
        self.sig_vc_settings.connect(self._on_vc_settings)
        self.sig_rvc_models.connect(self._on_rvc_models)
        self._sig_sources_loaded.connect(self._on_sources_loaded)

        self._apply_timer = QTimer(self)
        self._apply_timer.setSingleShot(True)
        self._apply_timer.setInterval(500)
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

        # No-backend warning
        self._rvc_no_backend_frame = QFrame()
        self._rvc_no_backend_frame.setFrameShape(QFrame.Shape.StyledPanel)
        nb = QVBoxLayout()
        nb.setContentsMargins(10, 8, 10, 8)
        self._rvc_no_backend_frame.setLayout(nb)
        nb.addWidget(QLabel(_T('ui', 'vc_rvc_no_backend')))
        self._rvc_no_backend_frame.setVisible(False)
        cl.addWidget(self._rvc_no_backend_frame)

        # Model selector
        model_row = QHBoxLayout()
        model_lbl = QLabel(_T('ui', 'vc_rvc_model'))
        model_lbl.setFixedWidth(110)
        model_row.addWidget(model_lbl)
        self._rvc_model_combo = QComboBox()
        self._rvc_model_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self._rvc_model_combo.currentIndexChanged.connect(lambda _: self._apply())
        model_row.addWidget(self._rvc_model_combo)
        refresh_btn = QPushButton('↻')
        refresh_btn.setFixedWidth(30)
        refresh_btn.setToolTip('Refresh model list')
        refresh_btn.clicked.connect(lambda: DbusWrapper.request_rvc_models(self.sig_rvc_models))
        model_row.addWidget(refresh_btn)
        cl.addLayout(model_row)

        # No-models hint
        self._rvc_no_models_lbl = QLabel()
        self._rvc_no_models_lbl.setWordWrap(True)
        font = self._rvc_no_models_lbl.font()
        font.setItalic(True)
        self._rvc_no_models_lbl.setFont(font)
        self._rvc_no_models_lbl.setVisible(False)
        cl.addWidget(self._rvc_no_models_lbl)

        # Pitch offset
        row, self._rvc_pitch_sl, self._rvc_pitch_lbl = _slider_row(
            _T('ui', 'vc_rvc_pitch'), -12, 12, 0,
            lambda v: f'{v:+d} st' if v != 0 else '0 st')
        self._rvc_pitch_sl.valueChanged.connect(
            lambda v: (self._rvc_pitch_lbl.setText(f'{v:+d} st' if v != 0 else '0 st'),
                       self._apply_timer.start()))
        cl.addLayout(row)

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

        # RVC panel
        rvc_avail = bool(self._rvc_caps.get('available', False))
        backends  = self._rvc_caps.get('backends', [])
        self._rvc_no_backend_frame.setVisible(not rvc_avail)
        self._rvc_backend_lbl.setText(
            _T('ui', 'vc_rvc_backend') + ': ' + (', '.join(backends) if backends else '—')
        )
        self._rvc_model_combo.setEnabled(rvc_avail)

        models: list[dict] = self._rvc_caps.get('models', [])
        self._refresh_rvc_models(models, self._rvc_caps.get('models_folder', ''))

        sources: list[dict] = caps.get('sources', [])
        if sources and not self._sources:
            self._populate_sources(sources)

    def _on_rvc_models(self, models: list) -> None:
        folder = self._rvc_caps.get('models_folder', '')
        self._refresh_rvc_models(models, folder)

    def _refresh_rvc_models(self, models: list[dict], folder: str) -> None:
        current = self._rvc_model_combo.currentData()
        self._rvc_model_combo.blockSignals(True)
        self._rvc_model_combo.clear()
        for m in models:
            self._rvc_model_combo.addItem(m['name'], m['name'])
        if current:
            idx = self._rvc_model_combo.findData(current)
            if idx >= 0:
                self._rvc_model_combo.setCurrentIndex(idx)
        self._rvc_model_combo.blockSignals(False)

        no_models = not models
        self._rvc_no_models_lbl.setVisible(no_models)
        if no_models and folder:
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
        rvc_model = str(rv.get('model', ''))
        if rvc_model:
            idx = self._rvc_model_combo.findData(rvc_model)
            if idx >= 0:
                self._rvc_model_combo.setCurrentIndex(idx)
        pitch_off = round(float(rv.get('pitch_offset', 0.0)))
        self._rvc_pitch_sl.blockSignals(True)
        self._rvc_pitch_sl.setValue(pitch_off)
        self._rvc_pitch_sl.blockSignals(False)
        v = pitch_off
        self._rvc_pitch_lbl.setText(f'{v:+d} st' if v != 0 else '0 st')

        self._loading = False
        self._update_global_state()

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
                'model':        self._rvc_model_combo.currentData() or '',
                'pitch_offset': float(self._rvc_pitch_sl.value()),
            },
        })

    def _retry_check(self) -> None:
        DbusWrapper.request_vc_capabilities(self.sig_vc_capabilities)
