import asyncio
import json
import logging
from threading import Thread
from time import sleep

from dbus_next.aio.message_bus import MessageBus
from dbus_next.aio.proxy_object import ProxyInterface
from dbus_next.constants import MessageType
from dbus_next.message import Message
from PySide6.QtCore import QObject, Signal, SignalInstance

from linux_arctis_manager.constants import (DBUS_BUS_NAME,
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


# ── V3 EQ translation ─────────────────────────────────────────────────────────
# V3 daemon uses band_mode ('fixed10'/'parametric10'/'fixed5') and 'preset'.
# The GUI uses mode ('simple'/'advanced') and 'preset_name'.
# All translation lives here so eq_widget.py stays unchanged.

_MODE_TO_BAND_MODE: dict[str, str] = {'simple': 'fixed10', 'advanced': 'parametric10'}
_BAND_MODE_TO_MODE: dict[str, str] = {
    'fixed10': 'simple', 'fixed5': 'simple', 'parametric10': 'advanced',
}
# Fixed display frequencies per band_mode (the daemon stores only gain, no freq).
_BAND_MODE_FREQS: dict[str, list[int]] = {
    'fixed10': [50, 100, 220, 440, 880, 1750, 3500, 5000, 10000, 20000],
    'fixed5':  [50, 440, 1750, 5000, 20000],
}


def _v3_settings_to_gui(s3: dict) -> dict:
    """Translate V3 EqSettings JSON to the format the GUI expects."""
    result: dict = {'app_overrides': []}
    for ch in ('media', 'chat'):
        cd = s3.get(ch, {})
        bm = cd.get('band_mode', 'fixed10')
        p = cd.get('preset', 'Flat')
        result[ch] = {
            'enabled': cd.get('enabled', False),
            'mode': _BAND_MODE_TO_MODE.get(bm, 'simple'),
            'preset_name': None if p.lower() == 'flat' else p,
            'backend': cd.get('backend', 'auto'),
        }
        # V3 AppMatcher is internally-tagged: {"type": "stream", "name": "..."}
        for ov in cd.get('app_overrides', []):
            matcher = ov.get('matcher', {})
            mt_type = matcher.get('type', '')
            if mt_type == 'stream':
                mt, val, app_id, gname = 'stream', matcher.get('name', ''), None, ''
            elif mt_type == 'executable':
                mt, val, app_id, gname = 'executable', matcher.get('path', ''), None, ''
            elif mt_type == 'steam_game':
                mt, val, app_id = 'steam', '', matcher.get('app_id')
                gname = ''
            else:
                continue
            ov_entry: dict = {
                'matcher_type': mt, 'value': val,
                'steam_app_id': app_id, 'steam_game_name': gname,
                'preset_name': ov.get('preset', ''), 'channel': ch,
            }
            hw_idx = ov.get('hw_preset_idx')
            if hw_idx is not None:
                ov_entry['hw_preset_idx'] = hw_idx
            result['app_overrides'].append(ov_entry)
    return result


def _gui_overrides_to_v3(overrides: list, channel: str) -> list:
    """Convert GUI V2 app overrides for one channel to V3 AppOverride format."""
    result = []
    for ov in overrides:
        if ov.get('channel') != channel:
            continue
        mt = ov.get('matcher_type', '')
        if mt == 'stream':
            matcher = {'type': 'stream', 'name': ov.get('value', '')}
        elif mt == 'executable':
            matcher = {'type': 'executable', 'path': ov.get('value', '')}
        elif mt == 'steam':
            matcher = {'type': 'steam_game', 'app_id': ov.get('steam_app_id')}
        else:
            continue
        entry: dict = {'matcher': matcher, 'preset': ov.get('preset_name', '')}
        hw_idx = ov.get('hw_preset_idx')
        if hw_idx is not None:
            entry['hw_preset_idx'] = hw_idx
        result.append(entry)
    return result


def _v3_preset_to_gui(p3: dict) -> dict:
    """Translate a V3 EqPreset to the GUI's V2 format."""
    bm = p3.get('band_mode', 'fixed10')
    bands_v3 = p3.get('bands', [])
    if bm == 'parametric10':
        mode = 'advanced'
        bands = [{'frequency': b.get('frequency', 1000), 'gain': b.get('gain', 0.0)}
                 for b in bands_v3]
    else:
        freqs = _BAND_MODE_FREQS.get(bm, _BAND_MODE_FREQS['fixed10'])
        mode = 'simple'
        bands = [{'frequency': f, 'gain': b.get('gain', 0.0)}
                 for f, b in zip(freqs, bands_v3)]
    return {'name': p3['name'], 'mode': mode, 'description': '', 'builtin': False, 'bands': bands}


def _gui_preset_to_v3(p2: dict) -> dict:
    """Translate a GUI V2 preset to V3 format for SavePreset."""
    mode = p2.get('mode', 'simple')
    bm = _MODE_TO_BAND_MODE.get(mode, 'fixed10')
    bands_v2 = p2.get('bands', [])
    if bm == 'parametric10':
        bands = [{'gain': b['gain'], 'frequency': b['frequency'], 'filter_type': 'peaking'}
                 for b in bands_v2]
    else:
        bands = [{'gain': b['gain']} for b in bands_v2]
    return {'name': p2['name'], 'band_mode': bm, 'bands': bands}


class DbusWrapper(QObject):
    sig_status = Signal(object)
    sig_settings = Signal(object)
    sig_device_connected = Signal(object)
    sig_device_disconnected = Signal()
    sig_ai_progress = Signal(str)
    sig_ai_complete = Signal(bool, str)
    sig_download_progress = Signal(str)
    sig_download_complete = Signal(bool, str, str)  # (success, message, model_name)
    sig_base_model_progress = Signal(str)
    sig_base_model_complete = Signal(bool, str)

    logger = logging.getLogger('DbusWrapper')

    def __init__(self, parent: QObject|None = None):
        super().__init__(parent)

        self._dbus: MessageBus|None = None
        self._stopping = False

        self._status_signal_loop: asyncio.AbstractEventLoop|None = None
        self._stop_status_signal_future: asyncio.Future|None = None

        self._settings_signal_loop: asyncio.AbstractEventLoop|None = None
        self._stop_settings_signal_future: asyncio.Future|None = None

        self._vc_signal_loop: asyncio.AbstractEventLoop|None = None
        self._stop_vc_signal_future: asyncio.Future|None = None

        self._status_iface: ProxyInterface|None = None
        self._settings_iface: ProxyInterface|None = None

    async def status_iface(self):
        if not self._status_iface:
            bus = await MessageBus().connect()
            introspection = await bus.introspect(DBUS_BUS_NAME, DBUS_STATUS_OBJECT_PATH)
            obj = bus.get_proxy_object(DBUS_BUS_NAME, DBUS_STATUS_OBJECT_PATH, introspection)
            self._status_iface = obj.get_interface(DBUS_STATUS_INTERFACE_NAME)

        return self._status_iface

    async def settings_iface(self):
        if not self._settings_iface:
            bus = await MessageBus().connect()
            introspection = await bus.introspect(DBUS_BUS_NAME, DBUS_SETTINGS_OBJECT_PATH)
            obj = bus.get_proxy_object(DBUS_BUS_NAME, DBUS_SETTINGS_OBJECT_PATH, introspection)
            self._settings_iface = obj.get_interface(DBUS_SETTINGS_INTERFACE_NAME)

        return self._settings_iface

    def start(self):
        self.request_status()
        self.request_settings()

        status_signal_thread = Thread(target=lambda: asyncio.run(self._register_status_dbus_signal()))
        status_signal_thread.start()

        settings_signal_thread = Thread(target=lambda: asyncio.run(self._register_settings_dbus_signal()))
        settings_signal_thread.start()

        vc_signal_thread = Thread(target=lambda: asyncio.run(self._register_vc_signals()), daemon=True)
        vc_signal_thread.start()
    
    async def _register_status_dbus_signal(self):
        try:
            def callback(status: str) -> None:
                self.sig_status.emit(json.loads(status) or {})

            def on_connected(pid: int, name: str, caps: list) -> None:
                self.sig_device_connected.emit({'pid': pid, 'name': name, 'capabilities': caps})

            def on_disconnected(pid: int) -> None:
                self.sig_device_disconnected.emit()

            iface = await self.status_iface()
            iface.on_status_changed(callback)  # type: ignore
            iface.on_device_connected(on_connected)  # type: ignore
            iface.on_device_disconnected(on_disconnected)  # type: ignore

            self._status_signal_loop = asyncio.get_running_loop()
            self._stop_status_signal_future = self._status_signal_loop.create_future()
            await self._stop_status_signal_future
        except Exception as e:
            self.logger.warning('status signal registration failed: %s', e)

    async def _register_settings_dbus_signal(self):
        try:
            def callback(settings_json: str) -> None:
                self.sig_settings.emit(json.loads(settings_json) or {})

            (await self.settings_iface()).on_settings_changed(callback) # type: ignore

            self._settings_signal_loop = asyncio.get_running_loop()
            self._stop_settings_signal_future = self._settings_signal_loop.create_future()
            await self._stop_settings_signal_future
        except Exception as e:
            self.logger.warning('settings signal registration failed: %s', e)

    async def _register_vc_signals(self):
        try:
            bus = await MessageBus().connect()
            introspection = await bus.introspect(DBUS_BUS_NAME, DBUS_VC_OBJECT_PATH)
            obj = bus.get_proxy_object(DBUS_BUS_NAME, DBUS_VC_OBJECT_PATH, introspection)
            iface = obj.get_interface(DBUS_VC_INTERFACE_NAME)

            def on_progress(message: str) -> None:
                self.sig_ai_progress.emit(message)

            def on_complete(result_json: str) -> None:
                try:
                    data = json.loads(result_json)
                    self.sig_ai_complete.emit(data.get('success', False), data.get('message', ''))
                except Exception:
                    self.sig_ai_complete.emit(False, result_json)

            def on_dl_progress(message: str) -> None:
                self.sig_download_progress.emit(message)

            def on_dl_complete(result_json: str) -> None:
                try:
                    data = json.loads(result_json)
                    self.sig_download_complete.emit(
                        data.get('success', False),
                        data.get('message', ''),
                        data.get('name', ''),
                    )
                except Exception:
                    self.sig_download_complete.emit(False, result_json, '')

            def on_base_progress(message: str) -> None:
                self.sig_base_model_progress.emit(message)

            def on_base_complete(result_json: str) -> None:
                try:
                    data = json.loads(result_json)
                    self.sig_base_model_complete.emit(
                        data.get('success', False), data.get('message', ''))
                except Exception:
                    self.sig_base_model_complete.emit(False, result_json)

            iface.on_install_progress(on_progress)                      # type: ignore
            iface.on_install_complete(on_complete)                       # type: ignore
            iface.on_download_progress(on_dl_progress)                   # type: ignore
            iface.on_download_complete(on_dl_complete)                   # type: ignore
            iface.on_base_model_download_progress(on_base_progress)     # type: ignore
            iface.on_base_model_download_complete(on_base_complete)     # type: ignore

            self._vc_signal_loop = asyncio.get_running_loop()
            self._stop_vc_signal_future = self._vc_signal_loop.create_future()
            await self._stop_vc_signal_future
        except Exception as e:
            self.logger.warning('VC signal registration failed: %s', e)

    def stop(self):
        self.logger.info("Stopping D-Bus wrapper...")
        self._stopping = True
        if self._status_signal_loop and self._stop_status_signal_future:
            self._status_signal_loop.call_soon_threadsafe(self._stop_status_signal_future.set_result, None)
        if self._settings_signal_loop and self._stop_settings_signal_future:
            self._settings_signal_loop.call_soon_threadsafe(self._stop_settings_signal_future.set_result, None)
        if self._vc_signal_loop and self._stop_vc_signal_future:
            self._vc_signal_loop.call_soon_threadsafe(self._stop_vc_signal_future.set_result, None)

    def request_status(self) -> None:
        request_thread = Thread(target=lambda: asyncio.run(self._request_status_async()))
        request_thread.start()

    async def _request_status_async(self):
        try:
            iface = await self.status_iface()
            result = await iface.call_get_status() # type: ignore
            self.sig_status.emit(json.loads(result) or {})
        except Exception as e:
            self.logger.warning('request_status failed: %s', e)

    @staticmethod
    def request_service_version(qt_signal: SignalInstance) -> None:
        async def _call():
            try:
                bus = await MessageBus().connect()
                reply = await bus.call(Message(
                    destination=DBUS_BUS_NAME,
                    path=DBUS_SETTINGS_OBJECT_PATH,
                    interface=DBUS_SETTINGS_INTERFACE_NAME,
                    member='GetVersion',
                    message_type=MessageType.METHOD_CALL,
                    signature='',
                    body=[],
                ))
                if reply is not None and reply.message_type != MessageType.ERROR:
                    qt_signal.emit(reply.body[0])
                else:
                    # Method not found on older service — emit empty string so
                    # the handler knows to restart the service.
                    DbusWrapper.logger.warning('GetVersion not available on service (old version)')
                    qt_signal.emit('')
            except Exception as e:
                DbusWrapper.logger.warning('GetVersion failed: %s', e)
                qt_signal.emit('')
        Thread(target=lambda: asyncio.run(_call())).start()

    def request_settings(self) -> None:
        request_thread = Thread(target=lambda: asyncio.run(self._request_settings_async()))
        request_thread.start()

    async def _request_settings_async(self):
        try:
            iface = await self.settings_iface()
            result = await iface.call_get_settings() # type: ignore
            self.sig_settings.emit(json.loads(result) or {})
        except Exception as e:
            self.logger.warning('request_settings failed: %s', e)
    
    @staticmethod
    def request_list_options(list_name: str, qt_signal: SignalInstance):
        request_thread = Thread(target=DbusWrapper.request_list_options_thread, kwargs={'list_name': list_name, 'qt_signal': qt_signal})
        request_thread.start()
    
    @staticmethod
    def request_list_options_thread(list_name: str, qt_signal: SignalInstance):
        asyncio.run(DbusWrapper.request_list_options_async(list_name, qt_signal))
    
    @staticmethod
    async def request_list_options_async(list_name: str, qt_signal: SignalInstance):
        dbus_bus = await MessageBus().connect()
        reply = await dbus_bus.call(Message(
            destination=DBUS_BUS_NAME,
            path=DBUS_SETTINGS_OBJECT_PATH,
            interface=DBUS_SETTINGS_INTERFACE_NAME,
            member='GetListOptions',
            message_type=MessageType.METHOD_CALL,
            signature='s',
            body=[list_name],
        ))

        if reply is None:
            DbusWrapper.logger.error('Error getting settings: no reply')

        elif reply.message_type == MessageType.ERROR:
            DbusWrapper.logger.error('Error getting settings: %s', reply.body)

        else:
            obj = {'name': list_name, 'list': json.loads(reply.body[0]) or []}
            qt_signal.emit(obj)

    @staticmethod
    def request_eq_capabilities(qt_signal: SignalInstance) -> None:
        async def _call() -> None:
            try:
                caps_v3 = await DbusWrapper._eq_call_json('GetEqCapabilities', '', [])
                qt_signal.emit({
                    'ladspa_available': caps_v3.get('ladspa_available', True),
                    'ladspa_plugin': 'mbeq_1197',
                    'has_hw_eq': caps_v3.get('has_hw_eq', False),
                    'hw_band_mode': caps_v3.get('hw_band_mode'),
                    'hw_override_backend': caps_v3.get('hw_override_backend', 'unsupported'),
                    'hw_override_unsupported_reason': caps_v3.get('hw_override_unsupported_reason'),
                })
            except Exception as e:
                DbusWrapper.logger.warning(f'EQ GetEQCapabilities failed: {e}')
        Thread(target=lambda: asyncio.run(_call())).start()

    @staticmethod
    def request_eq_settings(qt_signal: SignalInstance) -> None:
        async def _call() -> None:
            try:
                s3 = await DbusWrapper._eq_call_json('GetEqSettings', '', [])
                qt_signal.emit(_v3_settings_to_gui(s3))
            except Exception as e:
                DbusWrapper.logger.warning(f'EQ GetEQSettings failed: {e}')
        Thread(target=lambda: asyncio.run(_call())).start()

    @staticmethod
    def request_eq_presets(qt_signal: SignalInstance) -> None:
        async def _call() -> None:
            try:
                summaries = await DbusWrapper._eq_call_json('ListPresets', '', [])
                presets = []
                for s in summaries:
                    p3 = await DbusWrapper._eq_call_json('GetPreset', 's', [s['name']])
                    if p3:
                        presets.append(_v3_preset_to_gui(p3))
                qt_signal.emit(presets)
            except Exception as e:
                DbusWrapper.logger.warning(f'EQ request_eq_presets failed: {e}')
        Thread(target=lambda: asyncio.run(_call())).start()

    @staticmethod
    def request_steam_games(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_eq_async('GetSteamGames', '', [], qt_signal, is_json=True))).start()

    @staticmethod
    def request_running_streams(qt_signal: SignalInstance) -> None:
        async def _call() -> None:
            try:
                streams = await DbusWrapper._eq_call_json('GetRunningStreams', '', [])
                # V3 returns [{name, pid}]; GUI expects [str].
                if streams and isinstance(streams[0], dict):
                    streams = [s['name'] for s in streams]
                qt_signal.emit(streams)
            except Exception as e:
                DbusWrapper.logger.warning(f'EQ GetRunningStreams failed: {e}')
        Thread(target=lambda: asyncio.run(_call())).start()

    @staticmethod
    def _gui_ch_to_v3(cd: dict, overrides: list) -> dict:
        """Convert one GUI channel dict to the V3 ChannelEqSettings format."""
        bm = _MODE_TO_BAND_MODE.get(cd.get('mode', 'simple'), 'fixed10')
        return {
            'enabled':  cd.get('enabled', False),
            'backend':  cd.get('backend', 'auto'),
            'band_mode': bm,
            'preset':   cd.get('preset_name') or 'Flat',
            'app_overrides': overrides,
        }

    @staticmethod
    def set_channel_eq_settings(channel: str, cd: dict, app_overrides: list) -> None:
        """Send all settings for one channel as a single atomic D-Bus call."""
        body = DbusWrapper._gui_ch_to_v3(cd, _gui_overrides_to_v3(app_overrides, channel))
        async def _call() -> None:
            try:
                await DbusWrapper._eq_call_raw(
                    'SetEqChannelSettings', 'ss', [channel, json.dumps(body)])
            except Exception as e:
                DbusWrapper.logger.warning(f'EQ set_channel_eq_settings ({channel}) failed: {e}')
        Thread(target=lambda: asyncio.run(_call())).start()

    @staticmethod
    def set_eq_settings(settings: dict) -> None:
        """Send settings for both channels (used only by the explicit Apply button)."""
        for ch in ('media', 'chat'):
            DbusWrapper.set_channel_eq_settings(
                ch, settings.get(ch, {}), settings.get('app_overrides', []))

    @staticmethod
    def save_eq_preset(preset: dict) -> None:
        Thread(target=lambda: asyncio.run(
            DbusWrapper._call_eq_async('SavePreset', 's', [json.dumps(_gui_preset_to_v3(preset))])
        )).start()

    @staticmethod
    def delete_eq_preset(name: str) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_eq_async('DeletePreset', 's', [name]))).start()

    @staticmethod
    def apply_factory_eq_preset(preset_idx: int, qt_signal: SignalInstance) -> None:
        async def _call() -> None:
            try:
                dbus_bus = await MessageBus().connect()
                reply = await dbus_bus.call(Message(
                    destination=DBUS_BUS_NAME,
                    path=DBUS_SETTINGS_OBJECT_PATH,
                    interface=DBUS_SETTINGS_INTERFACE_NAME,
                    member='SetSetting',
                    message_type=MessageType.METHOD_CALL,
                    signature='ss',
                    body=['eq_preset', json.dumps(preset_idx)],
                ))
                ok = (reply is not None
                      and reply.message_type != MessageType.ERROR
                      and bool(reply.body)
                      and reply.body[0])
                qt_signal.emit({'ok': bool(ok), 'error': '' if ok else 'SetSetting returned false'})
            except Exception as e:
                qt_signal.emit({'ok': False, 'error': str(e)})
        Thread(target=lambda: asyncio.run(_call())).start()

    @staticmethod
    def apply_hw_eq_preset(preset_name: str, qt_signal: SignalInstance) -> None:
        async def _call() -> None:
            try:
                result = await DbusWrapper._eq_call_raw('ApplyHwPreset', 's', [preset_name])
                qt_signal.emit({'ok': result == '', 'error': result or ''})
            except Exception as e:
                qt_signal.emit({'ok': False, 'error': str(e)})
        Thread(target=lambda: asyncio.run(_call())).start()

    @staticmethod
    async def _eq_call_raw(member: str, signature: str, body: list) -> str | None:
        """Low-level EQ call; returns raw reply string or None on error."""
        bus = await MessageBus().connect()
        reply = await bus.call(Message(
            destination=DBUS_BUS_NAME, path=DBUS_EQ_OBJECT_PATH,
            interface=DBUS_EQ_INTERFACE_NAME, member=member,
            message_type=MessageType.METHOD_CALL, signature=signature, body=body,
        ))
        if reply and reply.message_type != MessageType.ERROR and reply.body:
            return reply.body[0]
        return None

    @staticmethod
    async def _eq_call_json(member: str, signature: str, body: list) -> dict | list:
        """EQ call that returns parsed JSON, or empty dict on error."""
        raw = await DbusWrapper._eq_call_raw(member, signature, body)
        if raw:
            return json.loads(raw)
        return {}

    @staticmethod
    async def _call_eq_async(member: str, signature: str, body: list,
                             qt_signal: SignalInstance | None = None, is_json: bool = False) -> None:
        try:
            raw = await DbusWrapper._eq_call_raw(member, signature, body)
            if qt_signal is not None and raw is not None:
                result = json.loads(raw) if is_json else raw
                qt_signal.emit(result)
        except Exception as e:
            DbusWrapper.logger.warning(f'EQ DBus call {member} failed: {e}')

    @staticmethod
    def request_nc_capabilities(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_nc_async('GetNCCapabilities', '', [], qt_signal, is_json=True))).start()

    @staticmethod
    def request_nc_settings(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_nc_async('GetNCSettings', '', [], qt_signal, is_json=True))).start()

    @staticmethod
    def set_nc_settings(settings: dict) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_nc_async('SetNCSettings', 's', [json.dumps(settings)]))).start()

    @staticmethod
    async def _call_nc_async(member: str, signature: str, body: list,
                             qt_signal: SignalInstance | None = None, is_json: bool = False) -> None:
        try:
            bus = await MessageBus().connect()
            reply = await bus.call(Message(
                destination=DBUS_BUS_NAME,
                path=DBUS_NC_OBJECT_PATH,
                interface=DBUS_NC_INTERFACE_NAME,
                member=member,
                message_type=MessageType.METHOD_CALL,
                signature=signature,
                body=body,
            ))
            if qt_signal is not None and reply is not None and reply.message_type != MessageType.ERROR:
                result = json.loads(reply.body[0]) if is_json else reply.body[0]
                qt_signal.emit(result)
        except Exception as e:
            DbusWrapper.logger.warning(f'NC DBus call {member} failed: {e}')

    @staticmethod
    def request_vc_capabilities(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('GetVCCapabilities', '', [], qt_signal, is_json=True))).start()

    @staticmethod
    def request_vc_settings(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('GetVCSettings', '', [], qt_signal, is_json=True))).start()

    @staticmethod
    def set_vc_settings(settings: dict) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('SetVCSettings', 's', [json.dumps(settings)]))).start()

    @staticmethod
    def request_rvc_models(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('GetRVCModels', '', [], qt_signal, is_json=True))).start()

    @staticmethod
    def request_rvc_metrics(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('GetRVCMetrics', '', [], qt_signal, is_json=True))).start()

    @staticmethod
    def set_rvc_live_params(params: dict) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('SetRVCLiveParams', 's', [json.dumps(params)]))).start()

    @staticmethod
    def calibration_start_recording(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('CalibrationStartRecording', '', [], qt_signal))).start()

    @staticmethod
    def calibration_stop_recording(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('CalibrationStopRecording', '', [], qt_signal))).start()

    @staticmethod
    def calibration_start_render(refine_params: dict | None, qt_signal: SignalInstance) -> None:
        payload = json.dumps(refine_params) if refine_params else ''
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('CalibrationStartRender', 's', [payload], qt_signal))).start()

    @staticmethod
    def calibration_get_status(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('CalibrationGetStatus', '', [], qt_signal, is_json=True))).start()

    @staticmethod
    def detect_gpu(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('DetectGPU', '', [], qt_signal, is_json=True))).start()

    @staticmethod
    def install_ai_deps(backend: str) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async('InstallAIDeps', 's', [backend]))).start()

    @staticmethod
    def search_hf_models(query: str, sort_by: str, qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async(
            'SearchHFModels', 'ss', [query, sort_by], qt_signal, is_json=True))).start()

    @staticmethod
    def list_repo_model_files(repo_id: str, qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async(
            'ListRepoFiles', 's', [repo_id], qt_signal, is_json=True))).start()

    @staticmethod
    def download_hf_model(repo_id: str, filename: str) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async(
            'DownloadHFModel', 'ss', [repo_id, filename]))).start()

    @staticmethod
    def delete_rvc_model(name: str, qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async(
            'DeleteRVCModel', 's', [name], qt_signal, is_json=False))).start()

    @staticmethod
    def get_hf_token(qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async(
            'GetHFToken', '', [], qt_signal, is_json=False))).start()

    @staticmethod
    def set_hf_token(token: str, qt_signal: SignalInstance) -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async(
            'SetHFToken', 's', [token], qt_signal, is_json=False))).start()

    @staticmethod
    def download_base_models() -> None:
        Thread(target=lambda: asyncio.run(DbusWrapper._call_vc_async(
            'DownloadBaseModels', '', []))).start()

    @staticmethod
    async def _call_vc_async(member: str, signature: str, body: list,
                             qt_signal: SignalInstance | None = None, is_json: bool = False) -> None:
        try:
            bus = await MessageBus().connect()
            reply = await bus.call(Message(
                destination=DBUS_BUS_NAME,
                path=DBUS_VC_OBJECT_PATH,
                interface=DBUS_VC_INTERFACE_NAME,
                member=member,
                message_type=MessageType.METHOD_CALL,
                signature=signature,
                body=body,
            ))
            if qt_signal is not None and reply is not None and reply.message_type != MessageType.ERROR:
                result = json.loads(reply.body[0]) if is_json else reply.body[0]
                qt_signal.emit(result)
        except Exception as e:
            DbusWrapper.logger.warning(f'VC DBus call {member} failed: {e}')

    @staticmethod
    def change_setting(name: str, value: int|bool|str) -> None:
        request_thread = Thread(target=DbusWrapper.change_setting_thread, kwargs={'name': name, 'value': value})
        request_thread.start()
    
    @staticmethod
    def change_setting_thread(name: str, value: int|bool|str):
        asyncio.run(DbusWrapper.change_setting_async(name, value))
    
    @staticmethod
    async def change_setting_async(name: str, value: int|bool|str):
        dbus_bus = await MessageBus().connect()
        await dbus_bus.call(Message(
            destination=DBUS_BUS_NAME,
            path=DBUS_SETTINGS_OBJECT_PATH,
            interface=DBUS_SETTINGS_INTERFACE_NAME,
            member='SetSetting',
            message_type=MessageType.METHOD_CALL,
            signature='ss',
            body=[name, json.dumps(value)],
        ))
    