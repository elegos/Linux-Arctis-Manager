from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ruamel.yaml import YAML

from linux_arctis_manager.config import ConfigSetting, SettingType
from linux_arctis_manager.constants import SETTINGS_FOLDER
from linux_arctis_manager.utils import JsonSerializable, ObservableDict


class DeviceSettings(JsonSerializable):
    vendor_id: int
    product_id: int

    settings: ObservableDict[str, int]

    def __init__(self, vendor_id: int, product_id: int):
        self.vendor_id = vendor_id
        self.product_id = product_id
        self.settings = ObservableDict()

    def _settings_file(self) -> Path:
        settings_file = SETTINGS_FOLDER / f'{self.vendor_id:04x}_{self.product_id:04x}.yaml'

        return settings_file

    def read_from_file(self):
        settings_file = self._settings_file()

        if not settings_file.exists():
            return

        yaml = YAML(typ='safe')
        raw = yaml.load(settings_file) or {}

        for key in raw:
            # Clean old / invalid settings
            if key in self.settings:
                self.settings[key] = int(raw[key])

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
        yaml.dump(self.settings.to_dict(), settings_file)

    def to_dict(self) -> dict:
        return self.__dict__


class GeneralSettings(JsonSerializable):
    _js_exclude_fields = ['settings_config']

    # Automatically redirect on Media channel
    redirect_audio_on_connect: bool = False

    # When disconnecting, redirect to this device
    redirect_audio_on_disconnect: bool = False
    redirect_audio_on_disconnect_device: str|None = None

    settings_config: list[ConfigSetting] = [
        ConfigSetting('redirect_audio_on_connect', SettingType.TOGGLE, False, values={ 'on': True, 'off': False, 'off_label': 'off', 'on_label': 'on' }),
        ConfigSetting('redirect_audio_on_disconnect', SettingType.TOGGLE, False, values={ 'on': True, 'off': False, 'off_label': 'off', 'on_label': 'on' }),
        ConfigSetting('redirect_audio_on_disconnect_device', SettingType.SELECT, None, options_source='pulse_audio_devices', options_mapping={ 'value': 'id', 'label': 'description' }),
    ]

    def __init__(self, **kwargs):
        for key, value in kwargs.items():
            if key in self.__class__.__annotations__:
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


# ---------------------------------------------------------------------------
# EQ Settings
# ---------------------------------------------------------------------------

EQ_SETTINGS_FILE = Path.home() / '.config' / 'arctis_manager' / 'eq_settings.yaml'


@dataclass
class ChannelEQSettings:
    enabled: bool = False
    mode: str = 'simple'               # 'simple' | 'advanced'
    preset_name: str | None = None     # None = flat (all zeros)


@dataclass
class EQAppOverride:
    matcher_type: str                  # 'stream' | 'executable' | 'steam'
    value: str = ''
    steam_app_id: int | None = None
    steam_game_name: str = ''
    preset_name: str = 'flat'
    channel: str = 'media'


import logging as _logging
_eq_log = _logging.getLogger('EQSettings')


class EQSettings:
    media: ChannelEQSettings
    chat: ChannelEQSettings
    app_overrides: list[EQAppOverride]

    def __init__(self) -> None:
        self.media = ChannelEQSettings()
        self.chat = ChannelEQSettings()
        self.app_overrides = []

    def save(self) -> None:
        EQ_SETTINGS_FILE.parent.mkdir(parents=True, exist_ok=True)
        yaml = YAML()
        data = {
            'media': {
                'enabled': self.media.enabled,
                'mode': self.media.mode,
                'preset_name': self.media.preset_name,
            },
            'chat': {
                'enabled': self.chat.enabled,
                'mode': self.chat.mode,
                'preset_name': self.chat.preset_name,
            },
            'app_overrides': [
                {
                    'matcher_type': o.matcher_type,
                    'value': o.value,
                    'steam_app_id': o.steam_app_id,
                    'steam_game_name': o.steam_game_name,
                    'preset_name': o.preset_name,
                    'channel': o.channel,
                }
                for o in self.app_overrides
            ],
        }
        with open(EQ_SETTINGS_FILE, 'w') as f:
            yaml.dump(data, f)

    @classmethod
    def load(cls) -> EQSettings:
        instance = cls()
        if not EQ_SETTINGS_FILE.exists():
            _eq_log.debug('EQ settings file not found (%s) — using defaults', EQ_SETTINGS_FILE)
            return instance
        try:
            yaml = YAML(typ='safe')
            data = yaml.load(EQ_SETTINGS_FILE)
            if not data:
                _eq_log.warning('EQ settings file is empty')
                return instance
            for channel in ('media', 'chat'):
                ch = data.get(channel, {})
                cfg = ChannelEQSettings(
                    enabled=ch.get('enabled', False),
                    mode=ch.get('mode', 'simple'),
                    preset_name=ch.get('preset_name'),
                )
                setattr(instance, channel, cfg)
                _eq_log.debug(
                    'Loaded EQ [%s]: enabled=%s  mode=%s  preset=%r',
                    channel, cfg.enabled, cfg.mode, cfg.preset_name,
                )
            overrides = data.get('app_overrides', [])
            for o in overrides:
                instance.app_overrides.append(EQAppOverride(
                    matcher_type=o.get('matcher_type', 'stream'),
                    value=o.get('value', ''),
                    steam_app_id=o.get('steam_app_id'),
                    steam_game_name=o.get('steam_game_name', ''),
                    preset_name=o.get('preset_name', 'flat'),
                    channel=o.get('channel', 'media'),
                ))
            _eq_log.debug('Loaded %d app override(s)', len(overrides))
        except Exception as exc:
            _eq_log.error('Failed to parse EQ settings: %s', exc)
        return instance

    def to_eq_config(self) -> EQConfig:
        from linux_arctis_manager.eq_manager import ChannelEQConfig, EQConfig
        from linux_arctis_manager.eq_preset import EQPreset, list_presets
        from linux_arctis_manager.app_matcher import AppEQOverride, AppMatcher

        presets_by_name = {p.name: p for p in list_presets()}
        _eq_log.debug('Available presets: %s', list(presets_by_name.keys()))

        def resolve_preset(ch: ChannelEQSettings, channel: str) -> EQPreset | None:
            if not ch.enabled:
                _eq_log.debug('to_eq_config [%s]: EQ disabled', channel)
                return None
            if ch.preset_name and ch.preset_name in presets_by_name:
                preset = presets_by_name[ch.preset_name]
                controls = preset.to_ladspa_controls()
                _eq_log.info(
                    'Applying EQ [%s]: preset=%r  mode=%s  gains=%s',
                    channel, preset.name, preset.mode,
                    ' '.join(f'{v:+.1f}' for v in controls),
                )
                return preset
            _eq_log.warning(
                'EQ [%s]: preset_name=%r not found — falling back to flat',
                channel, ch.preset_name,
            )
            return EQPreset.flat(mode=ch.mode)  # type: ignore[arg-type]

        media_cfg = ChannelEQConfig(
            enabled=self.media.enabled,
            mode=self.media.mode,  # type: ignore[arg-type]
            preset=resolve_preset(self.media, 'media'),
        )
        chat_cfg = ChannelEQConfig(
            enabled=self.chat.enabled,
            mode=self.chat.mode,  # type: ignore[arg-type]
            preset=resolve_preset(self.chat, 'chat'),
        )

        overrides = []
        for o in self.app_overrides:
            matcher = AppMatcher(
                type=o.matcher_type,  # type: ignore[arg-type]
                value=o.value,
                app_id=o.steam_app_id,
                name=o.steam_game_name,
            )
            preset = presets_by_name.get(o.preset_name) or EQPreset.flat()
            overrides.append(AppEQOverride(matcher=matcher, preset_name=preset.name, channel=o.channel))

        return EQConfig(media=media_cfg, chat=chat_cfg, app_overrides=overrides)
