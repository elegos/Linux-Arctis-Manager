from __future__ import annotations

import logging
from collections.abc import Callable

from PySide6.QtCore import Qt, QTimer, Signal
from PySide6.QtWidgets import (
    QComboBox,
    QFrame,
    QGroupBox,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QScrollArea,
    QSizePolicy,
    QSlider,
    QVBoxLayout,
    QWidget,
)

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.qt_widgets.q_checkable_button_group import (
    QCheckableButtonGroup,
)
from linux_arctis_manager.gui.qt_widgets.q_dual_state import QDualState
from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('QNCWidget')

_PRESET_OFF      = 0
_PRESET_LIGHT    = 1
_PRESET_STANDARD = 2
_PRESET_STUDIO   = 3
_PRESET_CUSTOM   = 4

_PRESET_KEYS = ('off', 'light', 'standard', 'studio', 'custom')

_PRESET_STAGES: dict[int, dict[str, bool]] = {
    _PRESET_OFF:      {'hpf': False, 'gate': False, 'comp': False},
    _PRESET_LIGHT:    {'hpf': True,  'gate': False, 'comp': False},
    _PRESET_STANDARD: {'hpf': True,  'gate': True,  'comp': False},
    _PRESET_STUDIO:   {'hpf': True,  'gate': True,  'comp': True},
}

_GATE_DEFAULTS = {'threshold': -42, 'reduction': -72, 'attack': 2, 'release': 450}
_COMP_DEFAULTS = {'threshold': -18, 'ratio': 18, 'makeup': 4}


def _slider_row(
    label: str, minimum: int, maximum: int, default: int,
    fmt_fn: Callable[[int], str], step: int = 1,
) -> tuple[QHBoxLayout, QSlider, QLabel]:
    row = QHBoxLayout()
    lbl = QLabel(label)
    lbl.setFixedWidth(100)
    row.addWidget(lbl)
    slider = QSlider(Qt.Orientation.Horizontal)
    slider.setMinimum(minimum)
    slider.setMaximum(maximum)
    slider.setSingleStep(step)
    slider.setValue(default)
    slider.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
    row.addWidget(slider)
    val_lbl = QLabel(fmt_fn(default))
    val_lbl.setFixedWidth(70)
    val_lbl.setAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
    row.addWidget(val_lbl)
    return row, slider, val_lbl


class QNCWidget(QWidget):
    sig_nc_capabilities  = Signal(object)
    sig_nc_settings      = Signal(object)
    _sig_sources_loaded  = Signal(object)   # internal: GetListOptions("pulse_audio_sources") reply

    def __init__(self, parent: QWidget, show_title: bool = True) -> None:
        super().__init__(parent)

        self._sources: list[dict] = []
        self._rnnoise_available = True
        self._swh_available = True
        self._loading = False   # suppress auto-apply while loading saved settings

        self.sig_nc_capabilities.connect(self._on_nc_capabilities)
        self.sig_nc_settings.connect(self._on_nc_settings)
        self._sig_sources_loaded.connect(self._on_sources_loaded)

        # Debounce timer: sliders fire on every tick; wait 500ms of idle before applying.
        self._apply_timer = QTimer(self)
        self._apply_timer.setSingleShot(True)
        self._apply_timer.setInterval(500)
        self._apply_timer.timeout.connect(self._apply)

        outer = QVBoxLayout()
        outer.setContentsMargins(0, 0, 0, 4)
        self.setLayout(outer)

        if show_title:
            title = QLabel(I18n.translate('ui', 'nc_title'))
            font = title.font()
            font.setBold(True)
            font.setPointSize(16)
            title.setFont(font)
            outer.addWidget(title)

        # ── RNNoise unavailable banner ─────────────────────────────────
        self._rnnoise_frame = QFrame()
        self._rnnoise_frame.setFrameShape(QFrame.Shape.StyledPanel)
        rf_layout = QVBoxLayout()
        rf_layout.setContentsMargins(10, 8, 10, 8)
        rf_layout.setSpacing(6)
        self._rnnoise_frame.setLayout(rf_layout)
        self._rnnoise_warn_label = QLabel()
        self._rnnoise_warn_label.setWordWrap(True)
        self._rnnoise_warn_label.setTextInteractionFlags(
            Qt.TextInteractionFlag.TextSelectableByMouse
            | Qt.TextInteractionFlag.TextSelectableByKeyboard
        )
        rf_layout.addWidget(self._rnnoise_warn_label)
        rf_btn_row = QHBoxLayout()
        self._rnnoise_retry_btn = QPushButton(I18n.translate('ui', 'nc_retry'))
        self._rnnoise_retry_btn.clicked.connect(self._retry_check)
        rf_btn_row.addWidget(self._rnnoise_retry_btn)
        rf_btn_row.addStretch()
        rf_layout.addLayout(rf_btn_row)
        self._rnnoise_frame.setVisible(False)
        outer.addWidget(self._rnnoise_frame)

        # ── Scroll area ────────────────────────────────────────────────
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        content = QWidget()
        cl = QVBoxLayout()
        cl.setAlignment(Qt.AlignmentFlag.AlignTop)
        content.setLayout(cl)
        scroll.setWidget(content)
        outer.addWidget(scroll, 1)
        self._content_widget = content

        # Input device
        dev_row = QHBoxLayout()
        dev_lbl = QLabel(I18n.translate('ui', 'nc_input_device'))
        dev_lbl.setFixedWidth(100)
        dev_row.addWidget(dev_lbl)
        self._source_combo = QComboBox()
        self._source_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self._source_combo.currentIndexChanged.connect(lambda _: self._apply())
        dev_row.addWidget(self._source_combo)
        cl.addLayout(dev_row)

        # Preset bar
        preset_row = QHBoxLayout()
        preset_lbl = QLabel(I18n.translate('ui', 'nc_preset'))
        preset_lbl.setFixedWidth(100)
        preset_row.addWidget(preset_lbl)
        self._preset_group = QCheckableButtonGroup()
        for idx, key in enumerate(_PRESET_KEYS):
            self._preset_group.addButton(
                value=idx,
                label=f'nc_preset_{key}',
                selected=(idx == _PRESET_OFF),
                i18n_section='ui',
            )
        self._preset_group.new_value.connect(self._on_preset_changed)
        self._preset_group.new_value.connect(lambda _: self._apply())
        preset_row.addWidget(self._preset_group)
        preset_row.addStretch()
        cl.addLayout(preset_row)

        # Preset description hint
        self._preset_hint = QLabel()
        self._preset_hint.setWordWrap(True)
        font = self._preset_hint.font()
        font.setItalic(True)
        self._preset_hint.setFont(font)
        cl.addWidget(self._preset_hint)

        # ── SWH unavailable banner ─────────────────────────────────────
        self._swh_frame = QFrame()
        self._swh_frame.setFrameShape(QFrame.Shape.StyledPanel)
        swh_layout = QVBoxLayout()
        swh_layout.setContentsMargins(10, 8, 10, 8)
        swh_layout.setSpacing(4)
        self._swh_frame.setLayout(swh_layout)
        self._swh_warn_label = QLabel()
        self._swh_warn_label.setWordWrap(True)
        self._swh_warn_label.setTextInteractionFlags(
            Qt.TextInteractionFlag.TextSelectableByMouse
            | Qt.TextInteractionFlag.TextSelectableByKeyboard
        )
        swh_layout.addWidget(self._swh_warn_label)
        swh_retry_row = QHBoxLayout()
        swh_retry_btn = QPushButton(I18n.translate('ui', 'nc_retry'))
        swh_retry_btn.clicked.connect(self._retry_check)
        swh_retry_row.addWidget(swh_retry_btn)
        swh_retry_row.addStretch()
        swh_layout.addLayout(swh_retry_row)
        self._swh_frame.setVisible(False)
        cl.addWidget(self._swh_frame)

        # ── HPF section ───────────────────────────────────────────────
        self._hpf_box = QGroupBox(I18n.translate('ui', 'nc_hpf_section'))
        hpf_layout = QVBoxLayout()
        self._hpf_box.setLayout(hpf_layout)
        hpf_enable_row = QHBoxLayout()
        hpf_enable_row.addWidget(QLabel(I18n.translate('ui', 'nc_stage_enable')))
        self._hpf_toggle = QDualState(
            off_text=I18n.translate('settings_values', 'off'),
            on_text=I18n.translate('settings_values', 'on'),
            init_state='left',
        )
        self._hpf_toggle.checkStateChanged.connect(lambda _: self._apply())
        hpf_enable_row.addWidget(self._hpf_toggle)
        hpf_enable_row.addStretch()
        hpf_layout.addLayout(hpf_enable_row)
        self._hpf_box.setVisible(False)
        cl.addWidget(self._hpf_box)

        # ── Gate section ──────────────────────────────────────────────
        self._gate_box = QGroupBox(I18n.translate('ui', 'nc_gate_section'))
        gate_layout = QVBoxLayout()
        self._gate_box.setLayout(gate_layout)
        gate_enable_row = QHBoxLayout()
        gate_enable_row.addWidget(QLabel(I18n.translate('ui', 'nc_stage_enable')))
        self._gate_toggle = QDualState(
            off_text=I18n.translate('settings_values', 'off'),
            on_text=I18n.translate('settings_values', 'on'),
            init_state='left',
        )
        self._gate_toggle.checkStateChanged.connect(lambda _: self._apply())
        gate_enable_row.addWidget(self._gate_toggle)
        gate_enable_row.addStretch()
        gate_layout.addLayout(gate_enable_row)

        self._gate_advanced = QWidget()
        ga = QVBoxLayout()
        ga.setContentsMargins(0, 4, 0, 0)
        self._gate_advanced.setLayout(ga)

        row, self._gate_thr_sl, self._gate_thr_lbl = _slider_row(
            I18n.translate('ui', 'nc_gate_threshold'), -60, 0,
            _GATE_DEFAULTS['threshold'], lambda v: f'{v} dB')
        self._gate_thr_sl.valueChanged.connect(
            lambda v: (self._gate_thr_lbl.setText(f'{v} dB'), self._apply_timer.start()))
        ga.addLayout(row)

        row, self._gate_red_sl, self._gate_red_lbl = _slider_row(
            I18n.translate('ui', 'nc_gate_reduction'), -90, 0,
            _GATE_DEFAULTS['reduction'], lambda v: f'{v} dB')
        self._gate_red_sl.valueChanged.connect(
            lambda v: (self._gate_red_lbl.setText(f'{v} dB'), self._apply_timer.start()))
        ga.addLayout(row)

        row, self._gate_atk_sl, self._gate_atk_lbl = _slider_row(
            I18n.translate('ui', 'nc_gate_attack'), 1, 100,
            _GATE_DEFAULTS['attack'], lambda v: f'{v} ms')
        self._gate_atk_sl.valueChanged.connect(
            lambda v: (self._gate_atk_lbl.setText(f'{v} ms'), self._apply_timer.start()))
        ga.addLayout(row)

        row, self._gate_rel_sl, self._gate_rel_lbl = _slider_row(
            I18n.translate('ui', 'nc_gate_release'), 50, 2000,
            _GATE_DEFAULTS['release'], lambda v: f'{v} ms', step=10)
        self._gate_rel_sl.valueChanged.connect(
            lambda v: (self._gate_rel_lbl.setText(f'{v} ms'), self._apply_timer.start()))
        ga.addLayout(row)

        gate_reset_row = QHBoxLayout()
        gate_reset_btn = QPushButton(I18n.translate('ui', 'nc_reset_defaults'))
        gate_reset_btn.clicked.connect(self._reset_gate)
        gate_reset_row.addWidget(gate_reset_btn)
        gate_reset_row.addStretch()
        ga.addLayout(gate_reset_row)

        gate_layout.addWidget(self._gate_advanced)
        self._gate_box.setVisible(False)
        cl.addWidget(self._gate_box)

        # ── Compressor section ────────────────────────────────────────
        self._comp_box = QGroupBox(I18n.translate('ui', 'nc_comp_section'))
        comp_layout = QVBoxLayout()
        self._comp_box.setLayout(comp_layout)
        comp_enable_row = QHBoxLayout()
        comp_enable_row.addWidget(QLabel(I18n.translate('ui', 'nc_stage_enable')))
        self._comp_toggle = QDualState(
            off_text=I18n.translate('settings_values', 'off'),
            on_text=I18n.translate('settings_values', 'on'),
            init_state='left',
        )
        self._comp_toggle.checkStateChanged.connect(lambda _: self._apply())
        comp_enable_row.addWidget(self._comp_toggle)
        comp_enable_row.addStretch()
        comp_layout.addLayout(comp_enable_row)

        self._comp_advanced = QWidget()
        ca = QVBoxLayout()
        ca.setContentsMargins(0, 4, 0, 0)
        self._comp_advanced.setLayout(ca)

        row, self._comp_thr_sl, self._comp_thr_lbl = _slider_row(
            I18n.translate('ui', 'nc_comp_threshold'), -40, 0,
            _COMP_DEFAULTS['threshold'], lambda v: f'{v} dB')
        self._comp_thr_sl.valueChanged.connect(
            lambda v: (self._comp_thr_lbl.setText(f'{v} dB'), self._apply_timer.start()))
        ca.addLayout(row)

        row, self._comp_rat_sl, self._comp_rat_lbl = _slider_row(
            I18n.translate('ui', 'nc_comp_ratio'), 10, 100,
            _COMP_DEFAULTS['ratio'], lambda v: f'{v / 10:.1f}:1')
        self._comp_rat_sl.valueChanged.connect(
            lambda v: (self._comp_rat_lbl.setText(f'{v / 10:.1f}:1'), self._apply_timer.start()))
        ca.addLayout(row)

        row, self._comp_mkp_sl, self._comp_mkp_lbl = _slider_row(
            I18n.translate('ui', 'nc_comp_makeup'), 0, 12,
            _COMP_DEFAULTS['makeup'], lambda v: f'+{v} dB')
        self._comp_mkp_sl.valueChanged.connect(
            lambda v: (self._comp_mkp_lbl.setText(f'+{v} dB'), self._apply_timer.start()))
        ca.addLayout(row)

        comp_reset_row = QHBoxLayout()
        comp_reset_btn = QPushButton(I18n.translate('ui', 'nc_reset_defaults'))
        comp_reset_btn.clicked.connect(self._reset_comp)
        comp_reset_row.addWidget(comp_reset_btn)
        comp_reset_row.addStretch()
        ca.addLayout(comp_reset_row)

        comp_layout.addWidget(self._comp_advanced)
        self._comp_box.setVisible(False)
        cl.addWidget(self._comp_box)

        self._apply_preset_ui(_PRESET_OFF)

    def showEvent(self, event) -> None:
        super().showEvent(event)
        self.refresh()

    def refresh(self) -> None:
        DbusWrapper.request_list_options('pulse_audio_sources', self._sig_sources_loaded)
        DbusWrapper.request_nc_capabilities(self.sig_nc_capabilities)
        DbusWrapper.request_nc_settings(self.sig_nc_settings)

    # ── Source loading (daemon GetListOptions("pulse_audio_sources")) ──

    def _on_sources_loaded(self, data: dict) -> None:
        sources: list[dict] = data.get('list', [])
        default_id = next((s['id'] for s in sources if s.get('is_default')), '')
        self._populate_sources(sources, prefer_id=default_id)

    # ── D-Bus handlers (daemon NC interface, stubbed until daemon supports NC) ──

    def _on_nc_capabilities(self, caps: dict) -> None:
        rnnoise_ok = bool(caps.get('rnnoise_available', False))
        swh_ok = bool(caps.get('swh_available', False))
        self._rnnoise_available = rnnoise_ok
        self._swh_available = swh_ok

        self._rnnoise_frame.setVisible(not rnnoise_ok)
        if not rnnoise_ok:
            self._rnnoise_warn_label.setText('\n'.join([
                I18n.translate('ui', 'nc_rnnoise_unavailable'),
                '',
                I18n.translate('ui', 'nc_plugin_install_hint'),
                '  ' + I18n.translate('ui', 'nc_rnnoise_install_fedora'),
                '  ' + I18n.translate('ui', 'nc_rnnoise_install_debian'),
                '  ' + I18n.translate('ui', 'nc_rnnoise_install_arch'),
            ]))
            logger.warning('RNNoise LADSPA plugin not found — NC controls disabled')
        self._content_widget.setEnabled(rnnoise_ok)

        self._swh_frame.setVisible(not swh_ok)
        if not swh_ok:
            self._swh_warn_label.setText('\n'.join([
                I18n.translate('ui', 'nc_swh_unavailable'),
                '',
                I18n.translate('ui', 'nc_plugin_install_hint'),
                '  ' + I18n.translate('ui', 'nc_swh_install_fedora'),
                '  ' + I18n.translate('ui', 'nc_swh_install_debian'),
                '  ' + I18n.translate('ui', 'nc_swh_install_arch'),
            ]))
            logger.warning('swh-plugins not found — HPF, gate, compressor disabled')
        self._hpf_box.setEnabled(swh_ok)
        self._gate_box.setEnabled(swh_ok)
        self._comp_box.setEnabled(swh_ok)

    def _on_nc_settings(self, settings: dict) -> None:
        self._loading = True
        source_id = settings.get('source_id', '')
        if source_id:
            idx = next((i for i, s in enumerate(self._sources) if s['id'] == source_id), None)
            if idx is not None:
                self._source_combo.blockSignals(True)
                self._source_combo.setCurrentIndex(idx)
                self._source_combo.blockSignals(False)

        g = settings.get('gate', {})
        self._gate_thr_sl.setValue(g.get('threshold', _GATE_DEFAULTS['threshold']))
        self._gate_red_sl.setValue(g.get('reduction', _GATE_DEFAULTS['reduction']))
        self._gate_atk_sl.setValue(g.get('attack',    _GATE_DEFAULTS['attack']))
        self._gate_rel_sl.setValue(g.get('release',   _GATE_DEFAULTS['release']))

        c = settings.get('compressor', {})
        self._comp_thr_sl.setValue(c.get('threshold', _COMP_DEFAULTS['threshold']))
        self._comp_rat_sl.setValue(c.get('ratio',     _COMP_DEFAULTS['ratio']))
        self._comp_mkp_sl.setValue(c.get('makeup',    _COMP_DEFAULTS['makeup']))

        preset_key = settings.get('preset', 'off')
        preset_idx = _PRESET_KEYS.index(preset_key) if preset_key in _PRESET_KEYS else _PRESET_OFF
        for btn in self._preset_group.buttons:
            btn.setChecked(btn.property('value') == preset_idx)
        self._apply_preset_ui(preset_idx)
        self._loading = False

    # ── Source combo ──────────────────────────────────────────────────

    def _populate_sources(self, sources: list[dict], prefer_id: str = '') -> None:
        self._sources = sources
        self._source_combo.blockSignals(True)
        self._source_combo.clear()
        for src in sources:
            self._source_combo.addItem(src['name'], src['id'])
        idx = next((i for i, s in enumerate(sources) if s['id'] == prefer_id), 0)
        self._source_combo.setCurrentIndex(idx)
        self._source_combo.blockSignals(False)

    # ── Preset logic ──────────────────────────────────────────────────

    def _on_preset_changed(self, idx: int) -> None:
        self._apply_preset_ui(idx)

    def _apply_preset_ui(self, preset_idx: int) -> None:
        is_custom = preset_idx == _PRESET_CUSTOM

        # Sections only visible in Custom mode
        self._hpf_box.setVisible(is_custom)
        self._gate_box.setVisible(is_custom)
        self._comp_box.setVisible(is_custom)

        # Advanced params only in Custom
        self._gate_advanced.setVisible(is_custom)
        self._comp_advanced.setVisible(is_custom)

        # For named presets, set toggle states — let signals fire naturally so
        # the animation runs and the dot moves to the correct position.
        if preset_idx in _PRESET_STAGES:
            stages = _PRESET_STAGES[preset_idx]
            self._hpf_toggle.toggle.setChecked(stages['hpf'])
            self._gate_toggle.toggle.setChecked(stages['gate'])
            self._comp_toggle.toggle.setChecked(stages['comp'])

        # Toggles editable only in Custom
        self._hpf_toggle.setEnabled(is_custom)
        self._gate_toggle.setEnabled(is_custom)
        self._comp_toggle.setEnabled(is_custom)

        hints = {
            _PRESET_OFF:      'nc_hint_off',
            _PRESET_LIGHT:    'nc_hint_light',
            _PRESET_STANDARD: 'nc_hint_standard',
            _PRESET_STUDIO:   'nc_hint_studio',
            _PRESET_CUSTOM:   'nc_hint_custom',
        }
        self._preset_hint.setText(I18n.translate('ui', hints[preset_idx]))

    # ── Reset helpers ─────────────────────────────────────────────────

    def _reset_gate(self) -> None:
        self._gate_thr_sl.setValue(_GATE_DEFAULTS['threshold'])
        self._gate_red_sl.setValue(_GATE_DEFAULTS['reduction'])
        self._gate_atk_sl.setValue(_GATE_DEFAULTS['attack'])
        self._gate_rel_sl.setValue(_GATE_DEFAULTS['release'])

    def _reset_comp(self) -> None:
        self._comp_thr_sl.setValue(_COMP_DEFAULTS['threshold'])
        self._comp_rat_sl.setValue(_COMP_DEFAULTS['ratio'])
        self._comp_mkp_sl.setValue(_COMP_DEFAULTS['makeup'])

    # ── Apply ─────────────────────────────────────────────────────────

    def _apply(self) -> None:
        if self._loading or not self._rnnoise_available:
            return

        preset_idx = next(
            (btn.property('value') for btn in self._preset_group.buttons if btn.isChecked()),
            _PRESET_OFF,
        )
        DbusWrapper.set_nc_settings({
            'preset':      _PRESET_KEYS[preset_idx],
            'source_id':   self._source_combo.currentData() or '',
            'hpf_enabled': self._hpf_toggle.toggle.isChecked(),
            'gate': {
                'enabled':   self._gate_toggle.toggle.isChecked(),
                'threshold': self._gate_thr_sl.value(),
                'reduction': self._gate_red_sl.value(),
                'attack':    self._gate_atk_sl.value(),
                'release':   self._gate_rel_sl.value(),
            },
            'compressor': {
                'enabled':   self._comp_toggle.toggle.isChecked(),
                'threshold': self._comp_thr_sl.value(),
                'ratio':     self._comp_rat_sl.value(),
                'makeup':    self._comp_mkp_sl.value(),
            },
        })

    def _retry_check(self) -> None:
        DbusWrapper.request_nc_capabilities(self.sig_nc_capabilities)
