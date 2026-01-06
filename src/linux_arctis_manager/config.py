from dataclasses import dataclass
from enum import Enum
import inspect
from typing import Callable, Any

from linux_arctis_manager import status_parser_fn

# TODO move elsewhere?
status_parsers: list[Callable[..., Any]] = []
for name, obj in inspect.getmembers(status_parser_fn, inspect.isfunction):
    if hasattr(obj, '_status_type'):
        status_parsers.append(obj)
# TODO end TODO

class PaddingPosition(Enum):
    START = 'start'
    END = 'end'

class SettingType(Enum):
    SLIDER = 'slider'
    TOGGLE = 'toggle'

class StatusParseType(Enum):
    PERCENTAGE = 'percentage'
    ON_OFF = 'on_off'
    INT_STR_MAPPING = 'int_str_mapping'
    INT_INT_MAPPING = 'int_int_mapping'

@dataclass
class ConfigStatusParser:
    name: str
    type: StatusParseType
    init_kwargs: dict[str, Any]

class ConfigStatusResponseMapping:
    starts_with: int

    def __init__(self, starts_with: int, **kwargs: dict[str, int]):
        self.starts_with = starts_with
        for key, value in kwargs.items():
            setattr(self, key, value)

    def get_settings_values(self, raw_response: list[bytes]) -> dict[str, int]:
        response = { k: v for k, v in self.__dict__.items() if k != 'starts_with' and v in range(len(raw_response)) }

        return response

class ConfigSetting:
    name: str
    type: SettingType

    def __init__(self, name: str, type: SettingType, **kwargs: dict[str, Any]):
        self.name = name
        self.type = type
        for key, value in kwargs.items():
            setattr(self, key, value)
    
    def get_kwargs(self) -> dict[str, Any]:
        return { k: v for k, v in self.__dict__.items() if k not in ['name', 'type'] }

@dataclass
class ConfigPadding:
    length: int
    position: PaddingPosition
    filler: int

    def __post_init__(self):
        self.position = PaddingPosition(self.position)

@dataclass
class ConfigStatus:
    request: int
    response_mapping: list[ConfigStatusResponseMapping]

    def __post_init__(self):
        raw_mappings: list[dict[str, int]] = self.response_mapping # pyright: ignore[reportAssignmentType]

        self.response_mapping = [ConfigStatusResponseMapping(
            starts_with=mapping.get('starts_with', 0),
            **{k: v for k, v in mapping.items() if k != 'starts_with'},
        ) for mapping in raw_mappings]

class DeviceConfiguration:
    vendor_id: int
    product_ids: list[int]
    command_interface_index: int
    listen_interface_indexes: list[int]
    command_padding: ConfigPadding
    device_init: list[list[int|str]] | None
    status: ConfigStatus | None
    status_parse: dict[str, ConfigStatusParser]
    settings: dict[str, list[ConfigSetting]]

    def __init__(self, raw_configuration: dict[str, Any]):
        raw_config: dict[str, Any] | None = raw_configuration.get('device', None)
        if raw_config is None:
            raise ValueError("Invalid configuration: missing 'device' section")

        self.vendor_id = raw_config.get('vendor_id', 0)
        self.product_ids = raw_config.get('product_ids', [])
        self.command_interface_index = raw_config.get('command_interface_index', -1)
        self.listen_interface_indexes = raw_config.get('listen_interface_indexes', [])

        if self.vendor_id == 0:
            raise ValueError("Invalid configuration: 'device.vendor_id' must be specified and non-zero")
        if not self.product_ids:
            raise ValueError("Invalid configuration: 'device.product_ids' must be a non-empty list")
        if not self.command_interface_index >= 0:
            raise ValueError("Invalid configuration: 'device.command_interface_index' must be a non-negative integer")
        if not self.listen_interface_indexes:
            raise ValueError("Invalid configuration: 'device.listen_interface_indexes' must be a non-empty list")
        if any(i < 0 for i in self.listen_interface_indexes):
            raise ValueError("Invalid configuration: 'device.listen_interface_indexes' must contain only non-negative integers")

        raw_padding = raw_config.get('command_padding', {})
        if raw_padding:
            self.command_padding = ConfigPadding(**raw_padding)
        else:
            raise ValueError("Invalid configuration: 'device.command_padding' must be specified")

        raw_device_init = raw_config.get('device_init', None)
        if raw_device_init is not None:
            self.device_init = raw_device_init
        
        raw_status = raw_config.get('status', {})
        if raw_status:
            self.status = ConfigStatus(raw_status.get('request', 0), raw_status.get('response_mapping', []))
        
        raw_status_parse: dict[str, dict[str, Any]] = raw_config.get('status_parse', {})
        self.status_parse = {}
        for status_name, status_raw_values in raw_status_parse.items():
            parser_type = StatusParseType(status_raw_values.get('type', ''))
            init_kwargs = dict(status_raw_values.items())
            del init_kwargs['type']

            self.status_parse[status_name] = ConfigStatusParser(
                name=status_name,
                type=parser_type,
                init_kwargs=init_kwargs,
            )

        raw_settings: dict[str, dict[str, Any]] = raw_config.get('settings', {})
        self.settings = {}
        for setting_section, settings in raw_settings.items():
            self.settings[setting_section] = []
            for setting_name, setting_values in settings.items():
                setting_type = SettingType(setting_values.get('type', ''))
                self.settings[setting_section].append(ConfigSetting(
                    name=setting_name,
                    type=setting_type,
                    **{k: v for k, v in setting_values.items() if k != 'type'},
                ))

