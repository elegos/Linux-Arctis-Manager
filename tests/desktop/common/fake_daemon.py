"""Stands in for the daemon: owns its D-Bus name and serves the sample data of
the calls the GUI and the tray shells need to draw themselves. Every other call
gets a D-Bus error, which the clients only log.
"""

# dbus-next reads D-Bus signatures from the annotations, and `sample_data` is
# found next to this script.
# pyright: reportUndefinedVariable=false, reportMissingImports=false

import asyncio
import json
import sys
from pathlib import Path

# First on purpose: importing the package is what lets `dbus_next` be imported
# on Python 3.15.
import linux_arctis_manager  # noqa: F401  # isort: skip
from dbus_next.aio.message_bus import MessageBus
from dbus_next.service import ServiceInterface, method, signal

from linux_arctis_manager import constants as c
from linux_arctis_manager.utils import project_version

sys.path.insert(0, str(Path(__file__).parent))
import sample_data


class Status(ServiceInterface):
    @method()
    def GetStatus(self) -> 's':  # noqa: F821
        return json.dumps(sample_data.STATUS)

    @method()
    def GetCurrentDevice(self) -> 's':  # noqa: F821
        return json.dumps({})

    @signal()
    def StatusChanged(self) -> 's':  # noqa: F821
        return json.dumps(sample_data.STATUS)


class Settings(ServiceInterface):
    @method()
    def GetVersion(self) -> 's':  # noqa: F821
        # The UI's own version, so it does not try to restart the service.
        return project_version()

    @method()
    def GetSettings(self) -> 's':  # noqa: F821
        return json.dumps(sample_data.SETTINGS)

    @method()
    def GetListOptions(self, _name: 's') -> 's':  # noqa: F821
        return json.dumps([])

    @signal()
    def SettingsChanged(self) -> 's':  # noqa: F821
        return json.dumps(sample_data.SETTINGS)


class NoiseCancelling(ServiceInterface):
    @method(name='GetNCSettings')
    def get_nc_settings(self) -> 's':  # noqa: F821
        return json.dumps({'preset': 'off'})


async def main() -> None:
    bus = await MessageBus().connect()
    bus.export(c.DBUS_STATUS_OBJECT_PATH, Status(c.DBUS_STATUS_INTERFACE_NAME))
    bus.export(c.DBUS_SETTINGS_OBJECT_PATH, Settings(c.DBUS_SETTINGS_INTERFACE_NAME))
    bus.export(c.DBUS_NC_OBJECT_PATH, NoiseCancelling(c.DBUS_NC_INTERFACE_NAME))
    await bus.request_name(c.DBUS_BUS_NAME)
    await bus.wait_for_disconnect()


if __name__ == '__main__':
    asyncio.run(main())
