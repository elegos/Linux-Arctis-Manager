from abc import ABC
from pathlib import Path
from typing import Any

from ruamel.yaml import YAML

from linux_arctis_manager.constants import SETTINGS_FOLDER


class JsonSerializable(ABC):
    def to_dict(self) -> dict[str, Any]:
        def serialize(value: Any) -> Any:
            if isinstance(value, JsonSerializable):
                return value.to_dict()
            if isinstance(value, list):
                return [serialize(item) for item in value]
            return value
        
        cls = type(self)
        fields = getattr(cls, '__annotations__', {}).keys()

        return { field: serialize(getattr(self, field)) for field in fields }


class DeviceSettings(JsonSerializable):
    vendor_id: int
    product_id: int

    settings: dict[str, int]

    def __init__(self, vendor_id: int, product_id: int):
        self.vendor_id = vendor_id
        self.product_id = product_id
        self.settings = {}

    def _settings_file(self) -> Path:
        settings_file = SETTINGS_FOLDER / f'{self.vendor_id:04x}_{self.product_id:04x}.yaml'

        return settings_file

    def read_from_file(self):
        settings_file = self._settings_file()

        if not settings_file.exists():
            return

        yaml = YAML(typ='safe')
        self.settings = yaml.load(settings_file)

    def __setattr__(self, name: str, value: Any) -> None:
        if name in ('vendor_id', 'product_id', 'settings'):
            super().__setattr__(name, value)

            return

        self.settings[name] = int(value)
    
    def get(self, name: str, default: int = 0) -> int:
        return self.settings.get(name, default)

    def write_to_file(self):
        settings_file = self._settings_file()
        settings_file.parent.mkdir(parents=True, exist_ok=True)
        
        yaml = YAML(typ='safe')
        yaml.dump(self.settings, settings_file)

    def to_dict(self) -> dict:
        return self.__dict__


class GeneralSettings(JsonSerializable):
    # Automatically redirect on Media channel
    redirect_audio_on_connect: bool = False

    # When disconnecting, redirect to this device
    redirect_audio_on_disconnect: bool = False
    redirect_audio_on_disconnect_device: str|None = None

    def __init__(self, **kwargs):
        for key, value in kwargs.items():
            if key in self.__annotations__:
                setattr(self, key, value)

    @staticmethod
    def read_from_file() -> 'GeneralSettings':
        settings_file = SETTINGS_FOLDER / 'general_settings.yaml'

        if not settings_file.exists():
            return GeneralSettings()

        yaml = YAML(typ='safe')

        return GeneralSettings(**yaml.load(settings_file))
    
    def write_to_file(self):
        settings_file = SETTINGS_FOLDER / 'general_settings.yaml'
        settings_file.parent.mkdir(parents=True, exist_ok=True)
        
        yaml = YAML(typ='safe')
        yaml.dump(self.__dict__, settings_file)
