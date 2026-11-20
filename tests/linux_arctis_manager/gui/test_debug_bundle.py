import subprocess
import tarfile
from datetime import datetime
from pathlib import Path

from linux_arctis_manager.gui.debug_bundle import (
    build_command,
    default_file_name,
    estimated_seconds,
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


def test_build_command_adds_the_listen_time_only_when_positive(tmp_path: Path):
    _, none = build_command(Path('/x/s.sh'), tmp_path / 'o', 0)
    _, sixty = build_command(Path('/x/s.sh'), tmp_path / 'o', 60)
    assert not any(a.startswith('--hidraw-capture') for a in none)
    assert '--hidraw-capture=60' in sixty


def test_estimated_seconds_covers_the_listen_window():
    assert estimated_seconds(60) > 60
    assert estimated_seconds(0) > 0


def _sandbox(tmp_path: Path) -> tuple[dict[str, str], Path, Path]:
    """Environment for running the real script without ever escalating privileges.

    `LAM_NO_PRIVILEGE` turns escalation off in the script, and stub
    `pkexec`/`sudo`/`hid-recorder` come first on PATH as a second line of defence, so a machine with a
    real SteelSeries dongle never gets a password prompt (nor does CI). Every
    call to a stub is logged, so tests can assert nothing privileged ran."""
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    fakebin = tmp_path / 'fakebin'
    fakebin.mkdir()
    calls = tmp_path / 'privileged-calls.log'
    for name in ('pkexec', 'sudo', 'hid-recorder'):
        stub = fakebin / name
        stub.write_text(f'#!/bin/sh\necho "{name} $*" >> "{calls}"\nexit 1\n')
        stub.chmod(0o755)
    env = {
        'PATH': f'{fakebin}:/usr/bin:/bin', 'LAM_NO_PRIVILEGE': '1', 'TMPDIR': str(scratch), 'HOME': str(tmp_path),
    }
    return env, scratch, calls


def _run_script(args: list[str], env: dict[str, str]) -> subprocess.CompletedProcess:
    # stdin=DEVNULL: the script picks sudo over pkexec when stdin is a terminal.
    return subprocess.run(
        ['bash', *args], env=env, stdin=subprocess.DEVNULL,
        capture_output=True, text=True, timeout=120, check=False,
    )


def test_script_writes_the_bundle_to_the_chosen_file_and_cleans_up(tmp_path: Path):
    env, scratch, calls = _sandbox(tmp_path)
    out = tmp_path / 'bundle.tar.gz'
    _program, args = build_command(script_path(), out)
    result = _run_script(args, env)
    assert result.returncode == 0, result.stderr
    with tarfile.open(out) as tar:
        assert 'bundle/report.txt' in tar.getnames()
    assert list(scratch.glob('lam-debug-info.*')) == [], 'scratch directory must be removed'
    assert not calls.exists(), f'privileged tool was invoked: {calls.read_text()}'


def test_script_fails_cleanly_when_the_output_cannot_be_written(tmp_path: Path):
    env, scratch, calls = _sandbox(tmp_path)
    _program, args = build_command(script_path(), tmp_path / 'missing-dir' / 'x.tar.gz')
    result = _run_script(args, env)
    assert result.returncode != 0
    assert list(scratch.glob('lam-debug-info.*')) == []
    assert not calls.exists()
