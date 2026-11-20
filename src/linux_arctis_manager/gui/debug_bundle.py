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


DEFAULT_LISTEN_SECONDS = 60


def build_command(script: Path, output: Path, listen_seconds: int = 0) -> tuple[str, list[str]]:
    # Run through bash so it doesn't depend on the package keeping the exec bit.
    args = [str(script), '--output', str(output)]
    if listen_seconds > 0:
        args.append(f'--hidraw-capture={listen_seconds}')
    return 'bash', args


def estimated_seconds(listen_seconds: int) -> int:
    """Rough total run time, to drive the progress bar: the listen window plus
    the fixed cost of the other collectors."""
    return max(listen_seconds, 0) + 10
