from __future__ import annotations

import logging

from PySide6.QtCore import Qt, QTimer, Signal
from PySide6.QtWidgets import (QComboBox, QDialog, QDialogButtonBox,
                               QFormLayout, QGroupBox, QHBoxLayout,
                               QInputDialog, QLabel, QLineEdit, QListWidget,
                               QMessageBox, QPushButton, QScrollArea,
                               QSizePolicy, QSlider, QVBoxLayout, QWidget)

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.qt_widgets.q_checkable_button_group import \
    QCheckableButtonGroup
from linux_arctis_manager.gui.qt_widgets.q_dual_state import QDualState
from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('QEQWidget')

_GAIN_MIN = -120   # -12.0 dB (slider units = tenths of dB)
_GAIN_MAX = 120    # +12.0 dB


def _fmt_gain(tenths: int) -> str:
    return f'{tenths / 10:+.1f} dB'


def _fmt_freq(hz: int) -> str:
    return f'{hz // 1000}k' if hz >= 1000 else f'{hz}'


class QBandRow(QWidget):
    def __init__(self, frequency: int, gain: float, parent: QWidget | None = None):
        super().__init__(parent)
        self.frequency = frequency
        layout = QHBoxLayout()
        layout.setContentsMargins(0, 2, 0, 2)
        self.setLayout(layout)

        freq_label = QLabel(f'{_fmt_freq(frequency)} Hz')
        freq_label.setFixedWidth(55)
        layout.addWidget(freq_label)

        self.slider = QSlider(Qt.Orientation.Horizontal)
        self.slider.setRange(_GAIN_MIN, _GAIN_MAX)
        self.slider.setValue(int(gain * 10))
        self.slider.setTickInterval(10)
        self.slider.setTickPosition(QSlider.TickPosition.TicksBelow)
        layout.addWidget(self.slider)

        self._val_label = QLabel(_fmt_gain(int(gain * 10)))
        self._val_label.setFixedWidth(65)
        layout.addWidget(self._val_label)

        self.slider.valueChanged.connect(lambda v: self._val_label.setText(_fmt_gain(v)))

    @property
    def gain(self) -> float:
        return self.slider.value() / 10.0


class QAddOverrideDialog(QDialog):
    def __init__(self, preset_names: list[str], steam_games: list[dict],
                 parent: QWidget | None = None):
        super().__init__(parent)
        self.setWindowTitle(I18n.translate('ui', 'eq_add_override'))
        self._steam_games = steam_games

        layout = QFormLayout()
        self.setLayout(layout)

        self._type_combo = QComboBox()
        self._type_combo.addItems([
            I18n.translate('ui', 'eq_match_stream'),
            I18n.translate('ui', 'eq_match_executable'),
            I18n.translate('ui', 'eq_match_steam'),
        ])
        self._type_combo.currentIndexChanged.connect(self._on_type_changed)
        layout.addRow(I18n.translate('ui', 'eq_match_by'), self._type_combo)

        self._value_input = QLineEdit()
        self._value_label = QLabel(I18n.translate('ui', 'eq_match_stream'))
        layout.addRow(self._value_label, self._value_input)

        self._game_combo = QComboBox()
        self._game_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        for g in steam_games:
            self._game_combo.addItem(g['name'], g['app_id'])
        self._game_label = QLabel(I18n.translate('ui', 'eq_match_steam'))
        layout.addRow(self._game_label, self._game_combo)
        self._game_label.setVisible(False)
        self._game_combo.setVisible(False)

        self._preset_combo = QComboBox()
        self._preset_combo.addItem(I18n.translate('ui', 'eq_flat'))
        for name in preset_names:
            self._preset_combo.addItem(name)
        layout.addRow(I18n.translate('ui', 'eq_preset'), self._preset_combo)

        self._channel_combo = QComboBox()
        self._channel_combo.addItems(['Media', 'Chat'])
        layout.addRow(I18n.translate('ui', 'eq_channel'), self._channel_combo)

        buttons = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel
        )
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout.addRow(buttons)

    def _on_type_changed(self, idx: int) -> None:
        is_steam = idx == 2
        self._value_label.setVisible(not is_steam)
        self._value_input.setVisible(not is_steam)
        self._game_label.setVisible(is_steam)
        self._game_combo.setVisible(is_steam)
        if idx == 0:
            self._value_label.setText(I18n.translate('ui', 'eq_match_stream'))
        elif idx == 1:
            self._value_label.setText(I18n.translate('ui', 'eq_match_executable'))

    def get_override(self) -> dict:
        type_idx = self._type_combo.currentIndex()
        matcher_types = ['stream', 'executable', 'steam']
        matcher_type = matcher_types[type_idx]
        game_idx = self._game_combo.currentIndex()
        preset_txt = self._preset_combo.currentText()
        flat_label = I18n.translate('ui', 'eq_flat')
        return {
            'matcher_type': matcher_type,
            'value': self._value_input.text().strip() if matcher_type != 'steam' else '',
            'steam_app_id': (self._steam_games[game_idx]['app_id']
                             if matcher_type == 'steam' and game_idx >= 0 and game_idx < len(self._steam_games)
                             else None),
            'steam_game_name': (self._steam_games[game_idx]['name']
                                if matcher_type == 'steam' and game_idx >= 0 and game_idx < len(self._steam_games)
                                else ''),
            'preset_name': '' if preset_txt == flat_label else preset_txt,
            'channel': 'chat' if self._channel_combo.currentIndex() == 1 else 'media',
        }


class QChannelSection(QGroupBox):
    preset_saved = Signal(object)    # dict
    preset_deleted = Signal(str)

    def __init__(self, channel: str, parent: QWidget | None = None):
        title = I18n.translate('ui', f'eq_{channel}_channel')
        super().__init__(title, parent)
        self._channel = channel
        self._presets: dict[str, dict] = {}
        self._band_rows: list[QBandRow] = []
        self._pending_select_name: str | None = None

        layout = QVBoxLayout()
        self.setLayout(layout)

        # Enable
        enable_row = QHBoxLayout()
        enable_row.addWidget(QLabel(I18n.translate('ui', 'eq_enable')))
        self._enable = QDualState(
            off_text=I18n.translate('settings_values', 'off'),
            on_text=I18n.translate('settings_values', 'on'),
            init_state='left',
        )
        enable_row.addWidget(self._enable)
        enable_row.addStretch()
        layout.addLayout(enable_row)

        # Mode
        mode_row = QHBoxLayout()
        mode_row.addWidget(QLabel(I18n.translate('ui', 'eq_mode')))
        self._mode_group = QCheckableButtonGroup()
        self._mode_group.addButton(0, 'simple', True, 'settings_values')
        self._mode_group.addButton(1, 'advanced', False, 'settings_values')
        self._mode_group.new_value.connect(self._on_mode_changed)
        mode_row.addWidget(self._mode_group)
        mode_row.addStretch()
        layout.addLayout(mode_row)

        # Preset row
        preset_row = QHBoxLayout()
        preset_row.addWidget(QLabel(I18n.translate('ui', 'eq_preset')))
        self._preset_combo = QComboBox()
        self._preset_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self._preset_combo.currentIndexChanged.connect(self._on_preset_changed)
        preset_row.addWidget(self._preset_combo)

        new_btn = QPushButton(I18n.translate('ui', 'eq_new_preset'))
        new_btn.clicked.connect(self._new_preset)
        preset_row.addWidget(new_btn)

        save_btn = QPushButton(I18n.translate('ui', 'eq_save_preset'))
        save_btn.clicked.connect(self._save_preset)
        preset_row.addWidget(save_btn)

        self._delete_btn = QPushButton(I18n.translate('ui', 'eq_delete_preset'))
        self._delete_btn.clicked.connect(self._delete_preset)
        preset_row.addWidget(self._delete_btn)
        layout.addLayout(preset_row)

        # Band sliders (shown only when a named preset is selected)
        self._bands_container = QWidget()
        self._bands_layout = QVBoxLayout()
        self._bands_layout.setContentsMargins(0, 0, 0, 0)
        self._bands_container.setLayout(self._bands_layout)
        self._bands_container.setVisible(False)
        layout.addWidget(self._bands_container)

        # Initialise combo with just "Flat"
        self._rebuild_preset_combo(None)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def load_settings(self, settings: dict, presets: dict[str, dict]) -> None:
        self._presets = presets
        enabled = settings.get('enabled', False)
        mode = settings.get('mode', 'simple')
        preset_name = settings.get('preset_name')

        self._enable.toggle.blockSignals(True)
        self._enable.toggle.setChecked(enabled)
        self._enable.toggle.blockSignals(False)

        mode_val = 1 if mode == 'advanced' else 0
        for btn in self._mode_group.buttons:
            btn.setChecked(btn.property('value') == mode_val)

        self._rebuild_preset_combo(preset_name)

    def set_presets(self, presets: dict[str, dict]) -> None:
        self._presets = presets
        select_name = self._pending_select_name or self._current_preset_name()
        self._pending_select_name = None
        self._rebuild_preset_combo(select_name)

    def get_settings(self) -> dict:
        return {
            'enabled': self._enable.toggle.isChecked(),
            'mode': self._current_mode(),
            'preset_name': self._current_preset_name(),
        }

    # ------------------------------------------------------------------
    # Internal
    # ------------------------------------------------------------------

    def _current_mode(self) -> str:
        for btn in self._mode_group.buttons:
            if btn.isChecked():
                return 'advanced' if btn.property('value') == 1 else 'simple'
        return 'simple'

    def _current_preset_name(self) -> str | None:
        txt = self._preset_combo.currentText()
        return None if txt == I18n.translate('ui', 'eq_flat') else txt

    def _rebuild_preset_combo(self, select_name: str | None) -> None:
        self._preset_combo.blockSignals(True)
        self._preset_combo.clear()
        self._preset_combo.addItem(I18n.translate('ui', 'eq_flat'))
        for name in sorted(self._presets.keys()):
            self._preset_combo.addItem(name)
        idx = 0
        if select_name:
            found = self._preset_combo.findText(select_name)
            if found >= 0:
                idx = found
        self._preset_combo.setCurrentIndex(idx)
        self._preset_combo.blockSignals(False)
        self._load_bands_for_current_preset()

    def _load_bands_for_current_preset(self) -> None:
        name = self._current_preset_name()
        if name is not None and name in self._presets:
            bands = self._presets[name].get('bands', [])
            self._rebuild_band_rows(bands)
            self._bands_container.setVisible(True)
        else:
            self._rebuild_band_rows([])
            self._bands_container.setVisible(False)

    def _rebuild_band_rows(self, bands: list[dict]) -> None:
        while self._bands_layout.count():
            item = self._bands_layout.takeAt(0)
            if w := item.widget():
                w.deleteLater()
        self._band_rows = []
        for band in bands:
            row = QBandRow(int(band['frequency']), float(band.get('gain', 0.0)))
            self._bands_layout.addWidget(row)
            self._band_rows.append(row)

    def _current_bands_as_list(self) -> list[dict]:
        return [{'frequency': row.frequency, 'gain': row.gain} for row in self._band_rows]

    def _on_mode_changed(self, _value: int) -> None:
        self._load_bands_for_current_preset()

    def _on_preset_changed(self, _idx: int) -> None:
        self._load_bands_for_current_preset()

    def _new_preset(self) -> None:
        mode = self._current_mode()
        name, ok = QInputDialog.getText(
            self,
            I18n.translate('ui', 'eq_new_preset'),
            I18n.translate('ui', 'eq_preset_name_prompt'),
        )
        if not ok or not name.strip():
            return
        name = name.strip()
        from linux_arctis_manager.eq_preset import MBEQ_BAND_FREQUENCIES, SIMPLE_BAND_FREQUENCIES
        freqs = SIMPLE_BAND_FREQUENCIES if mode == 'simple' else MBEQ_BAND_FREQUENCIES
        flat_bands = [{'frequency': f, 'gain': 0.0} for f in freqs]
        self._pending_select_name = name
        self.preset_saved.emit({'name': name, 'mode': mode, 'description': '', 'bands': flat_bands})

    def _save_preset(self) -> None:
        name = self._current_preset_name()
        if name is None:
            QMessageBox.information(self, '', I18n.translate('ui', 'eq_select_preset_first'))
            return
        self.preset_saved.emit({
            'name': name,
            'mode': self._current_mode(),
            'description': self._presets.get(name, {}).get('description', ''),
            'bands': self._current_bands_as_list(),
        })

    def _delete_preset(self) -> None:
        name = self._current_preset_name()
        if name is None:
            return
        reply = QMessageBox.question(
            self,
            I18n.translate('ui', 'eq_delete_preset'),
            f"{I18n.translate('ui', 'eq_delete_confirm')} '{name}'?",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
        )
        if reply == QMessageBox.StandardButton.Yes:
            self.preset_deleted.emit(name)


class QEQWidget(QWidget):
    sig_eq_settings = Signal(object)
    sig_presets = Signal(object)
    sig_steam_games = Signal(object)

    def __init__(self, parent: QWidget):
        super().__init__(parent)
        self._pending_settings: dict = {}
        self._presets: dict[str, dict] = {}
        self._overrides: list[dict] = []
        self._steam_games: list[dict] = []

        self.sig_eq_settings.connect(self._on_eq_settings)
        self.sig_presets.connect(self._on_presets)
        self.sig_steam_games.connect(self._on_steam_games)

        outer = QVBoxLayout()
        outer.setContentsMargins(0, 0, 0, 4)
        self.setLayout(outer)

        # Title
        title = QLabel(I18n.translate('ui', 'eq'))
        font = title.font()
        font.setBold(True)
        font.setPointSize(16)
        title.setFont(font)
        outer.addWidget(title)

        # Scroll area for channel sections + overrides
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        content = QWidget()
        cl = QVBoxLayout()
        cl.setAlignment(Qt.AlignmentFlag.AlignTop)
        content.setLayout(cl)
        scroll.setWidget(content)
        outer.addWidget(scroll, 1)

        self._media_section = QChannelSection('media', content)
        self._media_section.preset_saved.connect(self._on_preset_saved)
        self._media_section.preset_deleted.connect(self._on_preset_deleted)
        cl.addWidget(self._media_section)

        self._chat_section = QChannelSection('chat', content)
        self._chat_section.preset_saved.connect(self._on_preset_saved)
        self._chat_section.preset_deleted.connect(self._on_preset_deleted)
        cl.addWidget(self._chat_section)

        # App overrides section
        overrides_box = QGroupBox(I18n.translate('ui', 'eq_app_overrides'))
        ov = QVBoxLayout()
        overrides_box.setLayout(ov)

        self._overrides_list = QListWidget()
        self._overrides_list.setMaximumHeight(140)
        ov.addWidget(self._overrides_list)

        ov_btns = QHBoxLayout()
        add_btn = QPushButton(I18n.translate('ui', 'eq_add_override'))
        add_btn.clicked.connect(self._add_override)
        rm_btn = QPushButton(I18n.translate('ui', 'eq_remove_override'))
        rm_btn.clicked.connect(self._remove_override)
        ov_btns.addWidget(add_btn)
        ov_btns.addWidget(rm_btn)
        ov_btns.addStretch()
        ov.addLayout(ov_btns)
        cl.addWidget(overrides_box)

        # Apply button (outside scroll)
        apply_btn = QPushButton(I18n.translate('ui', 'eq_apply'))
        apply_btn.clicked.connect(self._apply)
        outer.addWidget(apply_btn)

    def showEvent(self, event) -> None:
        super().showEvent(event)
        self.refresh()

    def refresh(self) -> None:
        DbusWrapper.request_eq_settings(self.sig_eq_settings)
        DbusWrapper.request_eq_presets(self.sig_presets)
        DbusWrapper.request_steam_games(self.sig_steam_games)

    # ------------------------------------------------------------------
    # Signal handlers
    # ------------------------------------------------------------------

    def _on_eq_settings(self, settings: dict) -> None:
        self._pending_settings = settings
        self._overrides = settings.get('app_overrides', [])
        self._refresh_overrides_list()
        self._apply_settings_to_sections()

    def _on_presets(self, presets: list) -> None:
        self._presets = {p['name']: p for p in presets}
        self._apply_settings_to_sections()

    def _on_steam_games(self, games: list) -> None:
        self._steam_games = games

    def _apply_settings_to_sections(self) -> None:
        self._media_section.load_settings(self._pending_settings.get('media', {}), self._presets)
        self._chat_section.load_settings(self._pending_settings.get('chat', {}), self._presets)

    def _on_preset_saved(self, preset: dict) -> None:
        DbusWrapper.save_eq_preset(preset)
        QTimer.singleShot(400, lambda: DbusWrapper.request_eq_presets(self.sig_presets))

    def _on_preset_deleted(self, name: str) -> None:
        DbusWrapper.delete_eq_preset(name)
        QTimer.singleShot(400, lambda: DbusWrapper.request_eq_presets(self.sig_presets))

    # ------------------------------------------------------------------
    # App overrides
    # ------------------------------------------------------------------

    def _refresh_overrides_list(self) -> None:
        self._overrides_list.clear()
        for o in self._overrides:
            mt = o.get('matcher_type', '')
            if mt == 'steam':
                src = f"Steam: {o.get('steam_game_name') or o.get('steam_app_id', '')}"
            elif mt == 'executable':
                src = f"Exec: {o.get('value', '')}"
            else:
                src = f"Stream: {o.get('value', '')}"
            preset = o.get('preset_name') or 'flat'
            channel = o.get('channel', 'media')
            self._overrides_list.addItem(f'{src}  →  {preset} ({channel})')

    def _add_override(self) -> None:
        dlg = QAddOverrideDialog(list(self._presets.keys()), self._steam_games, self)
        if dlg.exec() == QDialog.DialogCode.Accepted:
            self._overrides.append(dlg.get_override())
            self._refresh_overrides_list()

    def _remove_override(self) -> None:
        idx = self._overrides_list.currentRow()
        if 0 <= idx < len(self._overrides):
            del self._overrides[idx]
            self._refresh_overrides_list()

    # ------------------------------------------------------------------
    # Apply
    # ------------------------------------------------------------------

    def _apply(self) -> None:
        settings = {
            'media': self._media_section.get_settings(),
            'chat': self._chat_section.get_settings(),
            'app_overrides': self._overrides,
        }
        DbusWrapper.set_eq_settings(settings)
