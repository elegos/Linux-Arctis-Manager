"""Running the support-bundle script (``collect_debug_info.sh``) from the GUI.

The script ships inside the Python package, so every install method (distro
packages, the user-level installer, ``make install``) has it next to the code.
It runs as the GUI's own user, in the user's session, and writes the archive
straight to the file the user picked: nothing is left in a temporary place."""

from datetime import datetime
from pathlib import Path

_SCRIPT_NAME = 'collect_debug_info.sh'


def script_path() -> Path:
    return Path(__file__).resolve().parent.parent / 'scripts' / _SCRIPT_NAME


def default_file_name(now: datetime) -> str:
    return f'lam-debug-info-{now:%Y%m%d-%H%M%S}.tar.gz'


def build_command(script: Path, output: Path) -> tuple[str, list[str]]:
    # Run through bash so it doesn't depend on the package keeping the exec bit.
    return 'bash', [str(script), '--output', str(output)]
