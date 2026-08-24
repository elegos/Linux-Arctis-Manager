"""Tests for DbusWrapper reconnection and periodic-refresh behaviour."""
import asyncio
import json
import sys
from unittest.mock import AsyncMock, MagicMock, patch, call

import pytest
from PySide6.QtWidgets import QApplication

# One QApplication for the whole module (required for QObject subclasses).
_app = QApplication.instance() or QApplication(sys.argv)


def _make_wrapper():
    """Return a fresh DbusWrapper with DBus I/O stubbed out."""
    from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
    wrapper = DbusWrapper()
    # Pre-populate signal spy lists (QSignalSpy-style manual capture)
    wrapper._captured_status: list = []
    wrapper.sig_status.connect(lambda s: wrapper._captured_status.append(s))
    return wrapper


# ── _request_status_async ────────────────────────────────────────────────────


def _run(coro):
    return asyncio.run(coro)


def test_request_status_emits_parsed_json():
    """request_status emits sig_status with the parsed JSON dict from GetStatus."""
    payload = json.dumps({"wireless": {"transparency_mode": {"type": "label", "value": "anc"}}})

    mock_iface = AsyncMock()
    mock_iface.call_get_status = AsyncMock(return_value=payload)

    mock_bus = AsyncMock()
    mock_bus.introspect = AsyncMock(return_value=MagicMock())
    mock_bus.get_proxy_object = MagicMock(return_value=MagicMock(
        get_interface=MagicMock(return_value=mock_iface)
    ))

    wrapper = _make_wrapper()

    with patch("linux_arctis_manager.gui.dbus_wrapper.MessageBus") as MockBus:
        MockBus.return_value.connect = AsyncMock(return_value=mock_bus)
        _run(wrapper._request_status_async())

    assert len(wrapper._captured_status) == 1
    assert wrapper._captured_status[0] == json.loads(payload)


def test_request_status_creates_fresh_connection_each_call():
    """Each request_status call creates a new MessageBus connection (no stale cache)."""
    mock_iface = AsyncMock()
    mock_iface.call_get_status = AsyncMock(return_value="{}")

    mock_bus = AsyncMock()
    mock_bus.introspect = AsyncMock(return_value=MagicMock())
    mock_bus.get_proxy_object = MagicMock(return_value=MagicMock(
        get_interface=MagicMock(return_value=mock_iface)
    ))

    wrapper = _make_wrapper()

    with patch("linux_arctis_manager.gui.dbus_wrapper.MessageBus") as MockBus:
        MockBus.return_value.connect = AsyncMock(return_value=mock_bus)
        _run(wrapper._request_status_async())
        _run(wrapper._request_status_async())

    # MessageBus() was instantiated twice — one per call.
    assert MockBus.call_count == 2


def test_request_status_logs_warning_on_dbus_error():
    """request_status silently logs a warning when the daemon is unavailable."""
    wrapper = _make_wrapper()

    with patch("linux_arctis_manager.gui.dbus_wrapper.MessageBus") as MockBus:
        MockBus.return_value.connect = AsyncMock(side_effect=ConnectionError("no daemon"))
        with patch.object(wrapper.logger, "warning") as mock_warn:
            _run(wrapper._request_status_async())
            assert mock_warn.called

    # No signal emitted on failure.
    assert wrapper._captured_status == []


# ── _register_status_dbus_signal (retry logic) ──────────────────────────────


def test_register_status_signal_retries_after_connection_failure():
    """Signal registration retries when the daemon is temporarily unavailable."""
    connect_calls = 0

    async def fake_connect():
        nonlocal connect_calls
        connect_calls += 1
        if connect_calls == 1:
            raise ConnectionError("daemon down")
        # Second call: return a live bus that never spontaneously disconnects.
        bus = AsyncMock()
        bus.introspect = AsyncMock(return_value=MagicMock())
        iface = MagicMock()
        iface.on_status_changed = MagicMock()
        iface.on_device_connected = MagicMock()
        iface.on_device_disconnected = MagicMock()
        bus.get_proxy_object = MagicMock(return_value=MagicMock(
            get_interface=MagicMock(return_value=iface)
        ))
        # wait_for_disconnect() blocks indefinitely — the stop_future will win.
        async def _never_disconnect():
            await asyncio.sleep(float("inf"))
        bus.wait_for_disconnect = _never_disconnect
        return bus

    wrapper = _make_wrapper()

    async def run_with_stop():
        task = asyncio.create_task(wrapper._register_status_dbus_signal())

        async def stopper():
            # Poll until the signal loop sets stop_status_signal_future (= connected)
            for _ in range(100):
                if (wrapper._stop_status_signal_future is not None and
                        not wrapper._stop_status_signal_future.done()):
                    wrapper._stop_status_signal_future.set_result(None)
                    return
                await asyncio.sleep(0.01)

        await asyncio.gather(task, stopper())

    with patch("linux_arctis_manager.gui.dbus_wrapper.MessageBus") as MockBus, \
         patch("asyncio.sleep", new=AsyncMock()):
        MockBus.return_value.connect = fake_connect
        with patch.object(wrapper, "request_status"):
            _run(run_with_stop())

    # connect() was called twice: once failing, once succeeding.
    assert connect_calls == 2


# ── periodic refresh (QTimer) ────────────────────────────────────────────────


def test_start_creates_periodic_refresh_timer():
    """dbus_wrapper.start() sets up a QTimer that periodically calls request_status."""
    from PySide6.QtCore import QTimer

    wrapper = _make_wrapper()

    # Patch out everything that would actually start threads / DBus.
    with patch.object(wrapper, "request_status") as mock_req, \
         patch.object(wrapper, "request_settings"), \
         patch("linux_arctis_manager.gui.dbus_wrapper.Thread"), \
         patch("linux_arctis_manager.gui.dbus_wrapper.asyncio"):

        wrapper.start()

    assert hasattr(wrapper, "_refresh_timer"), "start() must create _refresh_timer"
    timer = wrapper._refresh_timer
    assert isinstance(timer, QTimer)
    assert timer.isActive(), "_refresh_timer must be started"
    assert timer.interval() > 0, "_refresh_timer interval must be positive"
    # Clean up
    timer.stop()
