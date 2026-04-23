import os
import signal
from pathlib import Path
from unittest.mock import call, patch

import pytest

from linux_arctis_manager.scripts.daemon import (
    _is_running,
    _pid_file,
    _read_existing_pid,
    _remove_pid,
    _write_pid,
    check_single_instance,
)


@pytest.fixture
def pid_dir(tmp_path, monkeypatch):
    monkeypatch.setenv('XDG_RUNTIME_DIR', str(tmp_path))
    return tmp_path


# --- _pid_file ---

def test_pid_file_uses_xdg_runtime_dir(pid_dir):
    assert _pid_file() == pid_dir / 'lam-daemon.pid'


def test_pid_file_falls_back_to_tmp(monkeypatch):
    monkeypatch.delenv('XDG_RUNTIME_DIR', raising=False)
    assert _pid_file() == Path('/tmp/lam-daemon.pid')


# --- _read_existing_pid ---

def test_read_existing_pid_returns_none_when_missing(pid_dir):
    assert _read_existing_pid() is None


def test_read_existing_pid_returns_int(pid_dir):
    (pid_dir / 'lam-daemon.pid').write_text('1234')
    assert _read_existing_pid() == 1234


def test_read_existing_pid_returns_none_on_non_integer(pid_dir):
    (pid_dir / 'lam-daemon.pid').write_text('not-a-pid')
    assert _read_existing_pid() is None


def test_read_existing_pid_returns_none_on_oserror(pid_dir):
    with patch('linux_arctis_manager.scripts.daemon._pid_file') as mock_pid_file:
        mock_path = mock_pid_file.return_value
        mock_path.exists.return_value = True
        mock_path.read_text.side_effect = OSError
        assert _read_existing_pid() is None


# --- _is_running ---

def test_is_running_true_when_kill_succeeds():
    with patch('os.kill') as mock_kill:
        assert _is_running(999) is True
        mock_kill.assert_called_once_with(999, 0)


def test_is_running_false_on_process_lookup_error():
    with patch('os.kill', side_effect=ProcessLookupError):
        assert _is_running(999) is False


def test_is_running_true_on_permission_error():
    with patch('os.kill', side_effect=PermissionError):
        assert _is_running(999) is True


# --- _write_pid / _remove_pid ---

def test_write_pid_writes_current_pid(pid_dir):
    with patch('os.getpid', return_value=5678):
        _write_pid()
    assert (pid_dir / 'lam-daemon.pid').read_text() == '5678'


def test_remove_pid_deletes_file(pid_dir):
    pid_file = pid_dir / 'lam-daemon.pid'
    pid_file.write_text('1234')
    _remove_pid()
    assert not pid_file.exists()


def test_remove_pid_no_error_when_missing(pid_dir):
    _remove_pid()  # must not raise


# --- check_single_instance ---

def test_check_single_instance_no_op_when_no_pid_file(pid_dir):
    check_single_instance(replace=False)  # must not raise or exit


def test_check_single_instance_no_op_when_stale_pid(pid_dir):
    (pid_dir / 'lam-daemon.pid').write_text('9999')
    with patch('os.kill', side_effect=ProcessLookupError):
        check_single_instance(replace=False)  # stale PID, process gone — no exit


def test_check_single_instance_exits_when_running_no_replace(pid_dir):
    (pid_dir / 'lam-daemon.pid').write_text('9999')
    with patch('os.kill'), pytest.raises(SystemExit) as exc_info:
        check_single_instance(replace=False)
    assert exc_info.value.code == 1


def test_check_single_instance_sends_sigterm_on_replace(pid_dir):
    (pid_dir / 'lam-daemon.pid').write_text('9999')
    kill_calls = []

    def fake_kill(pid, sig):
        kill_calls.append((pid, sig))
        if sig == 0 and len([c for c in kill_calls if c[1] == 0]) > 1:
            raise ProcessLookupError

    with patch('os.kill', side_effect=fake_kill), \
         patch('time.sleep'):
        check_single_instance(replace=True)

    assert (9999, signal.SIGTERM) in kill_calls


def test_check_single_instance_replace_exits_if_process_does_not_die(pid_dir):
    (pid_dir / 'lam-daemon.pid').write_text('9999')
    with patch('os.kill'), \
         patch('time.sleep'), \
         patch('sys.exit') as mock_exit:
        check_single_instance(replace=True)
        mock_exit.assert_called_once_with(1)


def test_check_single_instance_replace_returns_if_process_dies(pid_dir):
    (pid_dir / 'lam-daemon.pid').write_text('9999')
    kill_call_count = 0

    def fake_kill(pid, sig):
        nonlocal kill_call_count
        kill_call_count += 1
        # signal 0 checks after SIGTERM: fail on 2nd to simulate process exited
        if sig == 0 and kill_call_count > 2:
            raise ProcessLookupError

    with patch('os.kill', side_effect=fake_kill), \
         patch('time.sleep'), \
         patch('sys.exit') as mock_exit:
        check_single_instance(replace=True)
        mock_exit.assert_not_called()
