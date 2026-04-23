import argparse
import asyncio
import logging
import os
import signal
import sys
import time
from pathlib import Path
from types import FrameType

from linux_arctis_manager.core import CoreEngine
from linux_arctis_manager.dbus_service import DbusManager
from linux_arctis_manager.scripts.dbus_awake import DbusAwake
from linux_arctis_manager.utils import project_version


def _pid_file() -> Path:
    runtime_dir = os.environ.get('XDG_RUNTIME_DIR', '/tmp')
    return Path(runtime_dir) / 'lam-daemon.pid'


def _read_existing_pid() -> int | None:
    pid_file = _pid_file()
    if not pid_file.exists():
        return None
    try:
        return int(pid_file.read_text().strip())
    except (ValueError, OSError):
        return None


def _is_running(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True  # process exists but belongs to another user


def _write_pid() -> None:
    _pid_file().write_text(str(os.getpid()))


def _remove_pid() -> None:
    _pid_file().unlink(missing_ok=True)


def check_single_instance(replace: bool) -> None:
    existing_pid = _read_existing_pid()

    if existing_pid is None or not _is_running(existing_pid):
        return

    if not replace:
        print(
            f"error: lam-daemon is already running (PID {existing_pid}). "
            "Use --replace to stop the running instance and start a new one.",
            file=sys.stderr,
        )
        sys.exit(1)

    print(f"Replacing running lam-daemon instance (PID {existing_pid})...", file=sys.stderr)
    try:
        os.kill(existing_pid, signal.SIGTERM)
    except ProcessLookupError:
        return

    for _ in range(100):
        time.sleep(0.1)
        if not _is_running(existing_pid):
            return

    print(
        f"error: lam-daemon (PID {existing_pid}) did not exit within 10 seconds.",
        file=sys.stderr,
    )
    sys.exit(1)


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
        frame: FrameType | None = None
    ) -> None:
    DbusManager.getInstance().stop()


def main():
    parser = argparse.ArgumentParser(description='Linux Arctis Manager daemon')
    parser.add_argument(
        '--replace',
        action='store_true',
        default=False,
        help='Stop the running instance and replace it with this one',
    )
    args = parser.parse_args()

    check_single_instance(args.replace)

    signal.signal(signal.SIGINT, sigterm_handler)
    signal.signal(signal.SIGTERM, sigterm_handler)

    _write_pid()
    try:
        asyncio.run(main_async())
    finally:
        _remove_pid()


if __name__ == '__main__':
    main()
