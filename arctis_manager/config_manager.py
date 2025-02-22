from copy import deepcopy
import json
import os
from pathlib import Path
from typing import Optional


class ConfigManager:
    config_path: Path
    general_config_path: Path

    @staticmethod
    def get_instance():
        if not hasattr(ConfigManager, '_instance'):
            ConfigManager._instance = ConfigManager()

        return ConfigManager._instance

    def __init__(self):
        config_home = os.getenv('XDG_CONFIG_HOME', os.path.expanduser('~/.config'))

        self.config_path = Path(config_home).joinpath('arctis_manager')
        self.config_path.mkdir(parents=True, exist_ok=True)
        self.general_config_path = self.config_path.joinpath('general.json')

    def get_device_config(self, vendor_id: int, product_id: int) -> Optional[dict]:
        config_file_path = self.config_path.joinpath(f'device_{hex(vendor_id)[2:]}_{hex(product_id)[2:]}.json')
        if config_file_path.is_file():
            return json.load(config_file_path.open('r'))

        return None

    def save_device_config(self, vendor_id: int, product_id: int, config: dict):
        config_file_path = self.config_path.joinpath(f'device_{hex(vendor_id)[2:]}_{hex(product_id)[2:]}.json')
        with config_file_path.open('w') as f:
            json.dump(config, f)

    def get_general_config(self) -> dict:
        if self.general_config_path.is_file():
            return json.load(self.general_config_path.open('r'))

        return {}

    def save_general_config(self, config: dict) -> None:
        with self.general_config_path.open('w') as f:
            json.dump(config, f)
