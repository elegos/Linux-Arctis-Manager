import asyncio
from unittest.mock import AsyncMock, MagicMock, patch

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper


def test_settings_interface_does_not_reuse_status_interface():
    wrapper = DbusWrapper()
    status_iface = MagicMock()
    settings_iface = MagicMock()
    wrapper._status_iface = status_iface

    bus = MagicMock()
    bus.introspect = AsyncMock(return_value=MagicMock())
    proxy = MagicMock()
    proxy.get_interface.return_value = settings_iface
    bus.get_proxy_object.return_value = proxy

    connection = MagicMock()
    connection.connect = AsyncMock(return_value=bus)

    with patch('linux_arctis_manager.gui.dbus_wrapper.MessageBus', return_value=connection):
        assert asyncio.run(wrapper.settings_iface()) is settings_iface

    assert wrapper._status_iface is status_iface
    assert wrapper._settings_iface is settings_iface
