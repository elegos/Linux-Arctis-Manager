"""
Native StatusNotifierItem (SNI) and dbusmenu D-Bus implementation.

Replaces QSystemTrayIcon so we can capture the real Activate(x, y)
cursor coordinates that the compositor sends on tray-icon click, and
provide a natively-rendered right-click menu on KDE / GNOME Wayland.

Architecture:
  SniItem (QObject) — public API, emits Qt signals
    starts a background thread with its own asyncio event loop
    _SniInterface  (ServiceInterface) — org.kde.StatusNotifierItem
    _MenuInterface (ServiceInterface) — com.canonical.dbusmenu
"""

# dbus_next's @method/@dbus_property/@signal decorators are annotated with
# D-Bus type-signature strings ('s', 'u', 'a(iiay)', ...), not real Python
# types — no static checker can resolve these as forward references.
# pyright: reportUndefinedVariable=false, reportInvalidTypeForm=false

from __future__ import annotations

import asyncio
import logging
import os
from threading import Thread

from dbus_next.aio.message_bus import MessageBus
from dbus_next.constants import PropertyAccess
from dbus_next.service import ServiceInterface, dbus_property, method, signal
from PySide6.QtCore import QObject, Signal
from PySide6.QtGui import QImage, QPixmap

logger = logging.getLogger('sni_item')

# ── Icon conversion ────────────────────────────────────────────────────────────

def _pixmap_to_sni(pixmap: QPixmap) -> list[tuple[int, int, bytes]]:
    """Convert QPixmap → SNI IconPixmap format a(iiay).

    SNI expects ARGB32 in network (big-endian) byte order.
    Qt stores ARGB32 as 0xAARRGGBB → on LE systems bytes are [B,G,R,A].
    """
    image = pixmap.toImage().convertToFormat(QImage.Format.Format_ARGB32)
    w, h = image.width(), image.height()
    src = bytes(image.bits())
    out = bytearray(len(src))
    for i in range(0, len(src), 4):
        b, g, r, a = src[i], src[i + 1], src[i + 2], src[i + 3]
        out[i], out[i + 1], out[i + 2], out[i + 3] = a, r, g, b
    return [(w, h, bytes(out))]


# ── D-Bus interfaces ───────────────────────────────────────────────────────────

class _SniInterface(ServiceInterface):
    """org.kde.StatusNotifierItem implementation."""

    def __init__(self, owner: SniItem) -> None:
        super().__init__('org.kde.StatusNotifierItem')
        self._owner = owner

    # Properties ──────────────────────────────────────────────────────────────

    @dbus_property(access=PropertyAccess.READ)
    def Category(self) -> s:
        return 'ApplicationStatus'

    @dbus_property(access=PropertyAccess.READ)
    def Id(self) -> s:
        return 'lam'

    @dbus_property(access=PropertyAccess.READ)
    def Title(self) -> s:
        return 'Arctis Manager'

    @dbus_property(access=PropertyAccess.READ)
    def Status(self) -> s:
        return 'Active'

    @dbus_property(access=PropertyAccess.READ)
    def WindowId(self) -> u:
        return 0

    @dbus_property(access=PropertyAccess.READ)
    def IconName(self) -> s:
        return ''

    @dbus_property(access=PropertyAccess.READ)
    def IconPixmap(self) -> a(iiay):
        return self._owner._icon_data

    @dbus_property(access=PropertyAccess.READ)
    def OverlayIconName(self) -> s:
        return ''

    @dbus_property(access=PropertyAccess.READ)
    def OverlayIconPixmap(self) -> a(iiay):
        return []

    @dbus_property(access=PropertyAccess.READ)
    def AttentionIconName(self) -> s:
        return ''

    @dbus_property(access=PropertyAccess.READ)
    def AttentionIconPixmap(self) -> a(iiay):
        return []

    @dbus_property(access=PropertyAccess.READ)
    def AttentionMovieName(self) -> s:
        return ''

    @dbus_property(access=PropertyAccess.READ)
    def ToolTip(self) -> '(sa(iiay)ss)':  # noqa: F722 # pyright: ignore
        return ['', [], 'Arctis Manager', '']

    @dbus_property(access=PropertyAccess.READ)
    def ItemIsMenu(self) -> b:
        return False

    @dbus_property(access=PropertyAccess.READ)
    def Menu(self) -> o:
        return '/'

    # Methods ─────────────────────────────────────────────────────────────────

    @method()
    async def Activate(self, x: i, y: i):
        self._owner.sig_activate.emit(x, y)

    @method()
    async def ContextMenu(self, x: i, y: i):
        self._owner.sig_activate.emit(x, y)

    @method()
    async def SecondaryActivate(self, x: i, y: i):
        pass

    @method()
    async def Scroll(self, delta: i, orientation: s):
        pass

    @method()
    async def ProvideXdgActivationToken(self, token: s):
        self._owner._xdg_token = token

    # Signals ─────────────────────────────────────────────────────────────────

    @signal()
    def NewTitle(self):
        pass

    @signal()
    def NewIcon(self):
        pass

    @signal()
    def NewAttentionIcon(self):
        pass

    @signal()
    def NewOverlayIcon(self):
        pass

    @signal()
    def NewToolTip(self):
        pass

    @signal()
    def NewStatus(self, status: str) -> s:
        return status


# ── Public class ───────────────────────────────────────────────────────────────

class SniItem(QObject):
    """Manages the native SNI tray icon.

    Emits:
        sig_activate(x, y)   — tray icon clicked (compositor coordinates)
    """

    sig_activate = Signal(int, int)

    def __init__(
        self,
        icon_pixmap: QPixmap,
        parent: QObject | None = None,
    ) -> None:
        super().__init__(parent)
        self._icon_data = _pixmap_to_sni(icon_pixmap)
        self._loop: asyncio.AbstractEventLoop | None = None
        self._stop_event: asyncio.Event | None = None
        self._thread: Thread | None = None
        self._xdg_token: str | None = None

    def start(self) -> None:
        self._thread = Thread(target=lambda: asyncio.run(self._run()), daemon=True)
        self._thread.start()

    def stop(self) -> None:
        if self._loop and self._stop_event:
            self._loop.call_soon_threadsafe(self._stop_event.set)

    async def _run(self) -> None:
        self._loop = asyncio.get_running_loop()
        self._stop_event = asyncio.Event()

        try:
            bus = await MessageBus().connect()
        except Exception as exc:
            logger.error('SNI: cannot connect to session bus: %s', exc)
            return

        sni_iface = _SniInterface(self)

        pid      = os.getpid()
        svc_name = f'org.kde.StatusNotifierItem-{pid}-1'

        try:
            await bus.request_name(svc_name)
        except Exception as exc:
            logger.error('SNI: cannot request service name %s: %s', svc_name, exc)
            bus.disconnect()
            return

        bus.export('/StatusNotifierItem', sni_iface)

        # Register with the StatusNotifierWatcher (KDE or freedesktop fallback).
        registered = False
        for watcher_svc, watcher_iface in (
            ('org.kde.StatusNotifierWatcher',        'org.kde.StatusNotifierWatcher'),
            ('org.freedesktop.StatusNotifierWatcher','org.freedesktop.StatusNotifierWatcher'),
        ):
            try:
                intro  = await bus.introspect(watcher_svc, '/StatusNotifierWatcher')
                proxy  = bus.get_proxy_object(watcher_svc, '/StatusNotifierWatcher', intro)
                iface  = proxy.get_interface(watcher_iface)
                # dbus_next builds ProxyInterface methods dynamically from
                # introspection XML — no static stub can see them.
                await iface.call_register_status_notifier_item(svc_name)  # pyright: ignore[reportAttributeAccessIssue]
                logger.info('SNI: registered with %s', watcher_svc)
                registered = True
                break
            except Exception as exc:
                logger.debug('SNI: watcher %s not available: %s', watcher_svc, exc)

        if not registered:
            logger.warning('SNI: no StatusNotifierWatcher found; icon may not appear')

        await self._stop_event.wait()
        bus.disconnect()
