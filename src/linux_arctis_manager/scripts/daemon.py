import asyncio
import logging
import signal
from types import FrameType

from linux_arctis_manager.core import CoreEngine
from linux_arctis_manager.dbus_service import DbusManager


async def main():
    logging.basicConfig(level=logging.INFO, format='[%(levelname)7s] %(name)20s: %(message)s')

    dbus_manager = DbusManager.getInstance()
    core_engine = CoreEngine()
    await dbus_manager.start(core_engine)

    core_loop = asyncio.create_task(core_engine.start())

    await dbus_manager.wait_for_stop()
    await core_loop

def sigterm_handler(
        sig: int,
        frame: FrameType|None = None
    ) -> None:
    DbusManager.getInstance().stop()

signal.signal(signal.SIGINT, sigterm_handler)
signal.signal(signal.SIGTERM, sigterm_handler)

asyncio.run(main())
