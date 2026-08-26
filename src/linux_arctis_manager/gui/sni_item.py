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

from __future__ import annotations

import asyncio
import logging
import os
from threading import Thread

from dbus_next.aio.message_bus import MessageBus
from dbus_next.constants import PropertyAccess
from dbus_next.service import ServiceInterface, Variant, dbus_property, method, signal
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
    return [[w, h, bytes(out)]]


# ── D-Bus interfaces ───────────────────────────────────────────────────────────

class _SniInterface(ServiceInterface):
    """org.kde.StatusNotifierItem implementation."""

    def __init__(self, owner: SniItem) -> None:
        super().__init__('org.kde.StatusNotifierItem')
        self._owner = owner

    # Properties ──────────────────────────────────────────────────────────────

    @dbus_property(access=PropertyAccess.READ)
    def Category(self) -> 's':
        return 'ApplicationStatus'

    @dbus_property(access=PropertyAccess.READ)
    def Id(self) -> 's':
        return 'lam'

    @dbus_property(access=PropertyAccess.READ)
    def Title(self) -> 's':
        return 'Arctis Manager'

    @dbus_property(access=PropertyAccess.READ)
    def Status(self) -> 's':
        return 'Active'

    @dbus_property(access=PropertyAccess.READ)
    def WindowId(self) -> 'u':
        return 0

    @dbus_property(access=PropertyAccess.READ)
    def IconName(self) -> 's':
        return ''

    @dbus_property(access=PropertyAccess.READ)
    def IconPixmap(self) -> 'a(iiay)':
        return self._owner._icon_data

    @dbus_property(access=PropertyAccess.READ)
    def OverlayIconName(self) -> 's':
        return ''

    @dbus_property(access=PropertyAccess.READ)
    def OverlayIconPixmap(self) -> 'a(iiay)':
        return []

    @dbus_property(access=PropertyAccess.READ)
    def AttentionIconName(self) -> 's':
        return ''

    @dbus_property(access=PropertyAccess.READ)
    def AttentionIconPixmap(self) -> 'a(iiay)':
        return []

    @dbus_property(access=PropertyAccess.READ)
    def AttentionMovieName(self) -> 's':
        return ''

    @dbus_property(access=PropertyAccess.READ)
    def ToolTip(self) -> '(sa(iiay)ss)':
        return ['', [], 'Arctis Manager', '']

    @dbus_property(access=PropertyAccess.READ)
    def ItemIsMenu(self) -> 'b':
        return False

    @dbus_property(access=PropertyAccess.READ)
    def Menu(self) -> 'o':
        return '/StatusNotifierMenu'

    # Methods ─────────────────────────────────────────────────────────────────

    @method()
    async def Activate(self, x: 'i', y: 'i'):  # noqa: N802
        self._owner.sig_activate.emit(x, y)

    @method()
    async def ContextMenu(self, x: 'i', y: 'i'):  # noqa: N802
        # KDE renders the dbusmenu natively; nothing to do here.
        pass

    @method()
    async def SecondaryActivate(self, x: 'i', y: 'i'):  # noqa: N802
        pass

    @method()
    async def Scroll(self, delta: 'i', orientation: 's'):  # noqa: N802
        pass

    @method()
    async def ProvideXdgActivationToken(self, token: 's'):  # noqa: N802
        self._owner._xdg_token = token

    # Signals ─────────────────────────────────────────────────────────────────

    @signal()
    def NewTitle(self):  # noqa: N802
        pass

    @signal()
    def NewIcon(self):  # noqa: N802
        pass

    @signal()
    def NewAttentionIcon(self):  # noqa: N802
        pass

    @signal()
    def NewOverlayIcon(self):  # noqa: N802
        pass

    @signal()
    def NewToolTip(self):  # noqa: N802
        pass

    @signal()
    def NewStatus(self, status: str) -> 's':  # noqa: N802
        return status


class _MenuInterface(ServiceInterface):
    """Minimal com.canonical.dbusmenu with three items: Open / Separator / Exit."""

    _REVISION = 1

    def __init__(self, open_label: str, exit_label: str, owner: SniItem) -> None:
        super().__init__('com.canonical.dbusmenu')
        self._open_label = open_label
        self._exit_label = exit_label
        self._owner = owner

    def _layout(self) -> list:
        return [
            0,
            {},
            [
                Variant('(ia{sv}av)', [
                    1,
                    {
                        'label':   Variant('s', self._open_label),
                        'enabled': Variant('b', True),
                        'visible': Variant('b', True),
                    },
                    [],
                ]),
                Variant('(ia{sv}av)', [
                    2,
                    {'type': Variant('s', 'separator')},
                    [],
                ]),
                Variant('(ia{sv}av)', [
                    3,
                    {
                        'label':   Variant('s', self._exit_label),
                        'enabled': Variant('b', True),
                        'visible': Variant('b', True),
                    },
                    [],
                ]),
            ],
        ]

    # Properties ──────────────────────────────────────────────────────────────

    @dbus_property(access=PropertyAccess.READ)
    def Version(self) -> 'u':
        return 3

    @dbus_property(access=PropertyAccess.READ)
    def TextDirection(self) -> 's':
        return 'ltr'

    @dbus_property(access=PropertyAccess.READ)
    def Status(self) -> 's':
        return 'normal'

    @dbus_property(access=PropertyAccess.READ)
    def IconThemePath(self) -> 'as':
        return []

    # Methods ─────────────────────────────────────────────────────────────────

    @method()
    async def GetLayout(  # noqa: N802
        self,
        parent_id: 'i',
        recursion_depth: 'i',
        property_names: 'as',
    ) -> 'u(ia{sv}av)':
        return [self._REVISION, self._layout()]

    @method()
    async def GetGroupProperties(  # noqa: N802
        self, ids: 'ai', property_names: 'as'
    ) -> 'a(ia{sv})':
        return []

    @method()
    async def GetProperty(self, id: 'i', name: 's') -> 'v':  # noqa: N802
        return Variant('s', '')

    @method()
    async def Event(  # noqa: N802
        self, id: 'i', event_id: 's', data: 'v', timestamp: 'u'
    ):
        if event_id == 'clicked':
            if id == 1:
                self._owner.sig_open_app.emit()
            elif id == 3:
                self._owner.sig_exit.emit()

    @method()
    async def EventGroup(self, events: 'a(isvu)') -> 'ai':  # noqa: N802
        for ev_id, event_id, _data, _ts in events:
            if event_id == 'clicked':
                if ev_id == 1:
                    self._owner.sig_open_app.emit()
                elif ev_id == 3:
                    self._owner.sig_exit.emit()
        return []

    @method()
    async def AboutToShow(self, id: 'i') -> 'b':  # noqa: N802
        return False

    @method()
    async def AboutToShowGroup(self, ids: 'ai') -> 'aiai':  # noqa: N802
        return [[], []]

    # Signals ─────────────────────────────────────────────────────────────────

    @signal()
    def ItemsPropertiesUpdated(  # noqa: N802
        self, updated: list, removed: list
    ) -> 'a(ia{sv})a(ias)':
        return [updated, removed]

    @signal()
    def LayoutUpdated(self, revision: int, parent: int) -> 'ui':  # noqa: N802
        return [revision, parent]

    @signal()
    def ItemActivationRequested(self, id: int, timestamp: int) -> 'iu':  # noqa: N802
        return [id, timestamp]


# ── Public class ───────────────────────────────────────────────────────────────

class SniItem(QObject):
    """Manages the native SNI tray icon and its dbusmenu context menu.

    Emits:
        sig_activate(x, y)   — tray icon left-clicked (compositor coordinates)
        sig_open_app         — "Open App" menu item clicked
        sig_exit             — "Exit" menu item clicked
    """

    sig_activate = Signal(int, int)
    sig_open_app = Signal()
    sig_exit = Signal()

    def __init__(
        self,
        icon_pixmap: QPixmap,
        open_label: str,
        exit_label: str,
        parent: QObject | None = None,
    ) -> None:
        super().__init__(parent)
        self._icon_data = _pixmap_to_sni(icon_pixmap)
        self._open_label = open_label
        self._exit_label = exit_label
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

        sni_iface  = _SniInterface(self)
        menu_iface = _MenuInterface(self._open_label, self._exit_label, self)

        pid      = os.getpid()
        svc_name = f'org.kde.StatusNotifierItem-{pid}-1'

        try:
            await bus.request_name(svc_name)
        except Exception as exc:
            logger.error('SNI: cannot request service name %s: %s', svc_name, exc)
            bus.disconnect()
            return

        bus.export('/StatusNotifierItem', sni_iface)
        bus.export('/StatusNotifierMenu', menu_iface)

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
                await iface.call_register_status_notifier_item(svc_name)
                logger.info('SNI: registered with %s', watcher_svc)
                registered = True
                break
            except Exception as exc:
                logger.debug('SNI: watcher %s not available: %s', watcher_svc, exc)

        if not registered:
            logger.warning('SNI: no StatusNotifierWatcher found; icon may not appear')

        await self._stop_event.wait()
        bus.disconnect()
