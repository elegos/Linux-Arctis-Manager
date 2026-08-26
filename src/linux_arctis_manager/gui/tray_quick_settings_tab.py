from __future__ import annotations

import logging

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (QComboBox, QFrame, QHBoxLayout, QLabel,
                               QPushButton, QScrollArea, QSizePolicy,
                               QSlider, QVBoxLayout, QWidget)

from linux_arctis_manager.config import SettingType, ConfigSetting
from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.qt_widgets.q_toggle import QToggle
from linux_arctis_manager.gui.tray_quick_settings_editor import (
    QQuickSettingsEditor, load_config,
)
from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('tray_quick_settings_tab')
_T = lambda s, k: I18n.translate(s, k)  # noqa: E731

_NC_PRESETS: list[tuple[str, str]] = [
    ('off',      'nc_preset_off'),
    ('light',    'nc_preset_light'),
    ('standard', 'nc_preset_standard'),
    ('studio',   'nc_preset_studio'),
]


class QTrayQuickSettingsTab(QWidget):
    sig_list_received = Signal(object)

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)

        self._pid: int | None = None
        self._settings_config: dict[str, ConfigSetting] = {}
        self._current_values: dict[str, object] = {}
        self._nc_preset: str = 'off'
        self._option_lists: dict[str, list[dict]] = {}
        self._enabled_items: list[str] = []
        self._setting_widgets: dict[str, QWidget] = {}
        self._active_editor: QQuickSettingsEditor | None = None

        self.setAttribute(Qt.WidgetAttribute.WA_NoSystemBackground)
        self.setStyleSheet('QTrayQuickSettingsTab { background: transparent; }')

        root = QVBoxLayout()
        root.setContentsMargins(8, 8, 8, 8)
        root.setSpacing(0)
        self.setLayout(root)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QScrollArea.Shape.NoFrame)
        scroll.setStyleSheet('QScrollArea { background: transparent; border: none; }')
        scroll.viewport().setStyleSheet('background: transparent;')
        self._scroll_content = QWidget()
        self._scroll_content.setStyleSheet('background: transparent;')
        self._items_layout = QVBoxLayout()
        self._items_layout.setContentsMargins(0, 0, 0, 0)
        self._items_layout.setAlignment(Qt.AlignmentFlag.AlignTop)
        self._items_layout.setSpacing(0)
        self._scroll_content.setLayout(self._items_layout)
        scroll.setWidget(self._scroll_content)
        root.addWidget(scroll)

        self._placeholder = QLabel(_T('ui', 'quick_settings_empty'))
        self._placeholder.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self._placeholder.setWordWrap(True)
        self._placeholder.setStyleSheet('color: gray; padding: 12px;')
        root.addWidget(self._placeholder)

        bar = QHBoxLayout()
        bar.addStretch()
        self._edit_btn = QPushButton(_T('ui', 'quick_settings_edit'))
        self._edit_btn.setFixedWidth(80)
        self._edit_btn.clicked.connect(self._open_editor)
        bar.addWidget(self._edit_btn)
        root.addLayout(bar)

        self.sig_list_received.connect(self._on_list_received)

    # ── Public API ─────────────────────────────────────────────────────────────

    def set_device(self, pid: int) -> None:
        self._pid = pid
        items = load_config(pid)
        if not items:
            items = load_config(0)  # migrate from anonymous pid=0 config
        self._enabled_items = items
        self._rebuild()

    def update_settings(self, settings: dict) -> None:
        cfg = settings.get('settings_config', {})
        new_config = {name: ConfigSetting(name=name, **kwargs) for name, kwargs in cfg.items()}
        old_keys = set(self._settings_config.keys())
        new_keys = set(new_config.keys())
        self._settings_config = new_config

        for sid in self._enabled_items:
            self._ensure_options(sid)

        # Rebuild only when the config structure changes (new device / first load).
        # On value-only changes, _update_live() via update_status() is enough.
        if old_keys != new_keys:
            self._rebuild()

    def update_status(self, status: dict) -> None:
        for _cat, fields in status.items():
            for name, obj in fields.items():
                self._current_values[name] = obj.get('value')
        self._update_live()

    def update_nc_preset(self, preset: str) -> None:
        self._nc_preset = preset
        self._update_live()

    # ── Editor ─────────────────────────────────────────────────────────────────

    def _open_editor(self) -> None:
        pid = self._pid if self._pid is not None else 0
        available = self._available_ids()
        self._active_editor = QQuickSettingsEditor(
            pid, available, self._enabled_items, parent=None
        )
        self._active_editor.saved.connect(self._on_config_saved)
        self._active_editor.finished.connect(self._on_editor_closed)
        panel = self._tray_panel()
        if panel:
            panel._suppress_hide = True  # type: ignore[attr-defined]
            panel.hide()
        self._active_editor.show()

    def _on_editor_closed(self) -> None:
        panel = self._tray_panel()
        if panel:
            panel._suppress_hide = False  # type: ignore[attr-defined]
        self._active_editor = None

    def _tray_panel(self):
        w = self.parent()
        while w is not None:
            if hasattr(w, '_suppress_hide'):
                return w
            w = w.parent() if hasattr(w, 'parent') else None
        return None

    # ── Available settings ─────────────────────────────────────────────────────

    def _available_ids(self) -> list[str]:
        ids = ['nc_preset']
        for sid in self._settings_config:
            if sid not in ids:
                ids.append(sid)
        return ids

    def _on_config_saved(self, items: list[str]) -> None:
        self._enabled_items = items
        for sid in items:
            self._ensure_options(sid)
        self._rebuild()

    def _ensure_options(self, sid: str) -> None:
        cfg = self._settings_config.get(sid)
        if cfg is None or cfg.type != SettingType.SELECT:
            return
        src = getattr(cfg, 'options_source', None)
        if src and src not in self._option_lists:
            DbusWrapper.request_list_options(src, self.sig_list_received)

    def _on_list_received(self, payload: dict) -> None:
        name = payload.get('name', '')
        lst = payload.get('list', [])
        if name and isinstance(lst, list):
            self._option_lists[name] = lst
        self._rebuild()

    # ── Rendering ──────────────────────────────────────────────────────────────

    def _rebuild(self) -> None:
        while self._items_layout.count():
            item = self._items_layout.takeAt(0)
            if w := item.widget():
                w.deleteLater()
        self._setting_widgets.clear()

        items = [i for i in self._enabled_items if self._is_renderable(i)]

        if not items:
            self._scroll_content.hide()
            self._placeholder.show()
        else:
            self._placeholder.hide()
            self._scroll_content.show()
            for idx, sid in enumerate(items):
                row = self._build_row(sid)
                if row:
                    if idx > 0:
                        sep = QFrame()
                        sep.setFrameShape(QFrame.Shape.HLine)
                        sep.setStyleSheet('color: #444;')
                        self._items_layout.addWidget(sep)
                    self._items_layout.addWidget(row)

    def _is_renderable(self, sid: str) -> bool:
        if sid == 'nc_preset':
            return True
        cfg = self._settings_config.get(sid)
        if cfg is None:
            return False
        if cfg.type == SettingType.SELECT:
            src = getattr(cfg, 'options_source', None)
            return src is None or src in self._option_lists
        return True

    def _build_row(self, sid: str) -> QWidget | None:
        ctrl = self._build_control(sid)
        if ctrl is None:
            return None

        row = QWidget()
        row.setStyleSheet('background: transparent;')
        lay = QVBoxLayout()
        lay.setContentsMargins(4, 6, 4, 4)
        lay.setSpacing(4)
        row.setLayout(lay)

        lbl = QLabel(self._label_for(sid))
        lbl.setStyleSheet('font-size: 11px; color: gray;')
        lay.addWidget(lbl)
        lay.addWidget(ctrl)

        self._setting_widgets[sid] = ctrl
        return row

    def _label_for(self, sid: str) -> str:
        if sid == 'nc_preset':
            return _T('ui', 'nc')
        label = _T('settings', sid)
        if label == sid:
            label = _T('status', sid)
        if label == sid:
            label = sid.replace('_', ' ').title()
        return label

    def _build_control(self, sid: str) -> QWidget | None:
        if sid == 'nc_preset':
            return self._build_nc_preset_combo()
        cfg = self._settings_config.get(sid)
        if cfg is None:
            return None
        raw_val = self._current_values.get(sid)
        if cfg.type == SettingType.TOGGLE:
            return self._build_toggle(sid, cfg, raw_val)
        if cfg.type == SettingType.SLIDER:
            return self._build_slider(sid, cfg, raw_val)
        if cfg.type == SettingType.DISCRETE_MAP:
            return self._build_discrete_map(sid, cfg, raw_val)
        if cfg.type == SettingType.SELECT:
            return self._build_select(sid, cfg, raw_val)
        return None

    # ── Control builders ───────────────────────────────────────────────────────

    def _build_nc_preset_combo(self) -> QWidget:
        combo = QComboBox()
        combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        for key, label_key in _NC_PRESETS:
            combo.addItem(_T('ui', label_key), userData=key)
            if key == self._nc_preset:
                combo.setCurrentIndex(combo.count() - 1)

        def _on_nc_change(_: int) -> None:
            preset = combo.currentData()
            self._nc_preset = preset
            DbusWrapper.set_nc_settings({'preset': preset})

        combo.currentIndexChanged.connect(_on_nc_change)
        return combo

    def _build_toggle(self, sid: str, cfg: ConfigSetting, raw_val: object) -> QWidget:
        values = getattr(cfg, 'values', {}) or {}
        on_value = values.get('on', True)
        checked = (raw_val == on_value) if raw_val is not None else False
        toggle = QToggle(parent=None)
        toggle.setChecked(checked)

        def _on_toggle(state: Qt.CheckState) -> None:
            checked = state == Qt.CheckState.Checked
            actual = on_value if checked else values.get('off', 0)
            self._current_values[sid] = actual
            DbusWrapper.change_setting(sid, actual)

        toggle.checkStateChanged.connect(_on_toggle)
        container = QWidget()
        h = QHBoxLayout()
        h.setContentsMargins(0, 0, 0, 0)
        container.setLayout(h)
        h.addWidget(toggle)
        h.addStretch()
        return container

    def _build_slider(self, sid: str, cfg: ConfigSetting, raw_val: object) -> QWidget:
        container = QWidget()
        h = QHBoxLayout()
        h.setContentsMargins(0, 0, 0, 0)
        container.setLayout(h)
        slider = QSlider(Qt.Orientation.Horizontal)
        slider.setMinimum(getattr(cfg, 'min', 0))
        slider.setMaximum(getattr(cfg, 'max', 100))
        slider.setSingleStep(getattr(cfg, 'step', 1))
        if raw_val is not None:
            try:
                slider.setValue(int(float(raw_val)))
            except (TypeError, ValueError):
                pass
        val_lbl = QLabel(str(raw_val) if raw_val is not None else '0')
        val_lbl.setFixedWidth(28)
        val_lbl.setAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
        slider.valueChanged.connect(lambda v: val_lbl.setText(str(v)))

        def _on_release() -> None:
            v = slider.value()
            self._current_values[sid] = v  # optimistic update
            DbusWrapper.change_setting(sid, v)

        slider.sliderReleased.connect(_on_release)
        h.addWidget(slider)
        h.addWidget(val_lbl)
        return container

    def _build_discrete_map(self, sid: str, cfg: ConfigSetting, raw_val: object) -> QWidget:
        combo = QComboBox()
        combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        mapping = cfg.get_kwargs().get('values_mapping', {}) or {}
        ordered_keys = sorted(mapping.keys(), key=lambda k: int(k))
        current_idx = 0
        for i, k in enumerate(ordered_keys):
            label = _T('settings_values', mapping[k])
            combo.addItem(label, userData=int(k))
            try:
                if raw_val is not None and int(k) == int(raw_val):
                    current_idx = i
            except (TypeError, ValueError):
                pass
        combo.blockSignals(True)
        combo.setCurrentIndex(current_idx)
        combo.blockSignals(False)

        def _on_change(index: int) -> None:
            try:
                v = int(ordered_keys[index])
                self._current_values[sid] = v  # optimistic update
                DbusWrapper.change_setting(sid, v)
            except (IndexError, ValueError):
                pass

        combo.currentIndexChanged.connect(_on_change)
        return combo

    def _build_select(self, sid: str, cfg: ConfigSetting, raw_val: object) -> QWidget:
        combo = QComboBox()
        combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        src = getattr(cfg, 'options_source', None)
        options = self._option_lists.get(src, []) if src else []
        current_idx = 0
        for i, opt in enumerate(options):
            opt_id = opt.get('id', '')
            opt_label = opt.get('name', opt.get('description', str(opt_id)))
            combo.addItem(opt_label, userData=opt_id)
            if opt_id == raw_val:
                current_idx = i
        combo.blockSignals(True)
        if options:
            combo.setCurrentIndex(current_idx)
        combo.blockSignals(False)

        def _on_change(_: int) -> None:
            v = combo.currentData()
            self._current_values[sid] = v  # optimistic update
            DbusWrapper.change_setting(sid, v)

        combo.currentIndexChanged.connect(_on_change)
        return combo

    # ── Live updates ──────────────────────────────────────────────────────────

    def _update_live(self) -> None:
        for sid, ctrl in self._setting_widgets.items():
            if sid == 'nc_preset':
                if isinstance(ctrl, QComboBox):
                    for i in range(ctrl.count()):
                        if ctrl.itemData(i) == self._nc_preset:
                            ctrl.blockSignals(True)
                            ctrl.setCurrentIndex(i)
                            ctrl.blockSignals(False)
                            break
                continue

            raw = self._current_values.get(sid)
            if raw is None:
                continue
            cfg = self._settings_config.get(sid)
            if cfg is None:
                continue

            if cfg.type == SettingType.TOGGLE:
                toggle = ctrl.findChild(QToggle)
                if toggle:
                    values = getattr(cfg, 'values', {}) or {}
                    on_value = values.get('on', True)
                    toggle.blockSignals(True)
                    toggle.setChecked(raw == on_value)
                    toggle.blockSignals(False)

            elif cfg.type == SettingType.SLIDER:
                slider = ctrl.findChild(QSlider)
                if slider:
                    slider.blockSignals(True)
                    try:
                        slider.setValue(int(float(raw)))
                    except (TypeError, ValueError):
                        pass
                    slider.blockSignals(False)

            elif cfg.type in (SettingType.DISCRETE_MAP, SettingType.SELECT):
                if isinstance(ctrl, QComboBox):
                    for i in range(ctrl.count()):
                        item_data = ctrl.itemData(i)
                        match = (item_data == raw)
                        if not match:
                            try:
                                match = int(item_data) == int(raw)
                            except (TypeError, ValueError):
                                pass
                        if match:
                            ctrl.blockSignals(True)
                            ctrl.setCurrentIndex(i)
                            ctrl.blockSignals(False)
                            break
