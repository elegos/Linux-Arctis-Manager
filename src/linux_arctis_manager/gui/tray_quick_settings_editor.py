from __future__ import annotations

import json
import logging
from pathlib import Path

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (QDialog, QDialogButtonBox, QHBoxLayout, QLabel,
                               QListWidget, QListWidgetItem, QPushButton,
                               QVBoxLayout, QWidget)

from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('tray_quick_settings_editor')
_T = lambda s, k: I18n.translate(s, k)  # noqa: E731

# Registry of all potentially available quick settings.
# id → (i18n_section, i18n_key, source)
# source: 'setting' = daemon settings D-Bus, 'nc_preset' = NC D-Bus
QUICK_SETTING_REGISTRY: list[tuple[str, str, str, str]] = [
    ('nc_preset',               'ui',       'nc',                      'nc_preset'),
    ('sidetone',                'status',   'sidetone',                'setting'),
    ('noise_cancelling',        'status',   'noise_cancelling',        'setting'),
    ('noise_cancelling_level',  'status',   'noise_cancelling_level',  'setting'),
    ('transparency_mode',       'status',   'transparency_mode',       'setting'),
    ('transparent_level',       'status',   'transparent_level',       'setting'),
    ('device_gain',             'status',   'device_gain',             'setting'),
    ('power_inactivity_timer',  'status',   'power_inactivity_timer',  'setting'),
    ('auto_off_time_minutes',   'status',   'auto_off_time_minutes',   'setting'),
    ('bluetooth_auto_mute',     'status',   'bluetooth_auto_mute',     'setting'),
    ('bluetooth_default',       'status',   'bluetooth_default',       'setting'),
]


def _config_path(pid: int) -> Path:
    base = Path.home() / '.config' / 'arctis_manager'
    base.mkdir(parents=True, exist_ok=True)
    return base / f'quick_settings_{pid:04x}.json'


def load_config(pid: int) -> list[str]:
    try:
        data = json.loads(_config_path(pid).read_text())
        return data.get('items', [])
    except Exception:
        return []


def save_config(pid: int, items: list[str]) -> None:
    try:
        _config_path(pid).write_text(json.dumps({'items': items}, indent=2))
    except Exception as e:
        logger.warning('Failed to save quick settings config: %s', e)


class QQuickSettingsEditor(QDialog):
    """Dialog for selecting and ordering quick settings for a device."""

    saved = Signal(list)  # emits ordered list[str] of setting IDs

    def __init__(
        self,
        pid: int,
        available_ids: list[str],
        current_items: list[str],
        parent: QWidget | None = None,
    ) -> None:
        super().__init__(parent)
        self._pid = pid
        self._available_ids = available_ids

        self.setWindowTitle(_T('ui', 'open_app'))  # reuse locale; title below
        self.setMinimumWidth(360)

        root = QVBoxLayout()
        root.setContentsMargins(12, 12, 12, 12)
        self.setLayout(root)

        title = QLabel(_T('ui', 'quick_settings_edit_title'))
        title.setStyleSheet('font-weight: bold; font-size: 13px;')
        root.addWidget(title)

        hint = QLabel(_T('ui', 'quick_settings_edit_hint'))
        hint.setWordWrap(True)
        hint.setStyleSheet('color: gray; font-size: 11px;')
        root.addWidget(hint)

        # List of currently enabled items (ordered)
        list_row = QHBoxLayout()
        root.addLayout(list_row)

        self._list = QListWidget()
        self._list.setDragDropMode(QListWidget.DragDropMode.InternalMove)
        self._list.setSelectionMode(QListWidget.SelectionMode.SingleSelection)
        list_row.addWidget(self._list)

        btn_col = QVBoxLayout()
        btn_col.setAlignment(Qt.AlignmentFlag.AlignTop)
        list_row.addLayout(btn_col)

        self._btn_up = QPushButton('▲')
        self._btn_up.setFixedWidth(32)
        self._btn_up.clicked.connect(self._move_up)
        btn_col.addWidget(self._btn_up)

        self._btn_down = QPushButton('▼')
        self._btn_down.setFixedWidth(32)
        self._btn_down.clicked.connect(self._move_down)
        btn_col.addWidget(self._btn_down)

        self._btn_remove = QPushButton('✕')
        self._btn_remove.setFixedWidth(32)
        self._btn_remove.clicked.connect(self._remove_item)
        btn_col.addWidget(self._btn_remove)

        # Available settings not yet added
        add_row = QHBoxLayout()
        root.addLayout(add_row)

        self._available_list = QListWidget()
        self._available_list.setMaximumHeight(120)
        add_row.addWidget(self._available_list)

        self._btn_add = QPushButton('＋ Add')
        self._btn_add.clicked.connect(self._add_item)
        add_row.addWidget(self._btn_add)

        buttons = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Save | QDialogButtonBox.StandardButton.Cancel
        )
        buttons.accepted.connect(self._on_save)
        buttons.rejected.connect(self.reject)
        root.addWidget(buttons)

        self._populate(current_items)

    # ── Internal helpers ───────────────────────────────────────────────────────

    def _label_for(self, setting_id: str) -> str:
        for sid, section, key, _ in QUICK_SETTING_REGISTRY:
            if sid == setting_id:
                return _T(section, key)
        return setting_id

    def _populate(self, current_items: list[str]) -> None:
        enabled = [i for i in current_items if i in self._available_ids]
        for sid in enabled:
            item = QListWidgetItem(self._label_for(sid))
            item.setData(Qt.ItemDataRole.UserRole, sid)
            self._list.addItem(item)
        self._refresh_available()

    def _enabled_ids(self) -> list[str]:
        return [
            self._list.item(i).data(Qt.ItemDataRole.UserRole)
            for i in range(self._list.count())
        ]

    def _refresh_available(self) -> None:
        self._available_list.clear()
        enabled = set(self._enabled_ids())
        for sid in self._available_ids:
            if sid not in enabled:
                item = QListWidgetItem(self._label_for(sid))
                item.setData(Qt.ItemDataRole.UserRole, sid)
                self._available_list.addItem(item)

    def _move_up(self) -> None:
        row = self._list.currentRow()
        if row > 0:
            item = self._list.takeItem(row)
            self._list.insertItem(row - 1, item)
            self._list.setCurrentRow(row - 1)

    def _move_down(self) -> None:
        row = self._list.currentRow()
        if row < self._list.count() - 1:
            item = self._list.takeItem(row)
            self._list.insertItem(row + 1, item)
            self._list.setCurrentRow(row + 1)

    def _remove_item(self) -> None:
        row = self._list.currentRow()
        if row >= 0:
            self._list.takeItem(row)
            self._refresh_available()

    def _add_item(self) -> None:
        sel = self._available_list.currentItem()
        if not sel:
            return
        sid = sel.data(Qt.ItemDataRole.UserRole)
        item = QListWidgetItem(self._label_for(sid))
        item.setData(Qt.ItemDataRole.UserRole, sid)
        self._list.addItem(item)
        self._refresh_available()

    def _on_save(self) -> None:
        items = self._enabled_ids()
        save_config(self._pid, items)
        self.saved.emit(items)
        self.accept()
