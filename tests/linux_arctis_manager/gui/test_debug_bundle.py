import subprocess
import tarfile
from datetime import datetime
from pathlib import Path

from linux_arctis_manager.gui.debug_bundle import (
    build_command,
    default_file_name,
    script_path,
)


def test_script_ships_inside_the_package():
    path = script_path()
    assert path.is_file()
    assert path.name == 'collect_debug_info.sh'


def test_default_file_name_is_timestamped():
    assert default_file_name(datetime(2026, 11, 17, 9, 5, 3)) == 'lam-debug-info-20261117-090503.tar.gz'


def test_build_command_runs_the_script_with_bash_and_the_chosen_output(tmp_path: Path):
    program, args = build_command(Path('/x/collect_debug_info.sh'), tmp_path / 'out.tar.gz')
    assert program == 'bash'
    assert args == ['/x/collect_debug_info.sh', '--output', str(tmp_path / 'out.tar.gz')]


def test_script_writes_the_bundle_to_the_chosen_file_and_cleans_up(tmp_path: Path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    out = tmp_path / 'bundle.tar.gz'
    program, args = build_command(script_path(), out)
    result = subprocess.run(
        [program, *args], env={'PATH': '/usr/bin:/bin', 'TMPDIR': str(scratch), 'HOME': str(tmp_path)},
        capture_output=True, text=True, timeout=120, check=False,
    )
    assert result.returncode == 0, result.stderr
    with tarfile.open(out) as tar:
        assert 'bundle/report.txt' in tar.getnames()
    assert list(scratch.glob('lam-debug-info.*')) == [], 'scratch directory must be removed'


def test_script_fails_cleanly_when_the_output_cannot_be_written(tmp_path: Path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    program, args = build_command(script_path(), tmp_path / 'missing-dir' / 'x.tar.gz')
    result = subprocess.run(
        [program, *args], env={'PATH': '/usr/bin:/bin', 'TMPDIR': str(scratch), 'HOME': str(tmp_path)},
        capture_output=True, text=True, timeout=120, check=False,
    )
    assert result.returncode != 0
    assert list(scratch.glob('lam-debug-info.*')) == []
