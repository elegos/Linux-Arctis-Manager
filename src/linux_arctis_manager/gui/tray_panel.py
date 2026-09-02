from __future__ import annotations

import ctypes
import logging

from PySide6.QtCore import (
    QByteArray,
    QEvent,
    QObject,
    QPoint,
    QSize,
    Qt,
    QTimer,
    Signal,
)
from PySide6.QtGui import QCursor
from PySide6.QtWidgets import (
    QApplication,
    QDialog,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSizePolicy,
    QTabWidget,
    QVBoxLayout,
    QWidget,
)

from linux_arctis_manager.gui.tray_quick_settings_tab import QTrayQuickSettingsTab
from linux_arctis_manager.gui.tray_status_tab import QTrayStatusTab
from linux_arctis_manager.i18n import I18n

logger = logging.getLogger('tray_panel')
_T = I18n.translate

_PANEL_WIDTH  = 360
_PANEL_HEIGHT = 440


class _OutsideClickFilter(QObject):
    """Catches within-app clicks outside the panel to close it (X11 / XWayland)."""

    def __init__(self, panel: QTrayPanel) -> None:
        super().__init__(panel)
        self._panel = panel

    def eventFilter(self, obj: QObject, event: QEvent) -> bool:
        if event.type() == QEvent.Type.MouseButtonPress and self._panel.isVisible():
            if self._panel.findChildren(QDialog):
                return False
            pos = QCursor.pos()
            if not self._panel.geometry().contains(pos):
                self._panel.hide()
        return False


class QTrayPanel(QWidget):
    """Frameless popup panel shown on tray-icon left-click.

    On Wayland (xdg-toplevel) the compositor ultimately controls window
    placement; move() is a hint that KWin may or may not honour.  The
    native SNI implementation in sni_item.py supplies real Activate(x, y)
    compositor coordinates, which are used as the positioning hint.
    """

    sig_open_main = Signal()
    sig_exit = Signal()

    def __init__(self, parent: QWidget | None = None) -> None:
        super().__init__(parent, Qt.WindowType.FramelessWindowHint | Qt.WindowType.Tool)
        self.setAttribute(Qt.WidgetAttribute.WA_TranslucentBackground)
        self.setFixedSize(_PANEL_WIDTH, _PANEL_HEIGHT)

        self._suppress_hide = False
        self._blur_set = False

        self._device_name: str = ''
        self._device_pid: int | None = None
        # Tri-state: None = startup, True = connected, False = disconnected
        self._connection_state: bool | None = None

        root = QVBoxLayout()
        root.setContentsMargins(0, 0, 0, 0)
        root.setSpacing(0)
        self.setLayout(root)

        # ── Header ─────────────────────────────────────────────────────────────
        header = QWidget()
        header.setObjectName('TrayPanelHeader')
        header.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        hlay = QHBoxLayout()
        hlay.setContentsMargins(12, 10, 12, 10)
        header.setLayout(hlay)

        self._device_label = QLabel(_T('ui', 'no_device_detected'))
        self._device_label.setStyleSheet('font-weight: bold; font-size: 13px;')
        hlay.addWidget(self._device_label)
        hlay.addStretch()

        root.addWidget(header)

        # ── Tabs ───────────────────────────────────────────────────────────────
        self._tabs = QTabWidget()
        self._tabs.setDocumentMode(True)
        self._tabs.setStyleSheet(
            'QTabWidget::pane { background: transparent; border: none; }'
            'QTabBar::tab { background: rgba(128,128,128,30); border-radius: 4px;'
            ' padding: 4px 12px; margin-right: 2px; }'
            'QTabBar::tab:selected { background: rgba(128,128,128,70); }'
            'QTabBar::tab:hover:!selected { background: rgba(128,128,128,45); }'
        )
        root.addWidget(self._tabs, 1)

        self._status_tab = QTrayStatusTab()
        self._tabs.addTab(self._status_tab, _T('ui', 'status'))

        self._qs_tab = QTrayQuickSettingsTab()
        self._tabs.addTab(self._qs_tab, _T('ui', 'quick_settings'))

        # ── Footer ─────────────────────────────────────────────────────────────
        footer = QWidget()
        flay = QHBoxLayout()
        flay.setContentsMargins(8, 6, 8, 6)
        footer.setLayout(flay)

        open_btn = QPushButton(_T('ui', 'open_app'))
        open_btn.clicked.connect(self._on_open)
        flay.addWidget(open_btn)
        flay.addStretch()

        exit_btn = QPushButton(_T('ui', 'exit'))
        exit_btn.clicked.connect(self._on_exit)
        flay.addWidget(exit_btn)

        root.addWidget(footer)

        # ── Hide on app deactivation (Wayland + X11) ───────────────────────────
        app = QApplication.instance()
        assert app is not None, 'a running QWidget always has a QApplication instance'
        app.applicationStateChanged.connect(self._on_app_state_changed)  # pyright: ignore[reportAttributeAccessIssue]
        self._click_filter = _OutsideClickFilter(self)
        app.installEventFilter(self._click_filter)

    def showEvent(self, event) -> None:
        super().showEvent(event)
        if not self._blur_set:
            self._blur_set = True
            self._try_set_blur()

    def _try_set_blur(self) -> None:
        """Request KWin to blur behind this window via the xcb _KDE_NET_WM_BLUR_BEHIND_REGION atom."""
        try:
            xcb = ctypes.CDLL('libxcb.so.1')
            from PySide6.QtGui import QGuiApplication

            ni = QGuiApplication.platformNativeInterface()  # pyright: ignore[reportAttributeAccessIssue]
            conn = ni.nativeResourceForIntegration(QByteArray(b'connection'))
            if not conn:
                return

            class _Cookie(ctypes.Structure):
                _fields_ = [('sequence', ctypes.c_uint32)]

            class _Reply(ctypes.Structure):
                _fields_ = [
                    ('response_type', ctypes.c_uint8),
                    ('pad0',          ctypes.c_uint8),
                    ('sequence',      ctypes.c_uint16),
                    ('length',        ctypes.c_uint32),
                    ('atom',          ctypes.c_uint32),
                ]

            xcb.xcb_intern_atom.restype = _Cookie
            xcb.xcb_intern_atom_reply.restype = ctypes.POINTER(_Reply)

            conn_ptr = ctypes.c_void_p(int(conn))
            name = b'_KDE_NET_WM_BLUR_BEHIND_REGION'
            cookie = xcb.xcb_intern_atom(conn_ptr, ctypes.c_uint8(0),
                                          ctypes.c_uint16(len(name)), name)
            reply_ptr = xcb.xcb_intern_atom_reply(conn_ptr, cookie, None)
            if not reply_ptr:
                return
            atom = reply_ptr.contents.atom
            ctypes.cdll.LoadLibrary('libc.so.6').free(reply_ptr)

            xcb.xcb_change_property(
                conn_ptr,
                ctypes.c_uint8(0),
                ctypes.c_uint32(int(self.winId())),
                ctypes.c_uint32(atom),
                ctypes.c_uint32(6),   # XCB_ATOM_CARDINAL
                ctypes.c_uint8(32),
                ctypes.c_uint32(0),   # 0 elements = blur entire window
                None,
            )
            xcb.xcb_flush(conn_ptr)
        except Exception:
            pass

    def sizeHint(self) -> QSize:
        return QSize(_PANEL_WIDTH, _PANEL_HEIGHT)

    # ── Show / hide ────────────────────────────────────────────────────────────

    def toggle_near(self, x: int = 0, y: int = 0) -> None:
        if self.isVisible():
            self.hide()
        else:
            self._show_near(x, y)

    def _show_near(self, hint_x: int = 0, hint_y: int = 0) -> None:
        """Position the panel near (hint_x, hint_y) — the SNI Activate coordinates.

        On X11 / XWayland these are the real cursor coordinates and move() works.
        On Wayland (xdg-toplevel) the compositor may ignore the move() hint and
        centre the window; that is a Wayland / KWin limitation, not a bug here.
        """
        # Resolve the click position: SNI coordinates > cursor > fallback
        click = QPoint(hint_x, hint_y) if hint_x != 0 or hint_y != 0 else QCursor.pos()

        screen = QApplication.screenAt(click) or QApplication.primaryScreen()
        if not screen:
            self.show()
            return

        avail = screen.availableGeometry()

        if click == QPoint(0, 0):
            # Wayland without hint: best-effort top-right corner.
            x = avail.right() - self.width()
            y = avail.top()
        else:
            # Place panel above or below the click, staying within the screen.
            if click.y() > avail.top() + avail.height() // 2:
                y = click.y() - self.height() - 8
            else:
                y = click.y() + 8
            y = max(avail.top(), min(y, avail.bottom() - self.height()))
            x = max(avail.left(), min(click.x() - self.width() + 16, avail.right() - self.width()))

        self.move(QPoint(x, y))
        self.show()
        self.raise_()
        self.activateWindow()

    def _on_app_state_changed(self, state) -> None:
        if (state == Qt.ApplicationState.ApplicationInactive and self.isVisible()
                and not self._suppress_hide and not self.findChildren(QDialog)):
            QTimer.singleShot(80, self._hide_if_still_inactive)

    def _hide_if_still_inactive(self) -> None:
        if QApplication.applicationState() == Qt.ApplicationState.ApplicationInactive:
            self.hide()

    def hideEvent(self, event) -> None:
        super().hideEvent(event)
        self._suppress_hide = False

    # ── Footer actions ─────────────────────────────────────────────────────────

    def _on_open(self) -> None:
        self.hide()
        self.sig_open_main.emit()

    def _on_exit(self) -> None:
        self.hide()
        self.sig_exit.emit()

    # ── Data updates ───────────────────────────────────────────────────────────

    def on_device_connected(self, info: dict) -> None:
        self._device_name = info.get('name', '')
        self._device_pid = info.get('pid')
        self._connection_state = True
        self._device_label.setText(self._device_name or _T('ui', 'no_device_detected'))
        if self._device_pid is not None:
            self._qs_tab.set_device(self._device_pid)

    def on_device_disconnected(self) -> None:
        self._device_name = ''
        self._device_pid = None
        self._connection_state = False
        self._device_label.setText(_T('ui', 'no_device_detected'))

    def on_status(self, status: dict) -> None:
        self._status_tab.update_status(status)
        self._qs_tab.update_status(status)
        if self._connection_state is None:
            if status:
                self._device_label.setText('Arctis Device')
            else:
                self._device_label.setText(_T('ui', 'no_device_detected'))
        elif self._connection_state is True:
            if status:
                self._device_label.setText(self._device_name or 'Arctis Device')
            else:
                self._device_label.setText(_T('ui', 'no_device_detected'))

    def on_settings(self, settings: dict) -> None:
        self._qs_tab.update_settings(settings)

    def on_nc_settings(self, nc: dict) -> None:
        self._qs_tab.update_nc_preset(nc.get('preset', 'off'))
