from pathlib import Path

from ruamel.yaml import YAML

from linux_arctis_manager.config import ConfigSetting, DeviceConfiguration, PaddingPosition, SettingType, StatusParseType


def test_config_parse():
    config_path = Path(__file__).parent.parent / 'src' / 'devices' / 'nova_pro_wireless.yaml'
    yaml = YAML(typ='safe')
    config_yaml = yaml.load(config_path)
    config = DeviceConfiguration(config_yaml)

    assert config.name == "SteelSeries Arctis Pro Wireless"
    assert config.vendor_id == 0x1038
    assert config.product_ids == [0x12e0, 0x12e5]

    assert config.command_interface_index == 7
    assert config.listen_interface_indexes == [7]
    
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

    assert len(config.status_parse) == 17
    assert config.status_parse.get('bluetooth_power_status') is not None
    assert config.status_parse['bluetooth_power_status'].type == StatusParseType.ON_OFF
    assert config.status_parse['bluetooth_power_status'].init_kwargs == {'off': 0x00, 'on': 0x01}
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
    assert config.status_parse['headset_power_status'].init_kwargs == {'values': {0x00: 'offline', 0x01: 'cable_charging', 0x02: 'online'}}

    assert config.settings is not None

    assert len(config.settings) == 3
    assert 'microphone' in config.settings
    assert 'power_management' in config.settings
    assert 'wireless' in config.settings

    assert len(config.settings['microphone']) == 4
    assert len(config.settings['power_management']) == 1
    assert len(config.settings['wireless']) == 1
    
    mic_settings: list[ConfigSetting] = config.settings['microphone']
    mic_gain = next((s for s in mic_settings if s.name == 'mic_gain'), None)
    assert mic_gain is not None
    assert mic_gain.name == 'mic_gain'
    assert mic_gain.type == SettingType.TOGGLE
    mic_gain_kwargs = mic_gain.get_kwargs()
    assert len(mic_gain_kwargs) == 1
    assert mic_gain_kwargs['values'] == {'off': 0x00, 'on': 0x01, 'off_label': 'high', 'on_label': 'low'}
