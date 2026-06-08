import json
from types import SimpleNamespace
from unittest.mock import MagicMock

import linux_arctis_manager.settings as settings_mod
from linux_arctis_manager.config import DeviceConfiguration
from linux_arctis_manager.dbus_service import ArctisManagerDbusSettingsService
from linux_arctis_manager.settings import DeviceSettings, GeneralSettings


def _device_config_with_toggle() -> DeviceConfiguration:
    raw = {
        'device': {
            'name': 'Test Device',
            'vendor_id': 0x1234,
            'product_ids': [0xabcd],
            'command_interface_index': [0, 0],
            'listen_interface_indexes': [0],
            'command_padding': {'length': 64, 'position': 'end', 'filler': 0x00},
            'settings': {
                'headset': {
                    'volume_limiter': {
                        'type': 'toggle',
                        'default': 0,
                        'values': {'on': 1, 'off': 0},
                    },
                    'sidetone': {
                        'type': 'slider',
                        'default': 0,
                        'min': 0,
                        'max': 3,
                        'step': 1,
                    },
                },
            },
        }
    }
    return DeviceConfiguration(raw)


def _make_service(tmp_path, monkeypatch, with_device=True):
    monkeypatch.setattr(settings_mod, 'SETTINGS_FOLDER', tmp_path)

    device_config = _device_config_with_toggle() if with_device else None
    device_settings = None
    if with_device:
        device_settings = DeviceSettings(0x1234, 0xabcd)
        device_settings.settings['volume_limiter'] = 0
        device_settings.settings['sidetone'] = 0

    core = SimpleNamespace(
        general_settings=GeneralSettings(),
        device_config=device_config,
        device_settings=device_settings,
        register_settings_observer=lambda observer: None,
    )
    service = ArctisManagerDbusSettingsService(core)
    service.signal_settings_changed = MagicMock()  # avoid emitting on a real bus
    return service, core


def test_settings_to_json_includes_systray_toggles(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)
    core.device_settings.systray_toggles = ['volume_limiter']

    payload = json.loads(service.settings_to_json(
        core.general_settings, core.device_config, core.device_settings))

    assert payload['systray_toggles'] == ['volume_limiter']


def test_settings_to_json_systray_toggles_empty_without_device(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch, with_device=False)

    payload = json.loads(service.settings_to_json(
        core.general_settings, None, None))

    assert payload['systray_toggles'] == []


def test_set_systray_toggle_adds_toggle(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)

    result = service.set_systray_toggle('volume_limiter', True)

    assert result is True
    assert core.device_settings.systray_toggles == ['volume_limiter']
    service.signal_settings_changed.assert_called_once()


def test_set_systray_toggle_removes_toggle(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)
    core.device_settings.systray_toggles = ['volume_limiter']

    result = service.set_systray_toggle('volume_limiter', False)

    assert result is True
    assert core.device_settings.systray_toggles == []


def test_set_systray_toggle_rejects_non_toggle_setting(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)

    result = service.set_systray_toggle('sidetone', True)

    assert result is False
    assert core.device_settings.systray_toggles == []


def test_set_systray_toggle_rejects_unknown_setting(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)

    result = service.set_systray_toggle('does_not_exist', True)

    assert result is False


def test_set_systray_toggle_rejects_when_no_device(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch, with_device=False)

    result = service.set_systray_toggle('volume_limiter', True)

    assert result is False


def test_set_systray_toggle_noop_when_already_pinned_does_not_emit(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)
    core.device_settings.systray_toggles = ['volume_limiter']

    result = service.set_systray_toggle('volume_limiter', True)

    assert result is True
    assert core.device_settings.systray_toggles == ['volume_limiter']
    service.signal_settings_changed.assert_not_called()


def test_set_systray_toggle_noop_when_already_absent_does_not_emit(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)

    result = service.set_systray_toggle('volume_limiter', False)

    assert result is True
    assert core.device_settings.systray_toggles == []
    service.signal_settings_changed.assert_not_called()


def test_on_settings_changed_emits_when_changed(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)

    service._on_settings_changed()

    service.signal_settings_changed.assert_called_once()


def test_on_settings_changed_dedups_identical_payload(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)

    service._on_settings_changed()
    service._on_settings_changed()  # identical payload -> no second emit

    service.signal_settings_changed.assert_called_once()


def test_on_settings_changed_emits_again_after_real_change(tmp_path, monkeypatch):
    service, core = _make_service(tmp_path, monkeypatch)

    service._on_settings_changed()
    core.device_settings.systray_toggles = ['volume_limiter']
    service._on_settings_changed()

    assert service.signal_settings_changed.call_count == 2
