# V2 → V3 feature gap

> V2 = Python daemon (`src/linux_arctis_manager/`)
> V3 = Rust daemon (`daemon/engine/`)

---

## D-Bus interfaces

| Interface | V2 class | V3 |
|---|---|---|
| `…Settings` | `ArctisManagerDbusSettingsService` | **Done** |
| `…Status` | `ArctisManagerDbusStatusService` | **Done** |
| `…Config` | `ArctisManagerDbusConfigService` | **Done** |
| `…EQ` | `ArctisManagerDbusEQService` | **Done** |
| `…NC` | `ArctisManagerDbusNCService` | **Missing** |
| `…VC` | `ArctisManagerDbusVCService` | **Missing** |

---

## Settings interface

| Feature | V2 | V3 |
|---|---|---|
| `GetSettings` — `device` section (current field values) | `device_settings.settings` | **Done** — plain values from YAML `apis` |
| `GetSettings` — `settings_config` (device schemas) | per-device YAML `settings` list | **Done** — inferred from YAML `apis` field defs; maps to `slider`/`toggle`/`discrete_map` |
| `GetSettings` — `general` section | `GeneralSettings.to_dict()` | **Done** — returns all 3 fields with current values |
| `GetSettings` — `settings_config` for general fields | `GeneralSettings.settings_config` | **Done** — toggle/select schemas included |
| `SetSetting` (device fields) | writes `device_settings` + sends HID | **Done** — `WriteApi` command via DSL |
| `SetSetting` (general fields) | writes `GeneralSettings` + persists | **Done** — persists to `general_settings.yaml` |
| `GetListOptions("pulse_audio_devices")` | enumerates PulseAudio sinks | **Done** — returns `{id: node.name, name: node.nick}`; stable across renames (v2 stored `node.nick` as both id and name — bug fixed) |
| `GetVersion` | method on Settings interface | **Done** — method on Settings; version sourced from shared `VERSION` file at build time |
| `SettingsChanged` signal | fired on any setting write | **Done** — emitted by `SetSetting` and `ReloadConfigs`; GUI subscribes |

---

## GeneralSettings (3 fields, persisted to `general_settings.yaml`)

| Field | Type | V3 |
|---|---|---|
| `redirect_audio_on_connect` | toggle | **Done** — redirects default sink to `Arctis_Media` on headset connect |
| `redirect_audio_on_disconnect` | toggle | **Done** — redirects default sink to chosen device on wireless disconnect |
| `redirect_audio_on_disconnect_device` | select (PulseAudio sink `node.name`) | **Done** — stores `node.name` (stable ALSA path); v2 bug with `node.nick` fixed |

V2 action on connect: `pactl set-default-sink Arctis_Media`.
V2 action on disconnect: `pactl set-default-sink <chosen_device>`.
V2 bug: stored `node.nick` as the device id — rename breaks redirect. V3 fix: store `node.name`.

---

## Device settings persistence

| Feature | V2 | V3 |
|---|---|---|
| Load defaults from YAML on first run | yes | yes (YAML `apis` field ranges) |
| Persist user overrides to `<vid>_<pid>.yaml` | yes | **Done** — saved to `settings/<vid:04x>_<pid:04x>.yaml` after each `SetSetting` |
| Re-apply persisted settings on reconnect | yes | **Done** — loaded and sent as `WriteApi` before event loop starts |

---

## Status interface

| Feature | V2 | V3 |
|---|---|---|
| `GetStatus` + `StatusChanged` signal | yes | **Done** |
| Status grouped by category (headset / mic / bluetooth / wireless) | yes — YAML `representation` dict | **Done** — `representation:` section in device YAML maps categories to field lists; fallback to single `"headset"` when absent |
| Field `type` values: `percentage`, `on_off`, `label` | yes — YAML `status_parse` | **Done** — `display_type` on `SyncEventField`/`SyncReadMap` propagates through `EmitEvent.display_types`; battery, mic volume, sidetone, brightness, gain, and BT toggle fields annotated |
| `DeviceConnected(pid, name, capabilities[])` signal | missing in V2 | **New in V3** |
| `DeviceDisconnected(pid)` signal | missing in V2 | **New in V3** |

---

## Audio routing

| Feature | V2 | V3 |
|---|---|---|
| `Arctis_Media` + `Arctis_Chat` null-sinks | yes | **Done** |
| Chatmix volume split | yes | **Done** |
| Wireless audio lifecycle (create on connect, teardown on disconnect) | yes | **Done** |
| Physical sink discovery with retry | yes | **Done** |
| Redirect default sink on headset connect/disconnect | yes (`GeneralSettings`) | **Done** — hooks in `run_device` and event-forwarding task |
| EQ LADSPA loopback routing (`Arctis_Media` → mbeq) | yes | **Done** — `eq_manager`: swaps channel loopback target, live gain update without reload |
| NC virtual mic source (`Arctis_NC_Mic`) | yes | **Missing** |
| VC virtual sink (`Arctis_VC_Sink`) | yes | **Missing** |
| Mic routing chain (NC → VC → `Arctis_Manager_Mic`) | yes | **Missing** |

---

## Device management

| Feature | V2 | V3 |
|---|---|---|
| USB hotplug (add/remove) | pyudev | **Done** — tokio-udev |
| YAML config loading + hot-reload via D-Bus | yes | **Done** |
| Device selector: prefer PID match, fall back to product string | yes | **Done** — prefers PID+interface match, falls back to PID-only |
| Reactive wireless reconnect (wait for dongle event) | polling | **Done** — blocks on `read_any_report(30s)`, wakes on dongle notification |
| Device settings file persistence | yes | **Done** — `device_persistence` module; `settings/<vid>_<pid>.yaml` |
| USB kernel driver detach/reattach | yes (pyusb) | N/A — hidraw-helper sidecar replaces direct USB access |

---

## Equalizer (EQ)

### Architecture (V3 design)

Three preset band modes:

| `band_mode` | Bands | Format | HW backend | LADSPA backend |
|---|---|---|---|---|
| `fixed_10` | 10 fixed frequencies | gain only, ±12 dB | Nova Pro wired/wireless, Nova Elite | `mbeq_1197` simple |
| `parametric_10` | 10 free-frequency | freq + filter_type + gain | Nova 3/5/7 Gen2 | `mbeq_1197` advanced (15b, 5 locked@0) |
| `fixed_5` | 5 fixed frequencies | gain only | Arctis 5 | `mbeq_1197` simple (5/10 active) |

Backend selection per channel:
- `auto`: hardware if device supports that `band_mode`, else LADSPA fallback
- `ladspa`: always software (user-selectable regardless of device capability)
- `hardware`: force HID EQ; silent fallback to LADSPA if unsupported

App override activation:
- LADSPA backend → PipeWire stream monitor (same as V2)
- Hardware backend → foreground window monitor (compositor D-Bus)

Preset YAML format (`~/.config/arctis_manager/eq_presets/<name>.yaml`):
```yaml
name: "Bass Boost"
band_mode: fixed_10       # or parametric_10, fixed_5
bands:
  - gain: 4.0             # fixed_10 / fixed_5: gain only
    # parametric_10 adds: frequency: <Hz>, filter_type: peaking|low_shelf|high_shelf
```

### Implementation status

| Feature | V2 | V3 |
|---|---|---|
| `…EQ` D-Bus interface (9 methods, 1 signal) | `ArctisManagerDbusEQService` | **Done** — `GetEQCapabilities`, `GetEQSettings`, `SetEQSetting`, `ListPresets`, `GetPreset`, `SavePreset`, `DeletePreset`, `GetRunningStreams`, `GetSteamGames`; `EQChanged` signal |
| Per-channel (media/chat) enable, backend, band_mode, preset | `EQSettings.{media,chat}` | **Done** — `eq::settings`: `ChannelEqSettings`, `EqBackend` (auto/ladspa/hardware), YAML persistence |
| `GetEQCapabilities()` → `{has_hw_eq, hw_band_mode}` | N/A (v2 software-only) | **Done** — always returns `has_hw_eq: false` (device YAML EQ API support not yet wired) |
| Hardware EQ via HID (fixed_10, parametric_10, fixed_5) | N/A | **Partial** — `eq::hardware` encodes payloads for NovaPro and ParametricNova; device YAML EQ APIs not yet declared |
| LADSPA `mbeq_1197` pipeline (10-band simple, 15-band advanced) | `EQManager` | **Done** — `eq::ladspa` + `eq_manager`: all 3 band modes, live gain update, routing swap |
| Preset library (YAML files in `eq_presets/`) | `list_presets()`, `EQPreset` | **Done** — `eq::preset`: `BandMode` (fixed_10/parametric_10/fixed_5), save/load/list |
| App-aware overrides (stream / executable / Steam game) | `EQAppOverride` | **Partial** — data model in `eq::settings` (`AppMatcher`, `AppOverride`); activation logic not yet wired |
| PipeWire stream monitor (LADSPA backend app override) | `EQManager.start_stream_monitor()` | **Missing** |
| Foreground window monitor (hardware backend app override) | N/A | **Missing** |
| `GetSteamGames` (Steam library scan) | `steam_library.py` | **Done** — ACF VDF parser, sorted by name |
| `GetRunningStreams` (PulseAudio client list) | `get_running_streams()` | **Done** — `pactl -f json list clients`, filters internal PipeWire clients |

---

## Noise cancellation (NC)

Everything in this section is **Missing** in V3.

| Feature | V2 |
|---|---|
| `…NC` D-Bus interface (3 methods) | `ArctisManagerDbusNCService` |
| Preset: off / light / standard / studio / custom | `NCSettings.preset` |
| RNNoise LADSPA pipeline | `NCManager` |
| HPF, noise gate, compressor stages (swh-plugins) | `NCSettings.{hpf,gate,comp}_*` |
| Virtual mic source routing | `MicRouter` |
| `GetNCCapabilities` (checks RNNoise + swh availability) | yes |

---

## Voice changer (VC)

Everything in this section is **Missing** in V3.

| Feature | V2 |
|---|---|
| `…VC` D-Bus interface (18 methods, 4 signals) | `ArctisManagerDbusVCService` |
| LADSPA chain: pitch / chorus / delay / distortion / reverb | `VCSettings`, `VoiceChangerManager` |
| RVC (Retrieval-based Voice Conversion) inference | `rvc/pipeline.py` |
| Per-model param snapshot | `VCSettings.rvc_model_params` |
| Live parameter update without pipeline rebuild | `SetRVCLiveParams` |
| Calibration wizard (record → render variants → pick) | `Calibration{Start,Stop}Recording`, `CalibrationStartRender` |
| GPU detection | `ai_deps.py` |
| AI deps install (pip in venv, with progress signal) | `InstallAIDeps`, `InstallProgress/Complete` |
| HuggingFace model search / browse / download | `SearchHFModels`, `DownloadHFModel`, … |
| HF token management | `GetHFToken`, `SetHFToken` |

---

## GUI tabs

| Tab | Widget | V3 daemon coverage |
|---|---|---|
| Status | `QStatusWidget` | **Done** — fields grouped by category via YAML `representation` |
| General | `QSettingsWidget(section='general')` | **Done** — general section populated with 3 fields and their schemas |
| Device | `QSettingsWidget(section='device')` | **Done** — renders sliders/toggles from V3 settings_config |
| Equalizer | `QEQWidget` | **Partial** — daemon-side D-Bus EQ interface done; GUI not yet updated |
| Microphone (NC + VC) | `QMicWidget`, `QNCWidget`, `QVCWidget` | **Missing** — no NC/VC interfaces |
