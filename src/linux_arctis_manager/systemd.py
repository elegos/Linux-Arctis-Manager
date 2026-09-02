import subprocess

from linux_arctis_manager.constants import (
    SYSTEMD_HELPER_SERVICE_NAME,
    SYSTEMD_SERVICE_NAME,
)

_UNITS = (SYSTEMD_HELPER_SERVICE_NAME, SYSTEMD_SERVICE_NAME)


def is_systemd_unit_enabled() -> bool:
    try:
        subprocess.check_call(['systemctl', '--user', 'is-enabled', SYSTEMD_SERVICE_NAME], stdout=subprocess.DEVNULL)
        return True
    except subprocess.CalledProcessError:
        pass

    return False

def ensure_systemd_unit(enable: bool = False, restart: bool = False) -> None:
    """Enable/start (or restart) the packaged lam-daemon + lam-hidraw-helper
    user units. Unlike the v2 daemon, v3 ships its unit files as part of the
    package (see packaging/systemd/user/); nothing is written here — if the
    units aren't installed, the calls below fail with a clear systemctl error
    instead of silently authoring a duplicate, dependency-less unit."""
    if not enable:
        return

    subprocess.run(['systemctl', '--user', 'daemon-reload'], check=True)

    is_active = subprocess.run(['systemctl', '--user', 'is-active', SYSTEMD_SERVICE_NAME], stdout=subprocess.DEVNULL, check=False).returncode == 0

    subprocess.run(['systemctl', '--user', 'enable', *_UNITS], check=True)
    if is_active and restart:
        subprocess.run(['systemctl', '--user', 'restart', *_UNITS], check=True)
    elif not is_active:
        subprocess.run(['systemctl', '--user', 'start', *_UNITS], check=True)
