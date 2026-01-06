import asyncio
import logging
import signal
from types import FrameType

from linux_arctis_manager.dbus_service import DbusManager


async def main():
    logging.basicConfig(level=logging.INFO)

    dbus_manager = DbusManager.getInstance()
    await dbus_manager.start()

    await dbus_manager.wait_for_stop()

def sigterm_handler(
        sig: int,
        frame: FrameType|None = None
    ) -> None:
    DbusManager.getInstance().stop()

signal.signal(signal.SIGINT, sigterm_handler)
signal.signal(signal.SIGTERM, sigterm_handler)

asyncio.run(main())
