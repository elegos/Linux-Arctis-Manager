import asyncio
import itertools
import json
import logging

from dbus_next.aio.message_bus import MessageBus
from dbus_next.service import ServiceInterface, method, signal

from linux_arctis_manager.config import DeviceConfiguration, parsed_status
from linux_arctis_manager.constants import (DBUS_BUS_NAME,
                                            DBUS_CONFIG_INTERFACE_NAME,
                                            DBUS_CONFIG_OBJECT_PATH,
                                            DBUS_EQ_INTERFACE_NAME,
                                            DBUS_EQ_OBJECT_PATH,
                                            DBUS_SETTINGS_INTERFACE_NAME,
                                            DBUS_SETTINGS_OBJECT_PATH,
                                            DBUS_STATUS_INTERFACE_NAME,
                                            DBUS_STATUS_OBJECT_PATH)
from linux_arctis_manager.core import CoreEngine
from linux_arctis_manager.pactl import TypedPulseSinkInfo
from linux_arctis_manager.settings import DeviceSettings, GeneralSettings


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
        self.last_device_status = ''
        self.core_engine.register_status_observer(self._on_status_changed)
    
    @staticmethod
    def _device_status_to_dbus_status(device_status: dict[str, int]|None, device_config: DeviceConfiguration|None) -> str:
        if not device_status or not device_config or not device_config.status:
            return json.dumps({})
        
        result = {}
        raw_status = parsed_status(device_status, device_config)
        for category, status_list in device_config.status.representation.items():
            result[category] = {}
            for status in status_list:
                if status in raw_status:
                    result[category][status] = {
                        'value': raw_status[status],
                        'type': 'label' if type(raw_status[status]) == str else device_config.status_parse[status].type.value
                    }
            if not result[category]:
                del result[category]

        return json.dumps(result)

    @signal('StatusChanged')
    def signal_status_changed(self, status_json_str: 's') -> 's': # type: ignore
        return status_json_str

    def _on_status_changed(self, new_status: dict[str, int]) -> None:
        dumped = self._device_status_to_dbus_status(new_status, self.core_engine.device_config)

        if dumped == self.last_device_status:
            return

        self.last_device_status = dumped

        self.signal_status_changed(dumped)

    @method('GetStatus')
    def method_get_status(self) -> 's': # type: ignore
        return self._device_status_to_dbus_status(self.core_engine.device_status, self.core_engine.device_config)


class ArctisManagerDbusSettingsService(ServiceInterface):
    def __init__(self, core: CoreEngine):
        super().__init__(DBUS_SETTINGS_INTERFACE_NAME)
        self.core_engine = core
        self.logger = logging.getLogger('ArctisManagerDbusSettingsService')
    
    def settings_to_json(self, general_settings: GeneralSettings, device_config: DeviceConfiguration|None, device_settings: DeviceSettings|None) -> str:
        settings = {
            'general': general_settings.to_dict(),
            'device': {},
            'settings_config': {
                config.name: config.to_dict()
                for config in self.core_engine.general_settings.settings_config
            },
        }

        if device_config and device_settings:
            settings.update({'device': device_settings.settings})
        if device_config and device_settings:
            settings.update({'device': device_settings.settings})
            settings['settings_config'].update({
                config.name: config.to_dict()
                for config in list(itertools.chain.from_iterable(
                    device_config.settings.values()
                ))
            })

        return json.dumps(settings)

    @signal('SettingsChanged')
    def signal_settings_changed(self, settings_json_str: 's') -> 's': # type: ignore
        return settings_json_str

    @method('GetSettings')
    def get_settings(self) -> 's': # type: ignore
        return self.settings_to_json(self.core_engine.general_settings, self.core_engine.device_config, self.core_engine.device_settings)
    
    @method('SetSetting')
    def set_setting(self, setting: 's', value: 's') -> 'b': # type: ignore
        try:
            value = json.loads(value)
        except json.JSONDecodeError as e:
            self.logger.error(f'SetSetting: error while parsing JSON value ({value}): {e}')

            return False

        general_settings_keys = self.core_engine.general_settings.to_dict().keys()
        if setting in general_settings_keys:
            config = next((config for config in self.core_engine.general_settings.settings_config if config.name == setting), None)
            if not config:
                self.logger.error(f'Unknown general setting configuration: {setting}')
                return False
            
            # TODO add type checking in case of default_value is None
            if type(config.default_value) != type(value) and config.default_value is not None:
                self.logger.error(f'Value type mismatch: {type(config.default_value)} != {type(value)}')
                return False

            setattr(self.core_engine.general_settings, setting, value)
            self.core_engine.general_settings.write_to_file()

            self.signal_settings_changed(self.settings_to_json(self.core_engine.general_settings, self.core_engine.device_config, self.core_engine.device_settings))

            return True
        
        if self.core_engine.device_config and self.core_engine.device_settings:
            device_settings_keys = self.core_engine.device_settings.settings.keys()
            if setting in device_settings_keys:
                config = next((config for section in self.core_engine.device_config.settings.keys() for config in self.core_engine.device_config.settings[section]), None)
                if not config:
                    self.logger.error(f'Unknown device setting configuration: {setting}')
                    return False
                
                if type(config.default_value) != type(value):
                    self.logger.error(f'Value type mismatch: {type(config.default_value)} != {type(value)}')
                    return False

                self.core_engine.device_settings.settings[setting] = value
                self.core_engine.device_settings.write_to_file()

                self.signal_settings_changed(self.settings_to_json(self.core_engine.general_settings, self.core_engine.device_config, self.core_engine.device_settings))

                return True

        return False
    
    @method('GetListOptions')
    def get_list_options(self, list_name: 's') -> 's': # type: ignore
        result = []
        if list_name == 'pulse_audio_devices':
            sinks: list[TypedPulseSinkInfo] = self.core_engine.pa_audio_manager.pulse.sink_list()
            for sink in sinks:
                id = sink.proplist.get('node.nick', '')
                name = sink.proplist.get('node.nick', '')

                if id and name:
                    result.append({ 'id': id, 'name': name })

        return json.dumps(result)

class ArctisManagerDbusEQService(ServiceInterface):
    def __init__(self, core: CoreEngine):
        super().__init__(DBUS_EQ_INTERFACE_NAME)
        self.core_engine = core
        self.logger = logging.getLogger('ArctisManagerDbusEQService')

    @signal('EQSettingsChanged')
    def signal_eq_settings_changed(self, settings_json: 's') -> 's': # type: ignore
        return settings_json

    def _eq_settings_json(self) -> str:
        from linux_arctis_manager.settings import EQSettings
        s = EQSettings.load()
        return json.dumps({
            'media': {'enabled': s.media.enabled, 'mode': s.media.mode, 'preset_name': s.media.preset_name},
            'chat': {'enabled': s.chat.enabled, 'mode': s.chat.mode, 'preset_name': s.chat.preset_name},
            'app_overrides': [
                {'matcher_type': o.matcher_type, 'value': o.value, 'steam_app_id': o.steam_app_id,
                 'steam_game_name': o.steam_game_name, 'preset_name': o.preset_name, 'channel': o.channel}
                for o in s.app_overrides
            ],
        })

    @method('GetEQSettings')
    def get_eq_settings(self) -> 's': # type: ignore
        return self._eq_settings_json()

    @method('SetEQSettings')
    def set_eq_settings(self, settings_json: 's') -> 'b': # type: ignore
        try:
            from linux_arctis_manager.settings import EQSettings, ChannelEQSettings, EQAppOverride
            data = json.loads(settings_json)
            self.logger.info(
                'SetEQSettings: media(enabled=%s, preset=%r)  chat(enabled=%s, preset=%r)',
                data.get('media', {}).get('enabled'), data.get('media', {}).get('preset_name'),
                data.get('chat', {}).get('enabled'),  data.get('chat', {}).get('preset_name'),
            )
            new_settings = EQSettings()
            for ch in ('media', 'chat'):
                cd = data.get(ch, {})
                setattr(new_settings, ch, ChannelEQSettings(
                    enabled=cd.get('enabled', False),
                    mode=cd.get('mode', 'simple'),
                    preset_name=cd.get('preset_name'),
                ))
            for o in data.get('app_overrides', []):
                new_settings.app_overrides.append(EQAppOverride(
                    matcher_type=o.get('matcher_type', 'stream'),
                    value=o.get('value', ''),
                    steam_app_id=o.get('steam_app_id'),
                    steam_game_name=o.get('steam_game_name', ''),
                    preset_name=o.get('preset_name', 'flat'),
                    channel=o.get('channel', 'media'),
                ))
            new_settings.save()
            self.core_engine.reapply_eq()
            self.signal_eq_settings_changed(self._eq_settings_json())
            return True
        except Exception as e:
            self.logger.error('SetEQSettings: %s', e)
            return False

    @method('GetPresets')
    def get_presets(self) -> 's': # type: ignore
        from linux_arctis_manager.eq_preset import list_presets
        return json.dumps([
            {'name': p.name, 'mode': p.mode, 'description': p.description,
             'builtin': p.builtin,
             'bands': [{'frequency': b.frequency, 'gain': b.gain} for b in p.bands]}
            for p in list_presets()
        ])

    @method('SavePreset')
    def save_preset(self, preset_json: 's') -> 'b': # type: ignore
        try:
            from linux_arctis_manager.eq_preset import EQPreset, EQBand
            data = json.loads(preset_json)
            bands = [EQBand(frequency=b['frequency'], gain=float(b['gain'])) for b in data.get('bands', [])]
            EQPreset(
                name=data['name'],
                mode=data.get('mode', 'simple'),
                description=data.get('description', ''),
                bands=bands,
            ).save()
            return True
        except Exception as e:
            self.logger.error(f'SavePreset: {e}')
            return False

    @method('DeletePreset')
    def delete_preset(self, name: 's') -> 'b': # type: ignore
        try:
            from linux_arctis_manager.constants import EQ_PRESETS_FOLDER
            slug = name.lower().replace(' ', '_')
            path = EQ_PRESETS_FOLDER / f'{slug}.yaml'
            if path.exists():
                path.unlink()
            return True
        except Exception as e:
            self.logger.error(f'DeletePreset: {e}')
            return False

    @method('GetEQCapabilities')
    def get_eq_capabilities(self) -> 's': # type: ignore
        from linux_arctis_manager.eq_manager import ladspa_plugin_available, LADSPA_PLUGIN
        available = ladspa_plugin_available()
        if not available:
            self.logger.warning('GetEQCapabilities: LADSPA plugin %r not found — EQ unavailable', LADSPA_PLUGIN)
        return json.dumps({
            'ladspa_available': available,
            'ladspa_plugin': LADSPA_PLUGIN,
        })

    @method('GetSteamGames')
    def get_steam_games(self) -> 's': # type: ignore
        try:
            from linux_arctis_manager.steam_library import list_installed_games
            return json.dumps([{'app_id': g.app_id, 'name': g.name} for g in list_installed_games()])
        except Exception as e:
            self.logger.warning(f'GetSteamGames: {e}')
            return json.dumps([])

    @method('GetRunningStreams')
    def get_running_streams(self) -> 's': # type: ignore
        import pulsectl
        # System-level binaries/names that are never user media apps.
        _EXCLUDED_BINARIES = frozenset({
            'wireplumber', 'kwin_wayland', 'plasmashell', 'kded6',
            'xdg-desktop-portal', 'kdeconnectd', 'uresourced',
        })
        _EXCLUDED_NAMES = frozenset({
            'linux-arctis-manager', 'arctis-manager-stream-query',
            'arctis-eq-manager', 'arctis-eq-stream-monitor',
            'WirePlumber', 'WirePlumber [export]', 'KWin',
        })
        try:
            seen_lower: set[str] = set()
            result: list[str] = []

            def _add(name: str) -> None:
                if name and name.lower() not in seen_lower and name not in _EXCLUDED_NAMES:
                    seen_lower.add(name.lower())
                    result.append(name)

            with pulsectl.Pulse('arctis-manager-stream-query') as pulse:
                # Currently playing streams (most reliable source of application.name).
                for inp in pulse.sink_input_list():
                    _add(inp.proplist.get('application.name', ''))
                # Registered clients — covers paused apps that released their stream.
                for client in pulse.client_list():
                    binary = client.proplist.get('application.process.binary', '')
                    if binary not in _EXCLUDED_BINARIES:
                        _add(client.proplist.get('application.name', ''))

            return json.dumps(sorted(result))
        except Exception as e:
            self.logger.warning('GetRunningStreams: %s', e)
            return json.dumps([])


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
            (ArctisManagerDbusStatusService, DBUS_STATUS_OBJECT_PATH),
            (ArctisManagerDbusEQService, DBUS_EQ_OBJECT_PATH),
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
