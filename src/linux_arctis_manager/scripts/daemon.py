import asyncio
import logging
import signal
from types import FrameType

from linux_arctis_manager.core import CoreEngine
from linux_arctis_manager.dbus_service import DbusManager
from linux_arctis_manager.scripts.dbus_awake import DbusAwake
from linux_arctis_manager.utils import project_version


async def main_async():
    logging.basicConfig(level=logging.INFO, format='[%(levelname)7s] %(name)20s: %(message)s')

    logger = logging.getLogger('Daemon')
    logger.info('-------------------------------')
    logger.info('- Arctis Manager is starting. -')
    logger.info(f'-{"v " + project_version():>27}  -')
    logger.info('-------------------------------')

    dbus_manager = DbusManager.getInstance()
    core_engine = CoreEngine()
    await dbus_manager.start(core_engine)

    core_loop = asyncio.create_task(core_engine.start())
    dbus_manager_loop = asyncio.create_task(DbusAwake.get_instance().start(core_engine))

    await dbus_manager.wait_for_stop()
    await core_loop

    dbus_manager_loop.cancel()

def sigterm_handler(
        sig: int,
        frame: FrameType|None = None
    ) -> None:
    DbusManager.getInstance().stop()

def main():
    signal.signal(signal.SIGINT, sigterm_handler)
    signal.signal(signal.SIGTERM, sigterm_handler)

    asyncio.run(main_async())

if __name__ == '__main__':
    main()
