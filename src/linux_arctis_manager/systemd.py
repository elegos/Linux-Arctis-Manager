import shutil
import subprocess
import sys
from pathlib import Path

from linux_arctis_manager.constants import (HOME_SYSTEMD_SERVICE_FOLDER,
                                            SYSTEMD_SERVICE_NAME)


def is_systemd_unit_enabled() -> bool:
    try:
        subprocess.check_call(['systemctl', '--user', 'is-enabled', SYSTEMD_SERVICE_NAME], stdout=subprocess.DEVNULL)
        return True
    except subprocess.CalledProcessError:
        pass

    return False

def ensure_systemd_unit(enable: bool = False, restart: bool = False) -> None:
    path = HOME_SYSTEMD_SERVICE_FOLDER / SYSTEMD_SERVICE_NAME
    path.parent.mkdir(parents=True, exist_ok=True)
    write_systemd_service(path)
    if enable:
        is_enabled = subprocess.run(['systemctl', '--user', 'is-enabled', SYSTEMD_SERVICE_NAME], stdout=subprocess.DEVNULL).returncode == 0
        is_active = subprocess.run(['systemctl', '--user', 'is-active', SYSTEMD_SERVICE_NAME], stdout=subprocess.DEVNULL).returncode == 0

        if not is_enabled:
            subprocess.run(['systemctl', '--user', 'enable', SYSTEMD_SERVICE_NAME], check=True)
        if is_active:
            if restart:
                subprocess.run(['systemctl', '--user', 'restart', '--now', SYSTEMD_SERVICE_NAME], check=True)
        else:
            subprocess.run(['systemctl', '--user', 'start', '--now', SYSTEMD_SERVICE_NAME], check=True)

def write_systemd_service(path: Path) -> None:
    daemon_path = shutil.which('lam-daemon') or Path(sys.argv[0]).resolve().parent / 'lam-daemon'

    template = f'''[Unit]
Description=Arctis Manager
StartLimitInterval=1min
StartLimitBurst=5

[Service]
Type=simple
ExecStart={daemon_path}
Restart=on-failure
RestartSec=5

[Install]
WantedBy=graphical-session.target'''
    
    if path.exists() and path.read_text() == f'{template}\n':
        return

    with open(path, 'w') as f:
        f.writelines([f'{line}\n' for line in template.split('\n')])
    
    subprocess.run(['systemctl', '--user', 'daemon-reload'], check=True)
