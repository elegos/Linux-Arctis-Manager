from __future__ import annotations

import math
import logging
import os

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
    def __init__(self, preset_names: list[str], steam_games: list[dict],
                 running_streams: list[str],
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
            I18n.translate('ui', 'eq_match_stream'),
            I18n.translate('ui', 'eq_match_executable'),
            I18n.translate('ui', 'eq_match_steam'),
        ])
        self._type_combo.currentIndexChanged.connect(self._on_type_changed)
        layout.addRow(I18n.translate('ui', 'eq_match_by'), self._type_combo)

        # Stacked value widget (page 0 = stream combo, page 1 = exec + browse, page 2 = steam).
        self._val_label = QLabel(I18n.translate('ui', 'eq_match_stream'))
        self._val_stack = QStackedWidget()

        # Page 0: select-only combo of running application names.
        self._stream_combo = QComboBox()
        self._stream_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        for name in running_streams:
            self._stream_combo.addItem(name)
        self._val_stack.addWidget(self._stream_combo)

        # Page 1: line edit + Browse button for executables.
        exec_widget = QWidget()
        exec_row = QHBoxLayout()
        exec_row.setContentsMargins(0, 0, 0, 0)
        exec_widget.setLayout(exec_row)
        self._exec_input = QLineEdit()
        browse_btn = QPushButton(I18n.translate('ui', 'eq_browse'))
        browse_btn.clicked.connect(self._browse_executable)
        exec_row.addWidget(self._exec_input)
        exec_row.addWidget(browse_btn)
        self._val_stack.addWidget(exec_widget)

        # Page 2: steam game combo.
        self._game_combo = QComboBox()
        self._game_combo.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        for g in self._unique_games:
            self._game_combo.addItem(g['name'], g['app_id'])
        self._val_stack.addWidget(self._game_combo)

        layout.addRow(self._val_label, self._val_stack)

        self._preset_combo = QComboBox()
        self._preset_combo.addItem(I18n.translate('ui', 'eq_flat'))
        for name in preset_names:
            self._preset_combo.addItem(name)
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
            I18n.translate('ui', 'eq_match_stream'),
            I18n.translate('ui', 'eq_match_executable'),
            I18n.translate('ui', 'eq_match_steam'),
        ]
        self._val_label.setText(labels[idx])
        self._val_stack.setCurrentIndex(idx)

    def _browse_executable(self) -> None:
        path, _ = QFileDialog.getOpenFileName(self, I18n.translate('ui', 'eq_match_executable'))
        if path:
            self._exec_input.setText(os.path.basename(path))

    def get_override(self) -> dict:
        type_idx = self._type_combo.currentIndex()
        mt = ('stream', 'executable', 'steam')[type_idx]
        flat_lbl = I18n.translate('ui', 'eq_flat')
        preset_txt = self._preset_combo.currentText()
        gi = self._game_combo.currentIndex()
        if mt == 'stream':
            value = self._stream_combo.currentText().strip()
            steam_app_id = None
            steam_game_name = ''
        elif mt == 'executable':
            value = os.path.basename(self._exec_input.text().strip())
            steam_app_id = None
            steam_game_name = ''
        else:
            value = ''
            steam_app_id = self._unique_games[gi]['app_id'] if 0 <= gi < len(self._unique_games) else None
            steam_game_name = self._unique_games[gi]['name'] if 0 <= gi < len(self._unique_games) else ''
        return {
            'matcher_type': mt,
            'value': value,
            'steam_app_id': steam_app_id,
            'steam_game_name': steam_game_name,
            'preset_name': '' if preset_txt == flat_lbl else preset_txt,
            'channel': 'chat' if self._channel_combo.currentIndex() == 1 else 'media',
        }


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
    sig_running_streams  = Signal(object)

    def __init__(self, parent: QWidget) -> None:
        super().__init__(parent)
        self._pending_settings: dict = {}
        self._presets: dict[str, dict] = {}
        self._overrides: list[dict] = []
        self._steam_games: list[dict] = []
        self._running_streams: list[str] = []
        self._initial_load_done = False   # prevents preset-list refresh from resetting the combo
        self._ladspa_available = True
        self._has_hw_eq = False

        self._apply_timer = QTimer(self)
        self._apply_timer.setSingleShot(True)
        self._apply_timer.setInterval(400)
        self._apply_timer.timeout.connect(self._apply)

        self.sig_eq_settings.connect(self._on_eq_settings)
        self.sig_presets.connect(self._on_presets)
        self.sig_steam_games.connect(self._on_steam_games)
        self.sig_eq_capabilities.connect(self._on_eq_capabilities)
        self.sig_running_streams.connect(self._on_running_streams)

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
        self._media_section.preset_saved.connect(self._on_preset_saved)
        self._media_section.preset_deleted.connect(self._on_preset_deleted)
        self._media_section.preset_selection_changed.connect(self._schedule_apply)
        cl.addWidget(self._media_section)

        self._chat_section = QChannelSection('chat', content)
        self._chat_section.preset_saved.connect(self._on_preset_saved)
        self._chat_section.preset_deleted.connect(self._on_preset_deleted)
        self._chat_section.preset_selection_changed.connect(self._schedule_apply)
        cl.addWidget(self._chat_section)

        # App overrides
        ov_box = QGroupBox(I18n.translate('ui', 'eq_app_overrides'))
        ov = QVBoxLayout()
        ov_box.setLayout(ov)
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
        DbusWrapper.request_running_streams(self.sig_running_streams)

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
        self._media_section.set_hw_eq_visible(has_hw_eq)
        self._chat_section.set_hw_eq_visible(has_hw_eq)
        self._media_section.setEnabled(available)
        self._chat_section.setEnabled(available)
        self._apply_btn.setEnabled(available)

    def _retry_ladspa_check(self) -> None:
        DbusWrapper.request_eq_capabilities(self.sig_eq_capabilities)

    def _on_eq_settings(self, settings: dict) -> None:
        self._pending_settings = settings
        self._overrides = settings.get('app_overrides', [])
        self._refresh_overrides_list()
        # Always re-sync sections when settings arrive (initial load or
        # explicit refresh after navigating back to this page).
        self._apply_to_sections()

    def _on_presets(self, presets: list) -> None:
        self._presets = {p['name']: p for p in presets}
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

    def _on_running_streams(self, streams: list) -> None:
        self._running_streams = streams

    def _apply_to_sections(self) -> None:
        self._media_section.load_settings(
            self._pending_settings.get('media', {}), self._presets)
        self._chat_section.load_settings(
            self._pending_settings.get('chat', {}), self._presets)

    def _on_preset_saved(self, preset: dict) -> None:
        DbusWrapper.save_eq_preset(preset)
        # Apply immediately so the new/updated preset takes effect in audio.
        self._apply()
        QTimer.singleShot(400, lambda: DbusWrapper.request_eq_presets(self.sig_presets))

    def _on_preset_deleted(self, name: str) -> None:
        DbusWrapper.delete_eq_preset(name)
        # If the deleted preset was active, revert to flat.
        self._apply()
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
            preset  = o.get('preset_name') or 'flat'
            channel = o.get('channel', 'media')
            self._overrides_list.addItem(f'{src}  →  {preset} ({channel})')

    def _add_override(self) -> None:
        DbusWrapper.request_running_streams(self.sig_running_streams)
        dlg = QAddOverrideDialog(list(self._presets.keys()), self._steam_games,
                                 self._running_streams, self)
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

    def _schedule_apply(self) -> None:
        if self._initial_load_done:
            self._apply_timer.start()   # restarts the timer if already running

    def _apply(self) -> None:
        if not self._initial_load_done or not self._ladspa_available:
            return
        self._apply_timer.stop()        # cancel any pending debounce
        DbusWrapper.set_eq_settings({
            'media': self._media_section.get_settings(),
            'chat':  self._chat_section.get_settings(),
            'app_overrides': self._overrides,
        })
