from unittest.mock import patch

import pytest

from linux_arctis_manager.scripts.cli import main


def test_main_invokes_arctis_devices_tool_with_default_vendor_id(monkeypatch):
    monkeypatch.setattr('sys.argv', ['lam-cli', 'tools', 'arctis-devices'])
    with patch('linux_arctis_manager.scripts.cli.arctis_usb_info', return_value=0) as mock_info, \
         pytest.raises(SystemExit) as exc_info:
        main()
    mock_info.assert_called_once_with(0x1038)
    assert exc_info.value.code == 0


def test_main_passes_custom_vendor_id(monkeypatch):
    monkeypatch.setattr('sys.argv', ['lam-cli', 'tools', 'arctis-devices', '--vendor-id', '4660'])
    with patch('linux_arctis_manager.scripts.cli.arctis_usb_info', return_value=1) as mock_info, \
         pytest.raises(SystemExit) as exc_info:
        main()
    mock_info.assert_called_once_with(4660)
    assert exc_info.value.code == 1


def test_main_requires_a_command(monkeypatch):
    monkeypatch.setattr('sys.argv', ['lam-cli'])
    with pytest.raises(SystemExit):
        main()
