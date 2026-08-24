from __future__ import annotations

import configparser
import math
import logging
import os
from pathlib import Path

from PySide6.QtCore import Qt, QTimer, Signal
from PySide6.QtGui import (QColor, QFont, QLinearGradient, QPainter,
                           QPainterPath, QPen)
from PySide6.QtWidgets import (QComboBox, QDialog, QDialogButtonBox,
                               QFileDialog, QFormLayout, QFrame, QGroupBox,
                               QHBoxLayout, QInputDialog, QLabel, QLineEdit,
                               QListWidget, QMessageBox, QPushButton,
                               QScrollArea, QSizePolicy, QSlider,
                               QStackedWidget, QVBoxLayout, QWidget)

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.qt_widgets.q_checkable_button_group import \
    QCheckableButtonGroup
from linux_arctis_manager.gui.qt_widgets.q_dual_state import QDualState
from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('QEQWidget')

_GAIN_MIN  = -120   # slider integer units (tenths of dB) → -12.0 dB
_GAIN_MAX  =  120   # slider integer units (tenths of dB) → +12.0 dB
_DB_MIN    = -12.0
_DB_MAX    =  12.0


def _fmt_gain(tenths: int) -> str:
    return f'{tenths / 10:+.1f}'


def _fmt_freq(hz: int) -> str:
    if hz >= 1000:
        v = hz / 1000
        return f'{v:g}k'
    return str(hz)


def _load_desktop_apps() -> list[tuple[str, str]]:
    """Return (display_name, exec_basename) pairs from .desktop files, sorted by name.

    Covers native, Snap, and Flatpak applications.
    """
    search_dirs = [
        Path('/usr/share/applications'),
        Path('/usr/local/share/applications'),
        Path.home() / '.local/share/applications',
        Path('/var/lib/snapd/desktop/applications'),
        Path('/var/lib/flatpak/exports/share/applications'),
        Path.home() / '.local/share/flatpak/exports/share/applications',
    ]
    apps: list[tuple[str, str]] = []
    seen: set[tuple[str, str]] = set()
    for d in search_dirs:
        if not d.is_dir():
            continue
        for path in sorted(d.glob('*.desktop')):
            try:
                cp = configparser.RawConfigParser(strict=False)
                cp.read(path, encoding='utf-8')
                if not cp.has_section('Desktop Entry'):
                    continue
                de = cp['Desktop Entry']
                if de.get('Type', '') != 'Application':
                    continue
                if de.get('NoDisplay', 'false').lower() == 'true':
                    continue
                name = de.get('Name', '').strip()
                if not name:
                    continue
                exec_val = de.get('TryExec', '') or de.get('Exec', '')
                exec_cmd = exec_val.split()[0] if exec_val else ''
                exec_base = os.path.basename(exec_cmd)
                exec_raw = de.get('Exec', '')
                # Skip Steam game launchers — handled by the Steam game matcher
                if 'steam://rungameid' in exec_raw:
                    continue
                # Flatpak wrapper: extract basename from the app ID in the Exec line
                if exec_base == 'flatpak':
                    parts = exec_raw.split()
                    app_id = next(
                        (p for p in reversed(parts)
                         if not p.startswith('-') and '.' in p and not p.startswith('%')),
                        '',
                    )
                    exec_base = app_id.split('.')[-1].lower() if app_id else ''
                # Skip generic shell/interpreter wrappers and AppImage entries with hash names
                _WRAPPERS = {'sh', 'bash', 'dash', 'env', 'xdg-open',
                             'python', 'python3', 'python2', 'ruby', 'perl',
                             'java', 'mono', 'wine', 'wine64'}
                if exec_base in _WRAPPERS or not exec_base or len(exec_base) > 80:
                    continue
                entry = (name, exec_base)
                if entry not in seen:
                    seen.add(entry)
                    apps.append(entry)
            except Exception:
                continue
    apps.sort(key=lambda x: x[0].lower())
    return apps


# ---------------------------------------------------------------------------
# EQ curve visualiser
# ---------------------------------------------------------------------------

class QEQCurveWidget(QWidget):
    """Draws a smooth EQ curve; dots are draggable to adjust band gains."""

    band_gain_changed = Signal(int, float)  # (band_index, new_gain_dB)

    _MARGIN_TOP    = 4
    _MARGIN_BOTTOM = 20   # room for Hz labels
    _PAD           = 18   # horizontal pad so edge dots aren't clipped
    _HIT_RADIUS    = 12   # px — click must be within this to grab a dot

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._freqs: list[int] = []
        self._gains: list[float] = []
        self._drag_idx: int | None = None
        self._disabled = False
        self.setFixedHeight(110)
        self.setMouseTracking(True)

    def set_data(self, freqs: list[int], gains: list[float]) -> None:
        self._freqs = freqs
        self._gains = list(gains)
        self.update()

    def set_disabled(self, disabled: bool) -> None:
        self._disabled = disabled

    # ------------------------------------------------------------------
    # Geometry helpers
    # ------------------------------------------------------------------

    def _graph_h(self) -> int:
        return self.height() - self._MARGIN_TOP - self._MARGIN_BOTTOM

    def _db_y(self, db: float) -> float:
        return self._MARGIN_TOP + (1.0 - (db - _DB_MIN) / (_DB_MAX - _DB_MIN)) * self._graph_h()

    def _y_db(self, y: float) -> float:
        return _DB_MIN + (1.0 - (y - self._MARGIN_TOP) / self._graph_h()) * (_DB_MAX - _DB_MIN)

    def _band_pts(self) -> list[tuple[float, float]]:
        if len(self._freqs) < 2:
            return []
        w = self.width()
        lo = math.log10(self._freqs[0])
        span = (w - 2 * self._PAD) / (math.log10(self._freqs[-1]) - lo)
        return [
            (self._PAD + (math.log10(f) - lo) * span, self._db_y(g))
            for f, g in zip(self._freqs, self._gains)
        ]

    # ------------------------------------------------------------------
    # Mouse interaction
    # ------------------------------------------------------------------

    def _nearest_idx(self, x: float, y: float) -> int | None:
        best_d, best_i = float('inf'), None
        for i, (px, py) in enumerate(self._band_pts()):
            d = math.hypot(x - px, y - py)
            if d < self._HIT_RADIUS and d < best_d:
                best_d, best_i = d, i
        return best_i

    def mousePressEvent(self, event) -> None:  # noqa: N802
        if self._disabled or event.button() != Qt.MouseButton.LeftButton:
            return
        self._drag_idx = self._nearest_idx(event.position().x(), event.position().y())

    def mouseMoveEvent(self, event) -> None:  # noqa: N802
        if self._drag_idx is None:
            hover = not self._disabled and self._nearest_idx(
                event.position().x(), event.position().y()) is not None
            self.setCursor(Qt.CursorShape.SizeVerCursor if hover else Qt.CursorShape.ArrowCursor)
            return
        db = max(_DB_MIN, min(_DB_MAX, self._y_db(event.position().y())))
        self._gains[self._drag_idx] = db
        self.band_gain_changed.emit(self._drag_idx, db)
        self.update()

    def mouseDoubleClickEvent(self, event) -> None:  # noqa: N802
        if self._disabled or event.button() != Qt.MouseButton.LeftButton:
            return
        idx = self._nearest_idx(event.position().x(), event.position().y())
        if idx is not None:
            self._gains[idx] = 0.0
            self.band_gain_changed.emit(idx, 0.0)
            self.update()

    def mouseReleaseEvent(self, event) -> None:  # noqa: N802
        self._drag_idx = None

    # ------------------------------------------------------------------
    # Paint
    # ------------------------------------------------------------------

    def paintEvent(self, event) -> None:  # noqa: N802
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)

        w, h = self.width(), self.height()
        bg = self.palette().color(self.backgroundRole())
        dark = bg.lightness() < 128

        bg_col    = QColor(18, 20, 26)   if dark else QColor(238, 240, 246)
        grid_col  = QColor(45, 48, 58)   if dark else QColor(205, 207, 215)
        dash_col  = QColor(55, 60, 75)   if dark else QColor(185, 188, 200)
        zero_col  = QColor(75, 80, 98)   if dark else QColor(155, 158, 172)
        curve_col = QColor(0, 195, 125)
        fill_col  = QColor(0, 195, 125, 50)
        dot_col   = QColor(40, 225, 155)
        drag_col  = QColor(255, 220, 60)
        lbl_col   = QColor(100, 106, 125) if dark else QColor(110, 115, 132)

        graph_top    = self._MARGIN_TOP
        graph_bottom = h - self._MARGIN_BOTTOM

        painter.fillRect(0, 0, w, h, bg_col)

        small = QFont()
        small.setPointSize(7)
        painter.setFont(small)

        # Horizontal grid lines + dB labels
        for db in (-60.0, -48.0, -24.0, -12.0, 0.0, 12.0, 24.0):
            y = int(self._db_y(db))
            if y < self._MARGIN_TOP or y > self.height() - self._MARGIN_BOTTOM:
                continue
            pen = QPen(zero_col if db == 0.0 else grid_col, 1)
            painter.setPen(pen)
            painter.drawLine(0, y, w, y)
            painter.setPen(lbl_col)
            painter.drawText(3, y - 1, f'{int(db):+d}')

        pts = self._band_pts()

        # Vertical dashed lines + Hz labels at the bottom
        if pts:
            painter.setRenderHint(QPainter.RenderHint.Antialiasing, False)
            dash_pen = QPen(dash_col, 1, Qt.PenStyle.DashLine)
            painter.setPen(dash_pen)
            for (px, _), _ in zip(pts, self._freqs):
                painter.drawLine(int(px), graph_top, int(px), graph_bottom)

            painter.setPen(lbl_col)
            fm = painter.fontMetrics()
            for (px, _), f in zip(pts, self._freqs):
                lbl = _fmt_freq(f) + 'Hz'
                tw  = fm.horizontalAdvance(lbl)
                tx  = max(0, min(int(px) - tw // 2, w - tw))
                painter.drawText(tx, h - 2, lbl)
            painter.setRenderHint(QPainter.RenderHint.Antialiasing, True)

        if len(pts) < 2:
            painter.end()
            return

        y0 = int(self._db_y(0.0))
        curve = self._catmull_rom(pts)

        # Fill between curve and 0 dB line
        fill = QPainterPath()
        fill.moveTo(pts[0][0], float(y0))
        fill.lineTo(pts[0][0], pts[0][1])
        fill.addPath(curve)
        fill.lineTo(pts[-1][0], float(y0))
        fill.closeSubpath()
        painter.fillPath(fill, fill_col)

        painter.setPen(QPen(curve_col, 2))
        painter.drawPath(curve)

        # Dots (highlighted when dragging)
        painter.setPen(Qt.PenStyle.NoPen)
        for i, (px, py) in enumerate(pts):
            painter.setBrush(drag_col if i == self._drag_idx else dot_col)
            painter.drawEllipse(int(px) - 4, int(py) - 4, 8, 8)

        painter.end()

    @staticmethod
    def _catmull_rom(pts: list[tuple[float, float]]) -> QPainterPath:
        path = QPainterPath()
        path.moveTo(pts[0][0], pts[0][1])
        n = len(pts)
        for i in range(n - 1):
            p0 = pts[max(0, i - 1)]
            p1 = pts[i]
            p2 = pts[i + 1]
            p3 = pts[min(n - 1, i + 2)]
            cp1x = p1[0] + (p2[0] - p0[0]) / 6.0
            cp1y = p1[1] + (p2[1] - p0[1]) / 6.0
            cp2x = p2[0] - (p3[0] - p1[0]) / 6.0
            cp2y = p2[1] - (p3[1] - p1[1]) / 6.0
            path.cubicTo(cp1x, cp1y, cp2x, cp2y, p2[0], p2[1])
        return path


# ---------------------------------------------------------------------------
# Single vertical band column
# ---------------------------------------------------------------------------

class QBandColumn(QWidget):
    gain_changed = Signal()

    WIDTH = 52

    def __init__(self, frequency: int, gain: float, disabled: bool = False,
                 parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.frequency = frequency
        self.setFixedWidth(self.WIDTH)

        layout = QVBoxLayout()
        layout.setContentsMargins(2, 4, 2, 4)
        layout.setSpacing(2)
        self.setLayout(layout)

        tiny = QFont()
        tiny.setPointSize(7)

        # Gain value label (top)
        self._val_label = QLabel(_fmt_gain(int(gain * 10)) + ' dB')
        self._val_label.setAlignment(Qt.AlignmentFlag.AlignHCenter)
        self._val_label.setFont(tiny)
        layout.addWidget(self._val_label)

        # Vertical slider
        self._slider = QSlider(Qt.Orientation.Vertical)
        self._slider.setRange(_GAIN_MIN, _GAIN_MAX)
        self._slider.setValue(int(gain * 10))
        self._slider.setEnabled(not disabled)
        self._slider.valueChanged.connect(
            lambda v: self._val_label.setText(_fmt_gain(v) + ' dB'))
        self._slider.valueChanged.connect(lambda _: self.gain_changed.emit())
        layout.addWidget(self._slider, 1)

        # Frequency label (bottom)
        freq_lbl = QLabel(_fmt_freq(frequency))
        freq_lbl.setAlignment(Qt.AlignmentFlag.AlignHCenter)
        freq_lbl.setFont(tiny)
        layout.addWidget(freq_lbl)

    @property
    def gain(self) -> float:
        return self._slider.value() / 10.0

    def set_enabled_slider(self, enabled: bool) -> None:
        self._slider.setEnabled(enabled)


# ---------------------------------------------------------------------------
# Combined curve + vertical sliders view
# ---------------------------------------------------------------------------

_SLIDER_AREA_HEIGHT = 150   # height of the band-column area (px)


class QEQBandsView(QWidget):
    band_changed = Signal()   # any band gain changed (slider or curve drag)

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._columns: list[QBandColumn] = []

        outer = QVBoxLayout()
        outer.setContentsMargins(0, 0, 0, 0)
        outer.setSpacing(4)
        self.setLayout(outer)

        self._curve = QEQCurveWidget()
        self._curve.band_gain_changed.connect(self._on_curve_drag)
        outer.addWidget(self._curve)

        # Horizontal scroll area for the band columns
        self._scroll = QScrollArea()
        self._scroll.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAsNeeded)
        self._scroll.setVerticalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self._scroll.setWidgetResizable(False)
        self._scroll.setFixedHeight(_SLIDER_AREA_HEIGHT)

        self._cols_widget = QWidget()
        self._cols_widget.setFixedHeight(_SLIDER_AREA_HEIGHT - 4)  # fill viewport height
        self._cols_layout = QHBoxLayout()
        self._cols_layout.setContentsMargins(4, 0, 4, 0)
        self._cols_layout.setSpacing(0)
        self._cols_layout.setAlignment(Qt.AlignmentFlag.AlignLeft)
        self._cols_widget.setLayout(self._cols_layout)
        self._scroll.setWidget(self._cols_widget)
        outer.addWidget(self._scroll)

        self.setVisible(False)

    def _on_curve_drag(self, idx: int, gain_db: float) -> None:
        if 0 <= idx < len(self._columns):
            col = self._columns[idx]
            col._slider.blockSignals(True)
            col._slider.setValue(int(round(gain_db * 10)))
            col._val_label.setText(_fmt_gain(int(round(gain_db * 10))) + ' dB')
            col._slider.blockSignals(False)
            self.band_changed.emit()

    def set_bands(self, bands: list[dict], disabled: bool = False) -> None:
        # Clear old columns
        while self._cols_layout.count():
            item = self._cols_layout.takeAt(0)
            if w := item.widget():
                w.deleteLater()
        self._columns = []

        for band in bands:
            col = QBandColumn(int(band['frequency']), float(band.get('gain', 0.0)), disabled)
            col.gain_changed.connect(self._refresh_curve)
            col.gain_changed.connect(self.band_changed)
            self._cols_layout.addWidget(col)
            self._columns.append(col)

        self._cols_widget.setFixedWidth(max(len(bands) * QBandColumn.WIDTH + 8, 80))
        self._curve.set_disabled(disabled)
        self._refresh_curve()
        self.setVisible(bool(bands))

    def get_bands(self) -> list[dict]:
        return [{'frequency': c.frequency, 'gain': c.gain} for c in self._columns]

    def _refresh_curve(self) -> None:
        self._curve.set_data(
            [c.frequency for c in self._columns],
            [c.gain for c in self._columns],
        )


# ---------------------------------------------------------------------------
# "Add app override" dialog
# ---------------------------------------------------------------------------

class QAddOverrideDialog(QDialog):
    def __init__(self, preset_names: list[str], factory_presets: dict[int, str],
                 steam_games: list[dict],
                 parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle(I18n.translate('ui', 'eq_add_override'))

        # Deduplicate steam games by app_id (multiple library folders cause duplicates).
        seen_ids: set = set()
        self._unique_games: list[dict] = []
        for g in steam_games:
            if g['app_id'] not in seen_ids:
                seen_ids.add(g['app_id'])
                self._unique_games.append(g)

        layout = QFormLayout()
        self.setLayout(layout)

        self._type_combo = QComboBox()
        self._type_combo.addItems([
            I18n.translate('ui', 'eq_match_executable'),
            I18n.translate('ui', 'eq_match_steam'),
        ])
        self._type_combo.currentIndexChanged.connect(self._on_type_changed)
        layout.addRow(I18n.translate('ui', 'eq_match_by'), self._type_combo)

        # Stacked value widget (page 0 = app picker, page 1 = steam).
        self._val_label = QLabel(I18n.translate('ui', 'eq_match_executable'))
        self._val_stack = QStackedWidget()

        # Page 0: searchable combo from installed .desktop apps + Browse fallback.
        exec_widget = QWidget()
        exec_row = QHBoxLayout()
        exec_row.setContentsMargins(0, 0, 0, 0)
        exec_widget.setLayout(exec_row)
        self._exec_combo = QComboBox()
        self._exec_combo.setEditable(True)
        self._exec_combo.setInsertPolicy(QComboBox.InsertPolicy.NoInsert)
        self._exec_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        completer = self._exec_combo.completer()
        completer.setFilterMode(Qt.MatchFlag.MatchContains)
        completer.setCaseSensitivity(Qt.CaseSensitivity.CaseInsensitive)
        for app_name, exec_base in _load_desktop_apps():
            self._exec_combo.addItem(app_name, exec_base)
        browse_btn = QPushButton(I18n.translate('ui', 'eq_browse'))
        browse_btn.clicked.connect(self._browse_executable)
        exec_row.addWidget(self._exec_combo)
        exec_row.addWidget(browse_btn)
        self._val_stack.addWidget(exec_widget)

        # Page 1: steam game combo.
        self._game_combo = QComboBox()
        self._game_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        for g in self._unique_games:
            self._game_combo.addItem(g['name'], g['app_id'])
        self._val_stack.addWidget(self._game_combo)

        layout.addRow(self._val_label, self._val_stack)

        self._preset_combo = QComboBox()
        self._preset_combo.addItem(I18n.translate('ui', 'eq_flat'), 'flat')
        for idx in sorted(factory_presets):
            label = I18n.translate('settings_values', factory_presets[idx])
            self._preset_combo.addItem(label, idx)
        if factory_presets and preset_names:
            self._preset_combo.insertSeparator(self._preset_combo.count())
        for name in preset_names:
            self._preset_combo.addItem(name, name)
        layout.addRow(I18n.translate('ui', 'eq_preset'), self._preset_combo)

        self._channel_combo = QComboBox()
        self._channel_combo.addItems(['Media', 'Chat'])
        layout.addRow(I18n.translate('ui', 'eq_channel'), self._channel_combo)

        btns = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        btns.accepted.connect(self.accept)
        btns.rejected.connect(self.reject)
        layout.addRow(btns)

    def _on_type_changed(self, idx: int) -> None:
        labels = [
            I18n.translate('ui', 'eq_match_executable'),
            I18n.translate('ui', 'eq_match_steam'),
        ]
        self._val_label.setText(labels[idx])
        self._val_stack.setCurrentIndex(idx)

    def _browse_executable(self) -> None:
        path, _ = QFileDialog.getOpenFileName(self, I18n.translate('ui', 'eq_match_executable'))
        if not path:
            return
        base = os.path.basename(path)
        for i in range(self._exec_combo.count()):
            if self._exec_combo.itemData(i) == base:
                self._exec_combo.setCurrentIndex(i)
                return
        self._exec_combo.insertItem(0, base, base)
        self._exec_combo.setCurrentIndex(0)

    def get_override(self) -> dict:
        type_idx = self._type_combo.currentIndex()
        preset_data = self._preset_combo.currentData()

        hw_preset_idx = None
        preset_name = ''
        if isinstance(preset_data, int):
            hw_preset_idx = preset_data
        elif isinstance(preset_data, str) and preset_data and preset_data != 'flat':
            preset_name = preset_data

        if type_idx == 0:  # executable
            combo_idx = self._exec_combo.currentIndex()
            if combo_idx >= 0:
                value = self._exec_combo.itemData(combo_idx) or os.path.basename(self._exec_combo.currentText())
            else:
                value = os.path.basename(self._exec_combo.currentText().strip())
            result = {
                'matcher_type': 'executable',
                'value': value,
                'steam_app_id': None,
                'steam_game_name': '',
                'preset_name': preset_name,
                'channel': 'chat' if self._channel_combo.currentIndex() == 1 else 'media',
            }
        else:  # steam
            gi = self._game_combo.currentIndex()
            result = {
                'matcher_type': 'steam',
                'value': '',
                'steam_app_id': self._unique_games[gi]['app_id'] if 0 <= gi < len(self._unique_games) else None,
                'steam_game_name': self._unique_games[gi]['name'] if 0 <= gi < len(self._unique_games) else '',
                'preset_name': preset_name,
                'channel': 'chat' if self._channel_combo.currentIndex() == 1 else 'media',
            }

        if hw_preset_idx is not None:
            result['hw_preset_idx'] = hw_preset_idx
        return result


# ---------------------------------------------------------------------------
# Per-channel EQ section
# ---------------------------------------------------------------------------

_BACKEND_IDX = {'auto': 0, 'ladspa': 1, 'hardware': 2}
_BACKEND_VAL = {0: 'auto', 1: 'ladspa', 2: 'hardware'}


class QChannelSection(QGroupBox):
    preset_saved              = Signal(object)   # dict with name/mode/bands
    preset_deleted            = Signal(str)      # preset name
    preset_selection_changed  = Signal()         # user changed preset combo

    def __init__(self, channel: str, parent: QWidget | None = None) -> None:
        title = I18n.translate('ui', f'eq_{channel}_channel')
        super().__init__(title, parent)
        self._channel = channel
        self._presets: dict[str, dict] = {}
        self._pending_select_name: str | None = None

        layout = QVBoxLayout()
        self.setLayout(layout)

        # Enable row
        er = QHBoxLayout()
        er.addWidget(QLabel(I18n.translate('ui', 'eq_enable')))
        self._enable = QDualState(
            off_text=I18n.translate('settings_values', 'off'),
            on_text=I18n.translate('settings_values', 'on'),
            init_state='left',
        )
        er.addWidget(self._enable)
        er.addStretch()
        layout.addLayout(er)

        # Mode row
        mr = QHBoxLayout()
        mr.addWidget(QLabel(I18n.translate('ui', 'eq_mode')))
        self._mode_group = QCheckableButtonGroup()
        self._mode_group.addButton(0, 'simple',   True,  'settings_values')
        self._mode_group.addButton(1, 'advanced',  False, 'settings_values')
        self._mode_group.new_value.connect(self._on_mode_changed)
        mr.addWidget(self._mode_group)
        mr.addStretch()
        layout.addLayout(mr)

        # Backend row — visible only when device has hardware EQ support
        br = QHBoxLayout()
        br.addWidget(QLabel(I18n.translate('ui', 'eq_backend')))
        self._backend_group = QCheckableButtonGroup()
        self._backend_group.addButton(0, 'auto',     True,  'settings_values')
        self._backend_group.addButton(1, 'ladspa',   False, 'settings_values')
        self._backend_group.addButton(2, 'hardware', False, 'settings_values')
        self._backend_group.new_value.connect(lambda _: self.preset_selection_changed.emit())
        br.addWidget(self._backend_group)
        br.addStretch()
        self._backend_widget = QWidget()
        self._backend_widget.setLayout(br)
        self._backend_widget.setVisible(False)
        layout.addWidget(self._backend_widget)

        # Note shown when hardware app-override tracking is unavailable (e.g. GNOME Wayland)
        self._hw_override_note = QLabel()
        self._hw_override_note.setWordWrap(True)
        self._hw_override_note.setVisible(False)
        layout.addWidget(self._hw_override_note)

        # Preset row
        pr = QHBoxLayout()
        pr.addWidget(QLabel(I18n.translate('ui', 'eq_preset')))
        self._preset_combo = QComboBox()
        self._preset_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self._preset_combo.currentIndexChanged.connect(self._on_preset_changed)
        pr.addWidget(self._preset_combo)

        self._new_btn = QPushButton(I18n.translate('ui', 'eq_new_preset'))
        self._new_btn.clicked.connect(self._new_preset)
        pr.addWidget(self._new_btn)

        self._delete_btn = QPushButton(I18n.translate('ui', 'eq_delete_preset'))
        self._delete_btn.clicked.connect(self._delete_preset)
        pr.addWidget(self._delete_btn)
        layout.addLayout(pr)

        self._autosave_timer = QTimer(self)
        self._autosave_timer.setSingleShot(True)
        self._autosave_timer.setInterval(500)
        self._autosave_timer.timeout.connect(self._save_preset)

        # Read-only hint label (shown for builtin presets)
        self._builtin_hint = QLabel(I18n.translate('ui', 'eq_builtin_hint'))
        self._builtin_hint.setVisible(False)
        layout.addWidget(self._builtin_hint)

        # Band view (curve + vertical sliders)
        self._bands_view = QEQBandsView()
        self._bands_view.band_changed.connect(self._on_band_changed)
        layout.addWidget(self._bands_view)

        # Seed combo with just "Flat"
        self._rebuild_preset_combo(None)

    # ------------------------------------------------------------------
    # Public API called by QEQWidget
    # ------------------------------------------------------------------

    def set_hw_eq_visible(self, visible: bool) -> None:
        self._backend_widget.setVisible(visible)

    def set_hw_override_note(self, reason: str | None) -> None:
        if reason:
            self._hw_override_note.setText(reason)
            self._hw_override_note.setVisible(True)
        else:
            self._hw_override_note.setVisible(False)

    def load_settings(self, settings: dict, presets: dict[str, dict]) -> None:
        self._presets = presets

        enabled     = settings.get('enabled', False)
        mode        = settings.get('mode', 'simple')
        preset_name = settings.get('preset_name')
        backend     = settings.get('backend', 'auto')

        self._enable.toggle.blockSignals(True)
        self._enable.toggle.setChecked(enabled)
        self._enable.toggle.blockSignals(False)

        mode_val = 1 if mode == 'advanced' else 0
        for btn in self._mode_group.buttons:
            btn.setChecked(btn.property('value') == mode_val)

        bidx = _BACKEND_IDX.get(backend, 0)
        for btn in self._backend_group.buttons:
            btn.setChecked(btn.property('value') == bidx)

        self._rebuild_preset_combo(preset_name)

    def set_presets(self, presets: dict[str, dict]) -> None:
        self._presets = presets
        # _pending_select_name wins; otherwise keep current selection
        self._rebuild_preset_combo(self._current_preset_name())

    def get_settings(self) -> dict:
        return {
            'enabled': self._enable.toggle.isChecked(),
            'mode': self._current_mode(),
            'preset_name': self._current_preset_name(),
            'backend': self._current_backend(),
        }

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _current_mode(self) -> str:
        for btn in self._mode_group.buttons:
            if btn.isChecked():
                return 'advanced' if btn.property('value') == 1 else 'simple'
        return 'simple'

    def _current_backend(self) -> str:
        for btn in self._backend_group.buttons:
            if btn.isChecked():
                return _BACKEND_VAL.get(btn.property('value'), 'auto')
        return 'auto'

    def _current_preset_name(self) -> str | None:
        txt = self._preset_combo.currentText()
        return None if txt == I18n.translate('ui', 'eq_flat') else txt

    def _current_preset_is_builtin(self) -> bool:
        name = self._current_preset_name()
        if name is None:
            return False
        return self._presets.get(name, {}).get('builtin', False)

    def _rebuild_preset_combo(self, fallback_name: str | None) -> None:
        # _pending_select_name always wins (e.g. after "New Preset...")
        select_name = self._pending_select_name or fallback_name
        self._pending_select_name = None

        self._preset_combo.blockSignals(True)
        self._preset_combo.clear()
        flat_lbl = I18n.translate('ui', 'eq_flat')
        self._preset_combo.addItem(flat_lbl)
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

    def _set_mode_silently(self, mode: str) -> None:
        target_val = 1 if mode == 'advanced' else 0
        for btn in self._mode_group.buttons:
            btn.blockSignals(True)
            btn.setChecked(btn.property('value') == target_val)
            btn.blockSignals(False)

    def _load_bands_for_current_preset(self) -> None:
        from linux_arctis_manager.eq_preset import EQBand, elevate_bands
        name = self._current_preset_name()
        if name is None or name not in self._presets:
            self._bands_view.set_bands([])
            self._delete_btn.setEnabled(False)
            self._builtin_hint.setVisible(False)
            return

        preset  = self._presets[name]
        bands   = preset.get('bands', [])
        builtin = preset.get('builtin', False)
        stored_mode = preset.get('mode', 'simple')
        ui_mode = self._current_mode()

        # For builtin presets: derive 15-band view from stored 10-band data when advanced is selected
        if builtin and stored_mode == 'simple' and ui_mode == 'advanced':
            bands_obj = [EQBand(frequency=b['frequency'], gain=b['gain']) for b in bands]
            derived = [{'frequency': b.frequency, 'gain': b.gain} for b in elevate_bands(bands_obj)]
            self._bands_view.set_bands(derived, disabled=True)
        else:
            self._bands_view.set_bands(bands, disabled=builtin)

        self._delete_btn.setEnabled(not builtin)
        self._builtin_hint.setVisible(builtin)

    def _on_band_changed(self) -> None:
        if self._current_preset_is_builtin() or self._current_preset_name() is None:
            return
        self._autosave_timer.start()

    def _on_mode_changed(self, _: int) -> None:
        from linux_arctis_manager.eq_preset import EQBand, elevate_bands, downsample_bands
        selected_mode = self._current_mode()
        name = self._current_preset_name()

        if name is None or name not in self._presets:
            self._load_bands_for_current_preset()
            return

        preset      = self._presets[name]
        stored_mode = preset.get('mode', 'simple')
        builtin     = preset.get('builtin', False)

        # Builtin: purely a view change, no conversion needed
        if builtin or selected_mode == stored_mode:
            self._load_bands_for_current_preset()
            return

        bands_raw = preset.get('bands', [])
        bands_obj = [EQBand(frequency=b['frequency'], gain=b['gain']) for b in bands_raw]

        if selected_mode == 'advanced':
            # Elevate simple → advanced by interpolating the 5 missing bands
            new_bands = elevate_bands(bands_obj)
            self.preset_saved.emit({
                'name': name,
                'mode': 'advanced',
                'description': preset.get('description', ''),
                'bands': [{'frequency': b.frequency, 'gain': b.gain} for b in new_bands],
            })
        else:
            # Downscale advanced → simple: redistribute extra band gains to neighbors
            new_bands = downsample_bands(bands_obj)
            self.preset_saved.emit({
                'name': name,
                'mode': 'simple',
                'description': preset.get('description', ''),
                'bands': [{'frequency': b.frequency, 'gain': b.gain} for b in new_bands],
            })

    def _on_preset_changed(self, _: int) -> None:
        # Sync mode toggle to the preset's stored mode
        name = self._current_preset_name()
        if name is not None and name in self._presets:
            stored_mode = self._presets[name].get('mode', 'simple')
            self._set_mode_silently(stored_mode)
        self._load_bands_for_current_preset()
        # Auto-enable when a real preset is chosen; auto-disable for Flat
        has_preset = self._current_preset_name() is not None
        self._enable.toggle.blockSignals(True)
        self._enable.toggle.setChecked(has_preset)
        self._enable.toggle.blockSignals(False)
        self.preset_selection_changed.emit()

    def _bands_as_list(self) -> list[dict]:
        return self._bands_view.get_bands()

    # ------------------------------------------------------------------
    # Preset actions
    # ------------------------------------------------------------------

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
        self._pending_select_name = name   # auto-select after refresh
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
            'bands': self._bands_as_list(),
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


# ---------------------------------------------------------------------------
# Top-level EQ widget
# ---------------------------------------------------------------------------

class QEQWidget(QWidget):
    sig_eq_settings      = Signal(object)
    sig_presets          = Signal(object)
    sig_steam_games      = Signal(object)
    sig_eq_capabilities  = Signal(object)
    sig_hw_apply         = Signal(object)

    def __init__(self, parent: QWidget) -> None:
        super().__init__(parent)
        self._pending_settings: dict = {}
        self._presets: dict[str, dict] = {}
        self._hw_preset_mapping: dict[int, str] = {}  # full slot→name including custom
        self._factory_presets: dict[int, str] = {}   # excludes the custom slot
        self._active_hw_preset_idx: int | None = None
        self._overrides: list[dict] = []
        self._steam_games: list[dict] = []
        self._initial_load_done = False   # prevents preset-list refresh from resetting the combo
        self._ladspa_available = True
        self._has_hw_eq = False

        self._apply_timer_media = QTimer(self)
        self._apply_timer_media.setSingleShot(True)
        self._apply_timer_media.setInterval(400)
        self._apply_timer_media.timeout.connect(lambda: self._apply_channel('media'))

        self._apply_timer_chat = QTimer(self)
        self._apply_timer_chat.setSingleShot(True)
        self._apply_timer_chat.setInterval(400)
        self._apply_timer_chat.timeout.connect(lambda: self._apply_channel('chat'))

        self.sig_eq_settings.connect(self._on_eq_settings)
        self.sig_presets.connect(self._on_presets)
        self.sig_steam_games.connect(self._on_steam_games)
        self.sig_eq_capabilities.connect(self._on_eq_capabilities)
        self.sig_hw_apply.connect(self._on_hw_apply_result)

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

        # LADSPA unavailable warning banner
        self._ladspa_frame = QFrame()
        self._ladspa_frame.setFrameShape(QFrame.Shape.StyledPanel)
        lf_layout = QVBoxLayout()
        lf_layout.setContentsMargins(10, 8, 10, 8)
        lf_layout.setSpacing(6)
        self._ladspa_frame.setLayout(lf_layout)

        self._ladspa_warn_label = QLabel()
        self._ladspa_warn_label.setWordWrap(True)
        lf_layout.addWidget(self._ladspa_warn_label)

        lf_btn_row = QHBoxLayout()
        self._ladspa_retry_btn = QPushButton(I18n.translate('ui', 'eq_retry'))
        self._ladspa_retry_btn.clicked.connect(self._retry_ladspa_check)
        lf_btn_row.addWidget(self._ladspa_retry_btn)
        lf_btn_row.addStretch()
        lf_layout.addLayout(lf_btn_row)

        self._ladspa_frame.setVisible(False)
        outer.addWidget(self._ladspa_frame)

        # Hardware EQ panel — visible only when device has a custom EQ slot.
        hw_frame = QFrame()
        hw_frame.setFrameShape(QFrame.Shape.StyledPanel)
        hw_layout = QVBoxLayout()
        hw_layout.setContentsMargins(10, 8, 10, 8)
        hw_layout.setSpacing(4)
        hw_frame.setLayout(hw_layout)

        hw_active_row = QHBoxLayout()
        hw_active_row.addWidget(QLabel(I18n.translate('ui', 'eq_hw_active_preset')))
        self._hw_active_label = QLabel()
        hw_active_row.addWidget(self._hw_active_label)
        hw_active_row.addStretch()
        hw_layout.addLayout(hw_active_row)

        hw_load_row = QHBoxLayout()
        self._hw_preset_combo = QComboBox()
        hw_load_row.addWidget(self._hw_preset_combo)
        self._hw_apply_btn = QPushButton(I18n.translate('ui', 'eq_hw_apply'))
        self._hw_apply_btn.clicked.connect(self._on_hw_apply_clicked)
        hw_load_row.addWidget(self._hw_apply_btn)
        hw_layout.addLayout(hw_load_row)

        self._hw_status_label = QLabel(I18n.translate('ui', 'eq_hw_status_idle'))
        hw_layout.addWidget(self._hw_status_label)

        self._hw_eq_panel = hw_frame
        self._hw_eq_panel.setVisible(False)
        outer.addWidget(self._hw_eq_panel)

        # Scroll area (channel sections + overrides)
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        content = QWidget()
        cl = QVBoxLayout()
        cl.setAlignment(Qt.AlignmentFlag.AlignTop)
        content.setLayout(cl)
        scroll.setWidget(content)
        outer.addWidget(scroll, 1)

        self._media_section = QChannelSection('media', content)
        self._media_section.preset_saved.connect(lambda p: self._on_preset_saved(p, 'media'))
        self._media_section.preset_deleted.connect(lambda n: self._on_preset_deleted(n, 'media'))
        self._media_section.preset_selection_changed.connect(lambda: self._schedule_apply('media'))
        cl.addWidget(self._media_section)

        self._chat_section = QChannelSection('chat', content)
        self._chat_section.preset_saved.connect(lambda p: self._on_preset_saved(p, 'chat'))
        self._chat_section.preset_deleted.connect(lambda n: self._on_preset_deleted(n, 'chat'))
        self._chat_section.preset_selection_changed.connect(lambda: self._schedule_apply('chat'))
        cl.addWidget(self._chat_section)

        # App overrides
        ov_box = QGroupBox(I18n.translate('ui', 'eq_app_overrides'))
        ov = QVBoxLayout()
        ov.setAlignment(Qt.AlignmentFlag.AlignTop)
        ov_box.setLayout(ov)

        # Warning banner — shown when the focus-tracking backend is unsupported
        self._hw_override_warn_frame = QFrame()
        self._hw_override_warn_frame.setFrameShape(QFrame.Shape.StyledPanel)
        self._hw_override_warn_frame.setStyleSheet(
            'QFrame { background-color: #fff3cd; border: 1px solid #ffc107; border-radius: 4px; }'
        )
        ow_layout = QHBoxLayout()
        ow_layout.setContentsMargins(8, 6, 8, 6)
        self._hw_override_warn_frame.setLayout(ow_layout)
        self._hw_override_warn_label = QLabel()
        self._hw_override_warn_label.setWordWrap(True)
        ow_layout.addWidget(self._hw_override_warn_label)
        self._hw_override_warn_frame.setVisible(False)
        ov.addWidget(self._hw_override_warn_frame)

        self._overrides_list = QListWidget()
        self._overrides_list.setMaximumHeight(140)
        ov.addWidget(self._overrides_list)
        ov_btns = QHBoxLayout()
        add_btn = QPushButton(I18n.translate('ui', 'eq_add_override'))
        add_btn.clicked.connect(self._add_override)
        rm_btn  = QPushButton(I18n.translate('ui', 'eq_remove_override'))
        rm_btn.clicked.connect(self._remove_override)
        ov_btns.addWidget(add_btn)
        ov_btns.addWidget(rm_btn)
        ov_btns.addStretch()
        ov.addLayout(ov_btns)
        cl.addWidget(ov_box)

        # Apply button (outside scroll)
        self._apply_btn = QPushButton(I18n.translate('ui', 'eq_apply'))
        self._apply_btn.clicked.connect(self._apply)
        outer.addWidget(self._apply_btn)

    def showEvent(self, event) -> None:  # noqa: N802
        super().showEvent(event)
        # On re-show, allow _on_eq_settings to re-sync the saved state.
        # _initial_load_done stays True so _on_presets won't reset combos.
        self.refresh()

    def refresh(self) -> None:
        DbusWrapper.request_eq_capabilities(self.sig_eq_capabilities)
        DbusWrapper.request_eq_settings(self.sig_eq_settings)
        DbusWrapper.request_eq_presets(self.sig_presets)
        DbusWrapper.request_steam_games(self.sig_steam_games)

    # ------------------------------------------------------------------
    # Data handlers
    # ------------------------------------------------------------------

    def _on_eq_capabilities(self, caps: dict) -> None:
        available = bool(caps.get('ladspa_available', True))
        plugin = caps.get('ladspa_plugin', 'mbeq_1197')
        has_hw_eq = bool(caps.get('has_hw_eq', False))
        self._ladspa_available = available
        self._has_hw_eq = has_hw_eq
        self._ladspa_frame.setVisible(not available)
        if not available:
            msg = '\n'.join([
                I18n.translate('ui', 'eq_ladspa_unavailable'),
                '',
                I18n.translate('ui', 'eq_ladspa_install_hint'),
                '  ' + I18n.translate('ui', 'eq_ladspa_install_fedora'),
                '  ' + I18n.translate('ui', 'eq_ladspa_install_debian'),
                '  ' + I18n.translate('ui', 'eq_ladspa_install_arch'),
            ])
            self._ladspa_warn_label.setText(msg)
            logger.warning('LADSPA plugin %r not found — EQ controls disabled', plugin)
        self._hw_eq_panel.setVisible(has_hw_eq)
        self._media_section.set_hw_eq_visible(has_hw_eq)
        self._chat_section.set_hw_eq_visible(has_hw_eq)
        if has_hw_eq:
            backend = caps.get('hw_override_backend', 'unsupported')
            reason = caps.get('hw_override_unsupported_reason') if backend == 'unsupported' else None
            self._media_section.set_hw_override_note(reason)
            self._chat_section.set_hw_override_note(reason)
            if reason:
                self._hw_override_warn_label.setText(
                    I18n.translate('ui', 'eq_hw_override_warn') + '\n' + reason
                )
            self._hw_override_warn_frame.setVisible(bool(reason))
        else:
            self._hw_override_warn_frame.setVisible(False)
        self._media_section.setEnabled(available)
        self._chat_section.setEnabled(available)
        self._apply_btn.setEnabled(available)

    def _retry_ladspa_check(self) -> None:
        DbusWrapper.request_eq_capabilities(self.sig_eq_capabilities)

    def _on_hw_apply_result(self, result: dict) -> None:
        if result.get('ok'):
            self._hw_status_label.setText(I18n.translate('ui', 'eq_hw_status_ok'))
        else:
            msg = I18n.translate('ui', 'eq_hw_status_error').format(result.get('error', ''))
            self._hw_status_label.setText(msg)

    def _on_eq_settings(self, settings: dict) -> None:
        self._pending_settings = settings
        self._overrides = settings.get('app_overrides', [])
        self._refresh_overrides_list()
        # Always re-sync sections when settings arrive (initial load or
        # explicit refresh after navigating back to this page).
        self._apply_to_sections()

    def on_hw_settings(self, settings: dict) -> None:
        """Called when D-Bus settings arrive; populates factory presets in combo."""
        mapping: dict = (
            settings.get('settings_config', {})
            .get('eq_preset', {})
            .get('values_mapping', {})
        )
        # slot 4 is "custom" (written by software path); all others are factory presets
        self._hw_preset_mapping = {int(k): v for k, v in mapping.items()}
        self._factory_presets = {k: v for k, v in self._hw_preset_mapping.items() if v != 'custom'}
        self._active_hw_preset_idx = settings.get('device', {}).get('eq_preset')
        self._rebuild_hw_combo()

    def _rebuild_hw_combo(self) -> None:
        """Rebuild _hw_preset_combo: factory presets, separator, software presets."""
        current = self._hw_preset_combo.currentData()
        self._hw_preset_combo.blockSignals(True)
        self._hw_preset_combo.clear()

        for idx in sorted(self._factory_presets):
            name = self._factory_presets[idx]
            label = I18n.translate('settings_values', name)
            self._hw_preset_combo.addItem(label, idx)

        if self._factory_presets and self._presets:
            sep_idx = self._hw_preset_combo.count()
            self._hw_preset_combo.insertSeparator(sep_idx)

        for name in self._presets:
            self._hw_preset_combo.addItem(name, name)

        # Restore previous selection, or select current active factory preset
        restored = self._hw_preset_combo.findData(current)
        if restored >= 0:
            self._hw_preset_combo.setCurrentIndex(restored)
        elif self._active_hw_preset_idx is not None:
            active = self._hw_preset_combo.findData(self._active_hw_preset_idx)
            if active >= 0:
                self._hw_preset_combo.setCurrentIndex(active)

        self._hw_preset_combo.blockSignals(False)
        self._update_hw_active_label()

    def _update_hw_active_label(self) -> None:
        if self._active_hw_preset_idx is not None:
            name = self._hw_preset_mapping.get(self._active_hw_preset_idx, str(self._active_hw_preset_idx))
            label = I18n.translate('settings_values', name)
        else:
            label = ''
        self._hw_active_label.setText(label)

    def _on_hw_apply_clicked(self) -> None:
        data = self._hw_preset_combo.currentData()
        if isinstance(data, int):
            self._active_hw_preset_idx = data
            self._update_hw_active_label()
            DbusWrapper.apply_factory_eq_preset(data, self.sig_hw_apply)
        elif isinstance(data, str):
            DbusWrapper.apply_hw_eq_preset(data, self.sig_hw_apply)

    def _on_presets(self, presets: list) -> None:
        self._presets = {p['name']: p for p in presets}
        self._rebuild_hw_combo()
        if not self._initial_load_done:
            # First time: both settings+presets now available — do full sync.
            self._initial_load_done = True
            self._apply_to_sections()
        else:
            # Subsequent refresh (after save/delete): only update the list,
            # keep the user's current selection.
            self._media_section.set_presets(self._presets)
            self._chat_section.set_presets(self._presets)

    def _on_steam_games(self, games: list) -> None:
        self._steam_games = games

    def _apply_to_sections(self) -> None:
        self._media_section.load_settings(
            self._pending_settings.get('media', {}), self._presets)
        self._chat_section.load_settings(
            self._pending_settings.get('chat', {}), self._presets)

    def _on_preset_saved(self, preset: dict, channel: str) -> None:
        is_new = preset['name'] not in self._presets
        DbusWrapper.save_eq_preset(preset)
        # Update local cache so sections don't reload stale bands from daemon.
        self._presets[preset['name']] = {
            **self._presets.get(preset['name'], {}),
            'bands': preset['bands'],
            'mode': preset.get('mode', 'simple'),
        }
        self._apply_channel(channel)
        if is_new:
            QTimer.singleShot(400, lambda: DbusWrapper.request_eq_presets(self.sig_presets))

    def _on_preset_deleted(self, name: str, channel: str) -> None:
        DbusWrapper.delete_eq_preset(name)
        self._apply_channel(channel)
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
            hw_idx = o.get('hw_preset_idx')
            if hw_idx is not None:
                hw_name = self._hw_preset_mapping.get(hw_idx, str(hw_idx))
                preset = I18n.translate('settings_values', hw_name)
            else:
                preset = o.get('preset_name') or 'flat'
            channel = o.get('channel', 'media')
            self._overrides_list.addItem(f'{src}  →  {preset} ({channel})')

    def _add_override(self) -> None:
        dlg = QAddOverrideDialog(list(self._presets.keys()), self._factory_presets,
                                 self._steam_games, self)
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

    def _schedule_apply(self, channel: str) -> None:
        if not self._initial_load_done:
            return
        if channel == 'media':
            self._apply_timer_media.start()
        else:
            self._apply_timer_chat.start()

    def _apply_channel(self, channel: str) -> None:
        if not self._initial_load_done or not self._ladspa_available:
            return
        section = self._media_section if channel == 'media' else self._chat_section
        DbusWrapper.set_channel_eq_settings(channel, section.get_settings(), self._overrides)

    def _apply(self) -> None:
        """Apply both channels — used only by the explicit Apply button."""
        if not self._initial_load_done or not self._ladspa_available:
            return
        self._apply_timer_media.stop()
        self._apply_timer_chat.stop()
        DbusWrapper.set_eq_settings({
            'media': self._media_section.get_settings(),
            'chat':  self._chat_section.get_settings(),
            'app_overrides': self._overrides,
        })
