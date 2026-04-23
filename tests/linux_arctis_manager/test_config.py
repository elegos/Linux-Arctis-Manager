from pathlib import Path
from typing import Any
from unittest.mock import patch

import pytest
from ruamel.yaml import YAML

from linux_arctis_manager.config import (ConfigPadding, ConfigSetting,
                                         ConfigStatus, ConfigStatusParser,
                                         ConfigStatusResponseMapping,
                                         DeviceConfiguration,
                                         OnlineStatusConfig, PaddingPosition,
                                         SettingType, StatusParseType,
                                         parsed_status)


def test_config_parse():
    config_path = Path(__file__).parent.parent.parent / 'src' / 'linux_arctis_manager' / 'devices' / 'nova_pro_wireless.yaml'
    yaml = YAML(typ='safe')
    config_yaml = yaml.load(config_path)
    config = DeviceConfiguration(config_yaml)

    assert config.name == "SteelSeries Arctis Nova Pro Wireless"
    assert config.vendor_id == 0x1038
    assert config.product_ids == [0x12e0, 0x12e5]

    assert config.command_interface_index == [4, 0]
    assert config.listen_interface_indexes == [4]
    
    assert config.command_padding.length == 64
    assert config.command_padding.position == PaddingPosition.END
    assert config.command_padding.filler == 0x00

    assert config.device_init is not None
    assert len(config.device_init) == 38

    assert config.status is not None
    assert config.status.request == 0x06b0
    assert len(config.status.response_mapping) == 3
    assert config.status.response_mapping[0].starts_with == 0x0725
    assert config.status.response_mapping[1].starts_with == 0x0745
    assert config.status.response_mapping[2].starts_with == 0x06b0
    assert len(config.status.response_mapping[0].__dict__.keys()) == 2
    assert len(config.status.response_mapping[1].__dict__.keys()) == 3
    assert len(config.status.response_mapping[2].__dict__.keys()) == 15
    assert hasattr(config.status.response_mapping[2], 'headset_power_status')
    assert getattr(config.status.response_mapping[2], 'headset_power_status') == 0x0f
    assert len(config.status.representation.keys()) == 5
    assert list(config.status.representation.keys()) == ['headset', 'mic', 'gamedac', 'bluetooth', 'wireless']
    assert config.status.representation['gamedac'] == ['station_volume', 'charge_slot_battery_charge']

    assert len(config.status_parse) == 17
    assert config.status_parse.get('bluetooth_power_status') is not None
    assert config.status_parse['bluetooth_power_status'].type == StatusParseType.ON_OFF
    assert config.status_parse['bluetooth_power_status'].init_kwargs == {'off': 0x01, 'on': 0x00}
    assert config.status_parse.get('mic_led_brightness') is not None
    assert config.status_parse['mic_led_brightness'].type == StatusParseType.PERCENTAGE
    assert config.status_parse['mic_led_brightness'].init_kwargs == {'perc_min': 0, 'perc_max': 10}
    assert config.status_parse.get('auto_off_time_minutes') is not None
    assert config.status_parse['auto_off_time_minutes'].type == StatusParseType.INT_INT_MAPPING
    assert config.status_parse['auto_off_time_minutes'].init_kwargs == {'values': {
        0x00: 0, 0x01: 1, 0x02: 5, 0x03: 10, 0x04: 15, 0x05: 30, 0x06: 60
    }}
    assert config.status_parse.get('headset_power_status') is not None
    assert config.status_parse['headset_power_status'].type == StatusParseType.INT_STR_MAPPING
    assert config.status_parse['headset_power_status'].init_kwargs == {'values': {0x01: 'offline', 0x02: 'cable_charging', 0x08: 'online'}}

    assert config.settings is not None

    assert len(config.settings) == 4
    assert 'headset' in config.settings
    assert 'microphone' in config.settings
    assert 'power_management' in config.settings
    assert 'wireless' in config.settings

    assert len(config.settings['headset']) == 1
    assert len(config.settings['microphone']) == 3
    assert len(config.settings['power_management']) == 1
    assert len(config.settings['wireless']) == 1
    
    headset_settings: list[ConfigSetting] = config.settings['headset']
    gain = next((s for s in headset_settings if s.name == 'gain'), None)
    assert gain is not None
    assert gain.name == 'gain'
    assert gain.type == SettingType.DISCRETE_MAP
    assert gain.default_value == 0x02
    gain_kwargs = gain.get_kwargs()
    assert len(gain_kwargs) == 1
    assert gain_kwargs['values_mapping'] == {0x01: 'low', 0x02: 'high'}

def _minimal_raw(overrides: dict[str, Any] | None = None) -> dict[str, Any]:
    base: dict[str, Any] = {
        'device': {
            'name': 'Test Device',
            'vendor_id': 0x1234,
            'product_ids': [0xABCD],
            'command_interface_index': [0, 0],
            'listen_interface_indexes': [0],
            'command_padding': {'length': 64, 'position': 'end', 'filler': 0x00},
        }
    }
    if overrides:
        base['device'].update(overrides)
    return base


# --- ConfigStatusResponseMapping ---

def test_get_status_values_empty_when_no_extra_fields():
    mapping = ConfigStatusResponseMapping(starts_with=0x01)
    assert mapping.get_status_values([0x01, 0x02, 0x03]) == {}


def test_get_status_values_skips_out_of_bounds_index():
    mapping = ConfigStatusResponseMapping(starts_with=0x01, field_a=5)
    assert mapping.get_status_values([0x01, 0x02]) == {}


def test_get_status_values_includes_in_bounds_indexes():
    mapping = ConfigStatusResponseMapping(starts_with=0x01, field_a=1, field_b=2)
    assert mapping.get_status_values([0x00, 0xAA, 0xBB]) == {'field_a': 0xAA, 'field_b': 0xBB}


def test_ConfigStatusResponseMapping_get_status_values():
    mapping = ConfigStatusResponseMapping(starts_with=0x123b, status1=0x02, status2=0x03)
    message = [0x12, 0x3b, 0x10, 0x11]

    status = mapping.get_status_values(message)

    assert status.get('status1', None) == 0x10
    assert status.get('status2', None) == 0x11


# --- ConfigSetting ---

def test_config_setting_accepts_string_type():
    s = ConfigSetting(name='vol', type='slider', default_value=5)
    assert s.type == SettingType.SLIDER


def test_config_setting_accepts_enum_type():
    s = ConfigSetting(name='vol', type=SettingType.TOGGLE, default_value=1)
    assert s.type == SettingType.TOGGLE


def test_config_setting_raises_on_unknown_type():
    with pytest.raises(ValueError):
        ConfigSetting(name='vol', type='invalid_type', default_value=0)


def test_config_setting_get_kwargs_excludes_reserved_fields():
    s = ConfigSetting(name='vol', type='slider', default_value=5, min=0, max=10)
    kwargs = s.get_kwargs()
    assert 'name' not in kwargs
    assert 'type' not in kwargs
    assert 'default_value' not in kwargs
    assert 'update_sequence' not in kwargs
    assert kwargs == {'min': 0, 'max': 10}


def test_config_setting_get_update_sequence_replaces_value_token():
    s = ConfigSetting(name='vol', type='slider', default_value=0,
                      update_sequence=[0x06, 0x37, 'value'])
    assert s.get_update_sequence(0xAB) == [0x06, 0x37, 0xAB]


def test_config_setting_get_update_sequence_passes_ints_through():
    s = ConfigSetting(name='vol', type='slider', default_value=0,
                      update_sequence=[0x01, 0x02])
    assert s.get_update_sequence(0xFF) == [0x01, 0x02]


def test_config_setting_get_update_sequence_raises_on_invalid_token():
    s = ConfigSetting(name='vol', type='slider', default_value=0,
                      update_sequence=[0x01, 'bad_token'])  # type: ignore[list-item]
    with pytest.raises(Exception, match="Invalid update sequence value"):
        s.get_update_sequence(1)


# --- ConfigPadding ---

def test_config_padding_converts_string_to_enum():
    p = ConfigPadding(length=64, position='end', filler=0x00)  # type: ignore[arg-type]
    assert p.position == PaddingPosition.END


def test_config_padding_accepts_enum_directly():
    p = ConfigPadding(length=64, position=PaddingPosition.START, filler=0xFF)
    assert p.position == PaddingPosition.START


# --- ConfigStatus ---

def test_config_status_converts_raw_dicts_to_mappings():
    raw = [{'starts_with': 0x01, 'battery': 2}]
    status = ConfigStatus(request=0x10, response_mapping=raw, representation={})  # type: ignore[arg-type]
    assert len(status.response_mapping) == 1
    assert isinstance(status.response_mapping[0], ConfigStatusResponseMapping)
    assert status.response_mapping[0].starts_with == 0x01


def test_config_status_empty_response_mapping():
    status = ConfigStatus(request=0x10, response_mapping=[], representation={})
    assert status.response_mapping == []


# --- DeviceConfiguration ---

def test_device_configuration_raises_when_device_section_missing():
    with pytest.raises(ValueError, match="missing 'device' section"):
        DeviceConfiguration({})


def test_device_configuration_raises_when_name_empty():
    with pytest.raises(ValueError, match="'device.name'"):
        DeviceConfiguration(_minimal_raw({'name': ''}))


def test_device_configuration_raises_when_vendor_id_zero():
    with pytest.raises(ValueError, match="'device.vendor_id'"):
        DeviceConfiguration(_minimal_raw({'vendor_id': 0}))


def test_device_configuration_raises_when_product_ids_empty():
    with pytest.raises(ValueError, match="'device.product_ids'"):
        DeviceConfiguration(_minimal_raw({'product_ids': []}))


def test_device_configuration_raises_when_listen_interface_indexes_empty():
    with pytest.raises(ValueError, match="'device.listen_interface_indexes'"):
        DeviceConfiguration(_minimal_raw({'listen_interface_indexes': []}))


def test_device_configuration_raises_when_listen_interface_indexes_negative():
    with pytest.raises(ValueError, match="'device.listen_interface_indexes'"):
        DeviceConfiguration(_minimal_raw({'listen_interface_indexes': [-1]}))


def test_device_configuration_raises_when_command_padding_missing():
    with pytest.raises(ValueError, match="'device.command_padding'"):
        DeviceConfiguration(_minimal_raw({'command_padding': {}}))


def test_device_configuration_parses_online_status_when_present():
    raw = _minimal_raw({'online_status': {'status_variable': 'power', 'online_value': 1}})
    config = DeviceConfiguration(raw)
    assert isinstance(config.online_status, OnlineStatusConfig)
    assert config.online_status.status_variable == 'power'
    assert config.online_status.online_value == 1


def test_device_configuration_online_status_none_when_absent():
    config = DeviceConfiguration(_minimal_raw())
    assert config.online_status is None


def test_device_configuration_parses_status_parse():
    raw = _minimal_raw({
        'status_parse': {
            'battery': {'type': 'percentage', 'perc_min': 0, 'perc_max': 100}
        }
    })
    config = DeviceConfiguration(raw)
    assert 'battery' in config.status_parse
    entry = config.status_parse['battery']
    assert isinstance(entry, ConfigStatusParser)
    assert entry.type == StatusParseType.PERCENTAGE
    assert entry.init_kwargs == {'perc_min': 0, 'perc_max': 100}


def test_device_configuration_empty_status_parse_by_default():
    config = DeviceConfiguration(_minimal_raw())
    assert config.status_parse == {}


# --- parsed_status ---

def test_parsed_status_returns_empty_when_raw_status_none():
    assert parsed_status(None, None) == {}


def test_parsed_status_returns_empty_when_device_config_none():
    assert parsed_status({'battery': 80}, None) == {}


def test_parsed_status_returns_raw_value_when_key_not_in_status_parse():
    config = DeviceConfiguration(_minimal_raw())
    assert parsed_status({'unknown_key': 42}, config) == {'unknown_key': 42}


def test_parsed_status_returns_raw_value_when_no_matching_parser():
    raw = _minimal_raw({
        'status_parse': {'battery': {'type': 'percentage', 'perc_min': 0, 'perc_max': 100}}
    })
    config = DeviceConfiguration(raw)
    with patch('linux_arctis_manager.config.status_parsers', []):
        result = parsed_status({'battery': 80}, config)
    assert result == {'battery': 80}


def test_parsed_status_calls_correct_parser():
    raw = _minimal_raw({
        'status_parse': {'battery': {'type': 'percentage', 'perc_min': 0, 'perc_max': 10}}
    })
    config = DeviceConfiguration(raw)
    assert parsed_status({'battery': 5}, config) == {'battery': 50}
