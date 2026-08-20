import asyncio
import itertools
import json
import logging
import threading

from dbus_next.aio.message_bus import MessageBus
from dbus_next.service import ServiceInterface, method, signal

from linux_arctis_manager.config import DeviceConfiguration, parsed_status
from linux_arctis_manager.constants import (DBUS_BUS_NAME,
                                            DBUS_CONFIG_INTERFACE_NAME,
                                            DBUS_CONFIG_OBJECT_PATH,
                                            DBUS_EQ_INTERFACE_NAME,
                                            DBUS_EQ_OBJECT_PATH,
                                            DBUS_NC_INTERFACE_NAME,
                                            DBUS_NC_OBJECT_PATH,
                                            DBUS_SETTINGS_INTERFACE_NAME,
                                            DBUS_SETTINGS_OBJECT_PATH,
                                            DBUS_STATUS_INTERFACE_NAME,
                                            DBUS_STATUS_OBJECT_PATH,
                                            DBUS_VC_INTERFACE_NAME,
                                            DBUS_VC_OBJECT_PATH)
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

    @method('GetVersion')
    def get_version(self) -> 's': # type: ignore
        from linux_arctis_manager.utils import project_version
        return project_version()

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


class ArctisManagerDbusNCService(ServiceInterface):
    def __init__(self, core: CoreEngine):
        super().__init__(DBUS_NC_INTERFACE_NAME)
        self.core_engine = core
        self.logger = logging.getLogger('ArctisManagerDbusNCService')

    @method('GetNCCapabilities')
    def get_nc_capabilities(self) -> 's':  # type: ignore
        from linux_arctis_manager.nc_manager import rnnoise_available, swh_available
        import pulsectl
        rnnoise_ok = rnnoise_available()
        swh_ok = swh_available()
        if not rnnoise_ok:
            self.logger.warning('GetNCCapabilities: RNNoise LADSPA plugin not found')
        if not swh_ok:
            self.logger.warning('GetNCCapabilities: swh-plugins not found (HPF/gate/compressor unavailable)')
        try:
            with pulsectl.Pulse('arctis-nc-caps') as pulse:
                sources = [
                    {'id': s.name, 'name': s.description}
                    for s in pulse.source_list()
                    if not s.name.endswith('.monitor')
                ]
        except Exception as e:
            self.logger.error('GetNCCapabilities: failed to list sources: %s', e)
            sources = []
        return json.dumps({
            'rnnoise_available': rnnoise_ok,
            'swh_available': swh_ok,
            'sources': sources,
        })

    @method('GetNCSettings')
    def get_nc_settings(self) -> 's':  # type: ignore
        from linux_arctis_manager.settings import NCSettings
        s = NCSettings.load()
        return json.dumps(s._to_dict())

    @method('SetNCSettings')
    def set_nc_settings(self, settings_json: 's') -> 'b':  # type: ignore
        try:
            from linux_arctis_manager.settings import NCSettings
            data = json.loads(settings_json)
            s = NCSettings()
            s.preset    = data.get('preset', 'off')
            s.source_id = data.get('source_id', '')
            s.hpf_enabled = bool(data.get('hpf_enabled', False))
            g = data.get('gate', {})
            s.gate_enabled   = bool(g.get('enabled', False))
            s.gate_threshold = int(g.get('threshold', -42))
            s.gate_reduction = int(g.get('reduction', -72))
            s.gate_attack    = int(g.get('attack', 2))
            s.gate_release   = int(g.get('release', 450))
            c = data.get('compressor', {})
            s.comp_enabled   = bool(c.get('enabled', False))
            s.comp_threshold = int(c.get('threshold', -18))
            s.comp_ratio     = int(c.get('ratio', 18))
            s.comp_makeup    = int(c.get('makeup', 4))
            s.save()
            self.core_engine.reapply_nc()
            return True
        except Exception as e:
            self.logger.error('SetNCSettings failed: %s', e)
            return False


class ArctisManagerDbusVCService(ServiceInterface):
    def __init__(self, core: CoreEngine):
        super().__init__(DBUS_VC_INTERFACE_NAME)
        self.core_engine = core
        self.logger = logging.getLogger('ArctisManagerDbusVCService')
        self._installing = False
        self._downloading = False
        self._downloading_base = False

    @method('GetVCCapabilities')
    def get_vc_capabilities(self) -> 's':  # type: ignore
        import pulsectl
        from linux_arctis_manager.voice_changer.ladspa.effects import capabilities as ladspa_caps
        from linux_arctis_manager.voice_changer.rvc.registry import BackendRegistry
        from linux_arctis_manager.voice_changer.rvc.model_manager import RVCModelManager

        ladspa = ladspa_caps()
        rvc_backends = BackendRegistry.available_backends()
        rvc_models = [
            {'name': m.name, 'path': str(m.path), 'has_index': m.has_index}
            for m in RVCModelManager.list_models()
        ]

        try:
            with pulsectl.Pulse('arctis-vc-caps') as pulse:
                sources = [
                    {'id': s.name, 'name': s.description}
                    for s in pulse.source_list()
                    if not s.name.endswith('.monitor')
                ]
        except Exception as e:
            self.logger.error('GetVCCapabilities: failed to list sources: %s', e)
            sources = []

        from linux_arctis_manager.ai_deps import ai_env_exists
        from linux_arctis_manager.voice_changer.rvc.model_downloader import base_models_status
        return json.dumps({
            'sources': sources,
            'ladspa': ladspa,
            'rvc': {
                'available':    bool(rvc_backends),
                'backends':     rvc_backends,
                'models':       rvc_models,
                'models_folder': str(RVCModelManager.models_folder()),
                'ai_env_exists': ai_env_exists(),
                'base_models':  base_models_status(),
            },
        })

    @method('GetVCSettings')
    def get_vc_settings(self) -> 's':  # type: ignore
        from linux_arctis_manager.voice_changer.settings import VCSettings
        return json.dumps(VCSettings.load()._to_dict())

    @method('SetVCSettings')
    def set_vc_settings(self, settings_json: 's') -> 'b':  # type: ignore
        try:
            from linux_arctis_manager.voice_changer.settings import VCSettings
            data = json.loads(settings_json)
            s = VCSettings()
            s.enabled   = bool(data.get('enabled', False))
            s.mode      = str(data.get('mode', 'ladspa'))
            s.source_id = str(data.get('source_id', ''))

            p = data.get('pitch', {})
            s.pitch_enabled   = bool(p.get('enabled', False))
            s.pitch_semitones = float(p.get('semitones', 0.0))

            c = data.get('chorus', {})
            s.chorus_enabled    = bool(c.get('enabled', False))
            s.chorus_voices     = int(c.get('voices', 3))
            s.chorus_delay_ms   = float(c.get('delay_ms', 20.0))
            s.chorus_sep_ms     = float(c.get('sep_ms', 0.5))
            s.chorus_detune_pct = float(c.get('detune_pct', 1.0))
            s.chorus_lfo_hz     = float(c.get('lfo_hz', 4.0))
            s.chorus_atten_db   = float(c.get('atten_db', -3.0))

            d = data.get('delay', {})
            s.delay_enabled = bool(d.get('enabled', False))
            s.delay_s       = float(d.get('delay_s', 0.3))

            dist = data.get('distortion', {})
            s.distortion_enabled   = bool(dist.get('enabled', False))
            s.distortion_level     = float(dist.get('level', 0.3))
            s.distortion_character = float(dist.get('character', 0.5))

            r = data.get('reverb', {})
            s.reverb_enabled    = bool(r.get('enabled', False))
            s.reverb_roomsize_m = float(r.get('roomsize_m', 30.0))
            s.reverb_time_s     = float(r.get('time_s', 2.0))
            s.reverb_damping    = float(r.get('damping', 0.5))
            s.reverb_bandwidth  = float(r.get('bandwidth', 0.75))
            s.reverb_dry_db     = float(r.get('dry_db', -3.0))
            s.reverb_early_db   = float(r.get('early_db', -9.0))
            s.reverb_tail_db    = float(r.get('tail_db', -12.0))

            rv = data.get('rvc', {})
            s.rvc_model         = str(rv.get('model', ''))
            s.rvc_pitch_offset  = float(rv.get('pitch_offset', 0.0))
            s.rvc_hubert_model  = str(rv.get('hubert_model', 'torchaudio'))
            s.rvc_vtln_alpha    = float(rv.get('vtln_alpha', 1.0))
            s.rvc_rms_mix_rate  = float(rv.get('rms_mix_rate', 0.25))
            s.rvc_filter_radius = int(rv.get('filter_radius', 3))
            s.rvc_target_rms    = float(rv.get('target_rms', 0.06))
            s.rvc_limiter_thr   = float(rv.get('limiter_thr', 0.80))
            s.rvc_index_rate    = float(rv.get('index_rate', 0.0))
            s.rvc_model_params  = dict(rv.get('model_params', {}) or {})

            s.save()
            self.core_engine.reapply_vc()
            return True
        except Exception as e:
            self.logger.error('SetVCSettings: %s', e)
            return False

    @method('GetRVCModels')
    def get_rvc_models(self) -> 's':  # type: ignore
        from linux_arctis_manager.voice_changer.rvc.model_manager import RVCModelManager
        models = [{'name': m.name, 'path': str(m.path), 'has_index': m.has_index}
                  for m in RVCModelManager.list_models()]
        return json.dumps(models)

    @method('GetRVCMetrics')
    def get_rvc_metrics(self) -> 's':  # type: ignore
        """Drain per-hop quality metrics from the live RVC pipeline (auto-tuner)."""
        vc = getattr(self.core_engine, 'vc_manager', None)
        metrics = vc.rvc_metrics() if vc else None
        return json.dumps(metrics or {})

    @method('SetRVCLiveParams')
    def set_rvc_live_params(self, params_json: 's') -> 'b':  # type: ignore
        """Update RVC tuning params on the running chain without a rebuild.

        Does NOT persist — the GUI persists the converged values via
        SetVCSettings when the auto-tune session ends.
        """
        try:
            from linux_arctis_manager.voice_changer.rvc.backend import RVCParams
            p = json.loads(params_json)
            params = RVCParams(
                hubert_model=str(p.get('hubert_model', 'torchaudio')),
                vtln_alpha=float(p.get('vtln_alpha', 1.0)),
                rms_mix_rate=float(p.get('rms_mix_rate', 0.25)),
                filter_radius=int(p.get('filter_radius', 3)),
                target_rms=float(p.get('target_rms', 0.06)),
                limiter_thr=float(p.get('limiter_thr', 0.80)),
                index_rate=float(p.get('index_rate', 0.0)),
            )
            vc = getattr(self.core_engine, 'vc_manager', None)
            return bool(vc and vc.update_rvc_params(params))
        except Exception as e:
            self.logger.error('SetRVCLiveParams: %s', e)
            return False

    # ── Guided voice calibration ──────────────────────────────────────

    def _calibration(self):
        from linux_arctis_manager.voice_changer.rvc.calibration import CalibrationSession
        if not hasattr(self, '_calib_session') or self._calib_session is None:
            self._calib_session = CalibrationSession()
        return self._calib_session

    @method('CalibrationStartRecording')
    def calibration_start_recording(self) -> 'b':  # type: ignore
        from linux_arctis_manager.voice_changer.settings import VCSettings
        # Record from the same source the live VC chain consumes: the NC
        # output when noise cancellation is active, else the configured
        # source.  The settings source_id alone can be stale (it is ignored
        # by the live chain whenever the NC override is in effect).
        nc = getattr(self.core_engine, 'nc_manager', None)
        source = (nc.output_source if nc and getattr(nc, 'output_source', None) else '')
        if not source:
            source = VCSettings.load().source_id
        if not source:
            return False
        self.logger.info('calibration recording from %r', source)
        return self._calibration().record_start(source)

    @method('CalibrationStopRecording')
    def calibration_stop_recording(self) -> 's':  # type: ignore
        return self._calibration().record_stop()

    @method('CalibrationStartRender')
    def calibration_start_render(self, refine_params_json: 's') -> 'b':  # type: ignore
        """Render 3 variants of the last recording.

        refine_params_json: '' for the first round (contrast variants around
        the current tuning); a params dict of the chosen variant to run a
        narrower refine round around it.
        """
        try:
            from linux_arctis_manager.voice_changer.rvc.backend import RVCParams
            from linux_arctis_manager.voice_changer.rvc.calibration import propose_variants
            from linux_arctis_manager.voice_changer.rvc.model_manager import RVCModelManager
            from linux_arctis_manager.voice_changer.settings import VCSettings

            s = VCSettings.load()
            model = next((m for m in RVCModelManager.list_models()
                          if m.name == s.rvc_model), None)
            if model is None:
                self.logger.error('CalibrationStartRender: model %r not found', s.rvc_model)
                return False

            base = RVCParams(
                hubert_model=s.rvc_hubert_model, vtln_alpha=s.rvc_vtln_alpha,
                rms_mix_rate=s.rvc_rms_mix_rate, filter_radius=s.rvc_filter_radius,
                target_rms=s.rvc_target_rms, limiter_thr=s.rvc_limiter_thr,
                index_rate=s.rvc_index_rate)
            refine = None
            if refine_params_json:
                p = json.loads(refine_params_json)
                refine = RVCParams(
                    hubert_model=str(p.get('hubert_model', base.hubert_model)),
                    vtln_alpha=float(p.get('vtln_alpha', base.vtln_alpha)),
                    rms_mix_rate=float(p.get('rms_mix_rate', base.rms_mix_rate)),
                    filter_radius=int(p.get('filter_radius', base.filter_radius)),
                    target_rms=float(p.get('target_rms', base.target_rms)),
                    limiter_thr=float(p.get('limiter_thr', base.limiter_thr)),
                    index_rate=float(p.get('index_rate', base.index_rate)))
            variants = propose_variants(base, refine)
            return self._calibration().render_start(
                model.path, s.rvc_pitch_offset, variants)
        except Exception as e:
            self.logger.error('CalibrationStartRender: %s', e)
            return False

    @method('CalibrationGetStatus')
    def calibration_get_status(self) -> 's':  # type: ignore
        return json.dumps(self._calibration().status())

    @signal('InstallProgress')
    def signal_install_progress(self, message: 's') -> 's':  # type: ignore
        return message

    @signal('InstallComplete')
    def signal_install_complete(self, result_json: 's') -> 's':  # type: ignore
        return result_json

    @method('DetectGPU')
    def detect_gpu_method(self) -> 's':  # type: ignore
        from linux_arctis_manager.ai_deps import detect_gpu
        return json.dumps(detect_gpu())

    @method('InstallAIDeps')
    def install_ai_deps_method(self, backend: 's') -> None:  # type: ignore
        if self._installing:
            self.signal_install_progress('Already installing, please wait...')
            return
        self._installing = True
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            loop = asyncio.get_event_loop()
        threading.Thread(target=self._run_install, args=(backend, loop), daemon=True).start()

    def _run_install(self, backend: str, loop: asyncio.AbstractEventLoop) -> None:
        from linux_arctis_manager.ai_deps import install_ai_deps, activate_ai_env

        def progress(msg: str) -> None:
            try:
                loop.call_soon_threadsafe(self.signal_install_progress, msg)
            except Exception:
                pass

        try:
            success = install_ai_deps(backend, progress)
            result = json.dumps({
                'success': success,
                'message': 'Installation complete.' if success else 'Installation failed.',
            })
            try:
                loop.call_soon_threadsafe(self.signal_install_complete, result)
            except Exception:
                pass
            if success:
                activate_ai_env()
        finally:
            self._installing = False

    @signal('DownloadProgress')
    def signal_download_progress(self, message: 's') -> 's':  # type: ignore
        return message

    @signal('DownloadComplete')
    def signal_download_complete(self, result_json: 's') -> 's':  # type: ignore
        return result_json

    @signal('BaseModelDownloadProgress')
    def signal_base_model_progress(self, message: 's') -> 's':  # type: ignore
        return message

    @signal('BaseModelDownloadComplete')
    def signal_base_model_complete(self, result_json: 's') -> 's':  # type: ignore
        return result_json

    @method('SearchHFModels')
    async def search_hf_models(self, query: 's', sort_by: 's') -> 's':  # type: ignore
        from linux_arctis_manager.voice_changer.rvc.hf_search import search_models
        loop = asyncio.get_event_loop()
        results = await loop.run_in_executor(None, search_models, query, sort_by)
        return json.dumps(results)

    @method('ListRepoFiles')
    async def list_repo_files_method(self, repo_id: 's') -> 's':  # type: ignore
        from linux_arctis_manager.voice_changer.rvc.hf_search import list_repo_model_files
        loop = asyncio.get_event_loop()
        files = await loop.run_in_executor(None, list_repo_model_files, repo_id)
        return json.dumps(files)

    @method('DownloadHFModel')
    def download_hf_model_method(self, repo_id: 's', filename: 's') -> None:  # type: ignore
        if self._downloading:
            self.signal_download_progress('Already downloading, please wait...')
            return
        self._downloading = True
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            loop = asyncio.get_event_loop()
        threading.Thread(
            target=self._run_download, args=(repo_id, filename, loop), daemon=True,
        ).start()

    def _run_download(self, repo_id: str, filename: str, loop: asyncio.AbstractEventLoop) -> None:
        from linux_arctis_manager.voice_changer.rvc.hf_search import download_model
        from linux_arctis_manager.voice_changer.rvc.model_manager import RVCModelManager

        last_msg: list[str] = ['']

        def progress(msg: str) -> None:
            last_msg[0] = msg
            try:
                loop.call_soon_threadsafe(self.signal_download_progress, msg)
            except Exception:
                pass

        try:
            success, names = download_model(repo_id, filename, RVCModelManager.models_folder(), progress)
            primary = names[0] if names else ''
            if success:
                msg = f'Downloaded {", ".join(names)}.' if names else 'Done.'
            else:
                reason = last_msg[0]
                msg = (f'Download of {filename} failed: {reason}' if reason
                       else f'Download of {filename} failed.')
            result = json.dumps({'success': success, 'message': msg, 'name': primary})
            try:
                loop.call_soon_threadsafe(self.signal_download_complete, result)
            except Exception:
                pass
        finally:
            self._downloading = False

    @method('DeleteRVCModel')
    def delete_rvc_model_method(self, name: 's') -> 'b':  # type: ignore
        from linux_arctis_manager.voice_changer.rvc.hf_search import delete_model
        from linux_arctis_manager.voice_changer.rvc.model_manager import RVCModelManager
        return delete_model(name, RVCModelManager.models_folder())

    @method('DownloadBaseModels')
    def download_base_models_method(self) -> None:  # type: ignore
        if self._downloading_base:
            self.signal_base_model_progress('Already downloading, please wait...')
            return
        self._downloading_base = True
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            loop = asyncio.get_event_loop()
        threading.Thread(target=self._run_base_download, args=(loop,), daemon=True).start()

    def _run_base_download(self, loop: asyncio.AbstractEventLoop) -> None:
        from linux_arctis_manager.voice_changer.rvc.model_downloader import download_base_models

        def progress(msg: str) -> None:
            try:
                loop.call_soon_threadsafe(self.signal_base_model_progress, msg)
            except Exception:
                pass

        try:
            download_base_models(progress)
            result = json.dumps({'success': True, 'message': 'Base models downloaded successfully.'})
        except Exception as e:
            self.logger.error('DownloadBaseModels failed: %s', e)
            result = json.dumps({'success': False, 'message': str(e)})
        try:
            loop.call_soon_threadsafe(self.signal_base_model_complete, result)
        except Exception:
            pass
        finally:
            self._downloading_base = False

    @method('GetHFToken')
    def get_hf_token_method(self) -> 's':  # type: ignore
        from linux_arctis_manager.voice_changer.rvc.hf_search import get_hf_token
        return get_hf_token()

    @method('SetHFToken')
    def set_hf_token_method(self, token: 's') -> 'b':  # type: ignore
        from linux_arctis_manager.voice_changer.rvc.hf_search import set_hf_token
        return set_hf_token(token)


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
            (ArctisManagerDbusConfigService,  DBUS_CONFIG_OBJECT_PATH),
            (ArctisManagerDbusSettingsService, DBUS_SETTINGS_OBJECT_PATH),
            (ArctisManagerDbusStatusService,  DBUS_STATUS_OBJECT_PATH),
            (ArctisManagerDbusEQService,      DBUS_EQ_OBJECT_PATH),
            (ArctisManagerDbusNCService,      DBUS_NC_OBJECT_PATH),
            (ArctisManagerDbusVCService,      DBUS_VC_OBJECT_PATH),
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
