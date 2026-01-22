import inspect
import logging
from dataclasses import dataclass
from enum import Enum
from typing import Any, Callable, Literal

from ruamel.yaml import YAML

from linux_arctis_manager import status_parser_fn
from linux_arctis_manager.constants import DEVICES_CONFIG_FOLDER
from linux_arctis_manager.utils import JsonSerializable


class PaddingPosition(Enum):
    START = 'start'
    END = 'end'

class SettingType(Enum):
    SLIDER = 'slider'
    TOGGLE = 'toggle'
    SELECT = 'select'

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

    def __init__(self, starts_with: int, **kwargs: int):
        self.starts_with = starts_with
        for key, value in kwargs.items():
            setattr(self, key, value)

    def get_status_values(self, raw_response: list[int]) -> dict[str, int]:
        response = { k: raw_response[v] for k, v in self.__dict__.items() if k != 'starts_with' and v in range(len(raw_response)) }

        return response

class ConfigSetting(JsonSerializable):
    name: str
    type: SettingType
    default_value: int|str|None
    update_sequence: list[int|Literal['value']]

    _js_exclude_fields = ['name', 'update_sequence']

    def __init__(self, name: str, type: SettingType|str, default_value: int|str|None, update_sequence: list[int|Literal['value']] = [], **kwargs: Any):
        self.name = name
        self.type = type if isinstance(type, SettingType) else SettingType(type)
        self.default_value = default_value
        self.update_sequence = update_sequence

        for key, value in kwargs.items():
            setattr(self, key, value)
    
    def get_kwargs(self) -> dict[str, Any]:
        return { k: v for k, v in self.__dict__.items() if k not in ['name', 'type', 'default_value', 'update_sequence'] }

    def to_dict(self) -> dict[str, Any]:
        return { **super().to_dict(), **self.get_kwargs() }
    
    def get_update_sequence(self, value: int) -> list[int]:
        result = []
        for b in self.update_sequence:
            if isinstance(b, int):
                result.append(b)
            elif b == 'value':
                result.append(value)
            else:
                raise Exception(f"Invalid update sequence value: {b}")

        return result
    
    def __getattribute__(self, name: str) -> Any:
        return super().__getattribute__(name)

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
    representation: dict[str, list[str]]

    def __post_init__(self):
        raw_mappings: list[dict[str, int]] = self.response_mapping # pyright: ignore[reportAssignmentType]

        self.response_mapping = [ConfigStatusResponseMapping(
            starts_with=mapping.get('starts_with', 0),
            **{k: v for k, v in mapping.items() if k != 'starts_with'},
        ) for mapping in raw_mappings]

@dataclass
class OnlineStatusConfig:
    status_variable: str
    online_value: Any

class DeviceConfiguration:
    name: str
    vendor_id: int
    product_ids: list[int]
    command_interface_index: tuple[int, int]
    listen_interface_indexes: list[int]
    command_padding: ConfigPadding
    device_init: list[list[int|str]] | None
    status: ConfigStatus | None
    status_parse: dict[str, ConfigStatusParser]
    online_status: OnlineStatusConfig | None
    settings: dict[str, list[ConfigSetting]]

    def __init__(self, raw_configuration: dict[str, Any]):
        raw_config: dict[str, Any] | None = raw_configuration.get('device', None)
        if raw_config is None:
            raise ValueError("Invalid configuration: missing 'device' section")

        self.name = raw_config.get('name', '')
        self.vendor_id = raw_config.get('vendor_id', 0)
        self.product_ids = raw_config.get('product_ids', [])
        self.command_interface_index = raw_config.get('command_interface_index', (-1, -1))
        self.listen_interface_indexes = raw_config.get('listen_interface_indexes', [])
        
        online_status = raw_config.get('online_status', None)
        self.online_status = OnlineStatusConfig(**online_status) if online_status else None

        if not self.name:
            raise ValueError("Invalid configuration: 'device.name' must be specified and non-empty")
        if self.vendor_id == 0:
            raise ValueError("Invalid configuration: 'device.vendor_id' must be specified and non-zero")
        if not self.product_ids:
            raise ValueError("Invalid configuration: 'device.product_ids' must be a non-empty list")
        if not self.command_interface_index[0] >= 0 or not self.command_interface_index[1] >= 0:
            raise ValueError("Invalid configuration: 'device.command_interface_index' must represent [bInterfaceNumber and bAlternateSetting]")
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
            self.status = ConfigStatus(
                request=raw_status.get('request', 0),
                response_mapping=raw_status.get('response_mapping', []),
                representation=raw_status.get('representation', {}),
            )
        
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
                setting_default_value = setting_values.get('default', None)

                self.settings[setting_section].append(ConfigSetting(
                    name=setting_name,
                    type=setting_type,
                    default_value=setting_default_value,
                    **{k: v for k, v in setting_values.items() if k not in ['default', 'type']},
                ))

def load_device_configurations() -> list[DeviceConfiguration]:
    result = []
    yaml = YAML(typ='safe')

    logger = logging.getLogger('Configuration')
    logger.info('Loading device configurations...')
    logger.info('Searching configuration files in:')
    for config_path in DEVICES_CONFIG_FOLDER:
        logger.info(f'\t- {config_path}')

    for config_path in DEVICES_CONFIG_FOLDER:
        if not config_path.exists() or not config_path.is_dir():
            continue

        for file in config_path.glob('*.yaml'):
            config_yaml = yaml.load(file)
            config = DeviceConfiguration(config_yaml)

            logger.info(f'Found: {config.name} (0x{config.vendor_id:04x}, {[f"0x{pid:04x}" for pid in config.product_ids]}) from {file}')

            result.append(config)

    return result

status_parsers: list[Callable[..., Any]] = []
for name, obj in inspect.getmembers(status_parser_fn, inspect.isfunction):
    if hasattr(obj, '_status_type'):
        status_parsers.append(obj)

def parsed_status(raw_status: dict[str, int]|None, device_config: DeviceConfiguration|None) -> dict[str, Any]:
    if raw_status is None or device_config is None:
        return {}

    result = {}
    for key, raw_value in raw_status.items():
        status_parse_config = next((csp for sp, csp in device_config.status_parse.items() if sp == key), None)
        if status_parse_config is None:
            result[key] = raw_value
            continue
        parser = next((p for p in status_parsers if getattr(p, '_status_type') == status_parse_config.type.value), None)
        if parser is None:
            result[key] = raw_value
            continue
        result[key] = parser(value=raw_value, **status_parse_config.init_kwargs)
    
    return result
