import linux_arctis_manager.settings as settings_mod
from linux_arctis_manager.settings import DeviceSettings
from ruamel.yaml import YAML


def test_systray_toggles_defaults_to_empty_list():
    ds = DeviceSettings(0x1234, 0xabcd)
    assert ds.systray_toggles == []


def test_setattr_systray_toggles_does_not_leak_into_settings():
    ds = DeviceSettings(0x1234, 0xabcd)
    ds.systray_toggles = ['volume_limiter']
    assert ds.systray_toggles == ['volume_limiter']
    assert 'systray_toggles' not in ds.settings


def test_write_then_read_new_format_round_trip(tmp_path, monkeypatch):
    monkeypatch.setattr(settings_mod, 'SETTINGS_FOLDER', tmp_path)

    ds = DeviceSettings(0x1234, 0xabcd)
    ds.settings['volume_limiter'] = 1
    ds.systray_toggles = ['volume_limiter']
    ds.write_to_file()

    ds2 = DeviceSettings(0x1234, 0xabcd)
    ds2.settings['volume_limiter'] = 0  # default must be present for read to overlay
    ds2.read_from_file()

    assert ds2.settings['volume_limiter'] == 1
    assert ds2.systray_toggles == ['volume_limiter']


def test_read_old_flat_format_keeps_settings_and_empty_toggles(tmp_path, monkeypatch):
    monkeypatch.setattr(settings_mod, 'SETTINGS_FOLDER', tmp_path)

    yaml = YAML(typ='safe')
    yaml.dump({'volume_limiter': 1}, tmp_path / '1234_abcd.yaml')

    ds = DeviceSettings(0x1234, 0xabcd)
    ds.settings['volume_limiter'] = 0
    ds.read_from_file()

    assert ds.settings['volume_limiter'] == 1
    assert ds.systray_toggles == []
