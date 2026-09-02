import subprocess
from unittest.mock import call, patch

from linux_arctis_manager.constants import (
    SYSTEMD_HELPER_SERVICE_NAME,
    SYSTEMD_SERVICE_NAME,
)
from linux_arctis_manager.systemd import ensure_systemd_unit, is_systemd_unit_enabled


def test_is_systemd_unit_enabled_true_when_check_call_succeeds():
    with patch('subprocess.check_call') as mock_check_call:
        mock_check_call.return_value = 0
        assert is_systemd_unit_enabled() is True


def test_is_systemd_unit_enabled_false_on_called_process_error():
    with patch('subprocess.check_call', side_effect=subprocess.CalledProcessError(1, 'systemctl')):
        assert is_systemd_unit_enabled() is False


def test_ensure_systemd_unit_noop_when_not_enabling():
    with patch('subprocess.run') as mock_run:
        ensure_systemd_unit(enable=False)
        mock_run.assert_not_called()


def test_ensure_systemd_unit_starts_when_inactive():
    with patch('subprocess.run') as mock_run:
        mock_run.side_effect = [
            None,  # daemon-reload
            type('R', (), {'returncode': 1})(),  # is-active -> inactive
            None,  # enable
            None,  # start
        ]
        ensure_systemd_unit(enable=True)

    calls = mock_run.call_args_list
    assert calls[0] == call(['systemctl', '--user', 'daemon-reload'], check=True)
    assert calls[-1] == call(
        ['systemctl', '--user', 'start', SYSTEMD_HELPER_SERVICE_NAME, SYSTEMD_SERVICE_NAME], check=True
    )


def test_ensure_systemd_unit_restarts_when_active_and_restart_requested():
    with patch('subprocess.run') as mock_run:
        mock_run.side_effect = [
            None,  # daemon-reload
            type('R', (), {'returncode': 0})(),  # is-active -> active
            None,  # enable
            None,  # restart
        ]
        ensure_systemd_unit(enable=True, restart=True)

    calls = mock_run.call_args_list
    assert calls[-1] == call(
        ['systemctl', '--user', 'restart', SYSTEMD_HELPER_SERVICE_NAME, SYSTEMD_SERVICE_NAME], check=True
    )


def test_ensure_systemd_unit_leaves_active_service_alone_without_restart():
    with patch('subprocess.run') as mock_run:
        mock_run.side_effect = [
            None,  # daemon-reload
            type('R', (), {'returncode': 0})(),  # is-active -> active
            None,  # enable
        ]
        ensure_systemd_unit(enable=True, restart=False)

    calls = mock_run.call_args_list
    assert len(calls) == 3  # no start/restart call issued
