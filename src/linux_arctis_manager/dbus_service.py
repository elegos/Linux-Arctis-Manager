import asyncio
import itertools
import json
import logging

from dbus_next.aio.message_bus import MessageBus
from dbus_next.service import ServiceInterface, method

from linux_arctis_manager.config import parsed_status
from linux_arctis_manager.constants import \
    DBUS_BUS_NAME, DBUS_CONFIG_INTERFACE_NAME, DBUS_CONFIG_OBJECT_PATH, DBUS_SETTINGS_OBJECT_PATH, \
    DBUS_SETTINGS_INTERFACE_NAME, DBUS_STATUS_INTERFACE_NAME, DBUS_STATUS_OBJECT_PATH
from linux_arctis_manager.core import CoreEngine

class ArctisManagerDbusConfigService(ServiceInterface):
    def __init__(self, core: CoreEngine):
        super().__init__(DBUS_CONFIG_INTERFACE_NAME)
        self.core_engine = core

    @method('ReloadConfigs')
    def reload_configs(self) -> 'b': # type: ignore
        self.core_engine.reload_device_configurations()

        return True

class ArctisManagerDbusStatusService(ServiceInterface):
    def __init__(self, core: CoreEngine):
        super().__init__(DBUS_STATUS_INTERFACE_NAME)
        self.core_engine = core

    @method('GetStatus')
    def get_status(self) -> 's': # type: ignore
        return json.dumps(parsed_status(self.core_engine.device_status, self.core_engine.device_config)) if self.core_engine.device_status else ''

class ArctisManagerDbusSettingsService(ServiceInterface):
    def __init__(self, core: CoreEngine):
        super().__init__(DBUS_SETTINGS_INTERFACE_NAME)
        self.core_engine = core
        self.logger = logging.getLogger('ArctisManagerDbusSettingsService')

    @method('GetSettings')
    def get_settings(self) -> 's': # type: ignore
        settings = {
            'general': self.core_engine.general_settings.to_dict(),
            'device': {},
            'settings_config': {
                config.name: config.to_dict()
                for config in self.core_engine.general_settings.settings_config
            },
        }

        if self.core_engine.device_config and self.core_engine.device_settings:
            settings.update({'device': self.core_engine.device_settings.settings})
            settings['settings_config'].update({
                config.name: config.to_dict()
                for config in list(itertools.chain.from_iterable(
                    self.core_engine.device_config.settings.values()
                ))
            })

        return json.dumps(settings)
    
    @method('SetSetting')
    def set_setting(self, setting: 's', value: 's') -> 'b': # type: ignore
        try:
            value = json.loads(value)
        except json.JSONDecodeError as e:
            self.logger.error(f'SetSetting: error while parsing JSON value ({value}): {e}')

            return False

        general_settings_keys = self.core_engine.general_settings.to_dict().keys()
        if setting in general_settings_keys:
            setting_config = next((config for config in self.core_engine.general_settings.settings_config if config.name == setting), None)
            if not setting_config:
                self.logger.error(f'Unknown general setting configuration: {setting}')
                return False
            
            if type(setting_config.default_value) != type(value):
                self.logger.error(f'Value type mismatch: {type(setting_config.default_value)} != {type(value)}')
                return False

            setattr(self.core_engine.general_settings, setting, value)
            self.core_engine.general_settings.write_to_file()

            return True
        
        if self.core_engine.device_config and self.core_engine.device_settings:
            device_settings_keys = self.core_engine.device_settings.settings.keys()
            if setting in device_settings_keys:
                setting_config = next((config for config in itertools.chain.from_iterable(self.core_engine.device_config.settings.values()) if config.name == setting), None)
                if not setting_config:
                    self.logger.error(f'Unknown device setting configuration: {setting}')
                    return False
                
                if type(setting_config.default_value) != type(value):
                    self.logger.error(f'Value type mismatch: {type(setting_config.default_value)} != {type(value)}')
                    return False

                setattr(self.core_engine.device_settings, setting, value)
                self.core_engine.device_settings.write_to_file()

                return True

        return False


class DbusManager:
    _instance: 'DbusManager|None' = None

    @staticmethod
    def getInstance() -> 'DbusManager':
        if DbusManager._instance is None:
            DbusManager._instance = DbusManager()

        return DbusManager._instance

    def __init__(self):
        self.log = logging.getLogger('DbusManager')
    
    def setup_sinks(self):
        pass
    
    async def start(self, core_engine: CoreEngine):
        self.log.info("Initializing service...")

        self.core_engine = core_engine

        bus = await MessageBus().connect()
        for tpl in [
            (ArctisManagerDbusConfigService, DBUS_CONFIG_OBJECT_PATH),
            (ArctisManagerDbusSettingsService, DBUS_SETTINGS_OBJECT_PATH),
            (ArctisManagerDbusStatusService, DBUS_STATUS_OBJECT_PATH)
        ]:
            interface = tpl[0](self.core_engine)
            bus.export(tpl[1], interface)

        await bus.request_name(DBUS_BUS_NAME)

    async def wait_for_stop(self) -> None:
        while not getattr(self, '_stopping', False):
            await asyncio.sleep(1)
        
        self.core_engine.stop()
        self.core_engine.teardown()

    def stop(self):
        self.log.info("Stopping D-Bus service...")
        self._stopping = True
