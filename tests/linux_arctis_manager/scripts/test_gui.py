import subprocess
from unittest.mock import patch

from linux_arctis_manager.scripts.gui import (
    _is_dbus_service_available,
    _wait_for_dbus_service,
)

# main() itself creates a real QApplication/event loop and is exercised via
# manual smoke-testing (see the "run" workflow), not unit tests — only the
# pure helper functions below are practical to test headlessly.


def test_is_dbus_service_available_true_on_zero_return_code():
    with patch('subprocess.run') as mock_run:
        mock_run.return_value = type('R', (), {'returncode': 0})()
        assert _is_dbus_service_available() is True


def test_is_dbus_service_available_false_on_nonzero_return_code():
    with patch('subprocess.run') as mock_run:
        mock_run.return_value = type('R', (), {'returncode': 1})()
        assert _is_dbus_service_available() is False


def test_is_dbus_service_available_false_on_timeout():
    with patch('subprocess.run', side_effect=subprocess.TimeoutExpired(cmd='dbus-send', timeout=3)):
        assert _is_dbus_service_available() is False


def test_is_dbus_service_available_false_when_dbus_send_missing():
    with patch('subprocess.run', side_effect=FileNotFoundError):
        assert _is_dbus_service_available() is False


def test_wait_for_dbus_service_returns_true_immediately_when_available():
    with patch('linux_arctis_manager.scripts.gui._is_dbus_service_available', return_value=True):
        assert _wait_for_dbus_service(timeout=1.0) is True


def test_wait_for_dbus_service_times_out_when_never_available():
    with patch('linux_arctis_manager.scripts.gui._is_dbus_service_available', return_value=False), \
         patch('time.sleep'):
        assert _wait_for_dbus_service(timeout=0.01) is False
