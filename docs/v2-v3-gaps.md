# V2 → V3 feature gap

> V2 = Python daemon (`src/linux_arctis_manager/`)
> V3 = Rust daemon (`daemon/engine/`)

---

## D-Bus interfaces

| Interface | V2 class | V3 |
|---|---|---|
| `…Settings` | `ArctisManagerDbusSettingsService` | **Partial** — see Settings section |
| `…Status` | `ArctisManagerDbusStatusService` | **Done** |
| `…Config` | `ArctisManagerDbusConfigService` | **Done** |
| `…EQ` | `ArctisManagerDbusEQService` | **Missing** |
| `…NC` | `ArctisManagerDbusNCService` | **Missing** |
| `…VC` | `ArctisManagerDbusVCService` | **Missing** |

---

## Settings interface

| Feature | V2 | V3 |
|---|---|---|
| `GetSettings` — `device` section (current field values) | `device_settings.settings` | **Done** — plain values from YAML `apis` |
| `GetSettings` — `settings_config` (device schemas) | per-device YAML `settings` list | **Done** — inferred from YAML `apis` field defs; maps to `slider`/`toggle`/`discrete_map` |
| `GetSettings` — `general` section | `GeneralSettings.to_dict()` | **Missing** — always `{}` |
| `GetSettings` — `settings_config` for general fields | `GeneralSettings.settings_config` | **Missing** |
| `SetSetting` (device fields) | writes `device_settings` + sends HID | **Done** — `WriteApi` command via DSL |
| `SetSetting` (general fields) | writes `GeneralSettings` + persists | **Missing** |
| `GetListOptions("pulse_audio_devices")` | enumerates PulseAudio sinks | **Done** — returns `{id: node.name, name: node.nick}`; stable across renames (v2 stored `node.nick` as both id and name — bug fixed) |
| `GetVersion` | method on Settings interface | **Done** — method on Settings; version sourced from shared `VERSION` file at build time |
| `SettingsChanged` signal | fired on any setting write | **Done** — emitted by `SetSetting` and `ReloadConfigs`; GUI subscribes |

---

## GeneralSettings (3 fields, persisted to `general_settings.yaml`)

| Field | Type | V3 |
|---|---|---|
| `redirect_audio_on_connect` | toggle | **Missing** |
| `redirect_audio_on_disconnect` | toggle | **Missing** |
| `redirect_audio_on_disconnect_device` | select (PulseAudio sink `node.nick`) | **Missing** — V3 will store `node.name` (stable ALSA path) instead of `node.nick`; `GetListOptions` already returns correct pairs |

V2 action on connect: `pactl set-default-sink Arctis_Media`.
V2 action on disconnect: `pactl set-default-sink <chosen_device>`.
V2 bug: stored `node.nick` as the device id — rename breaks redirect. V3 fix: store `node.name`.

---

## Device settings persistence

| Feature | V2 | V3 |
|---|---|---|
| Load defaults from YAML on first run | yes | yes (YAML `apis` field ranges) |
| Persist user overrides to `<vid>_<pid>.yaml` | yes | **Missing** — settings lost on daemon restart |
| Re-apply persisted settings on reconnect | yes | **Missing** |

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
| Redirect default sink on headset connect/disconnect | yes (`GeneralSettings`) | **Missing** |
| EQ LADSPA loopback routing (`Arctis_Media` → mbeq) | yes | **Missing** |
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
| Device settings file persistence | yes | **Missing** |
| USB kernel driver detach/reattach | yes (pyusb) | N/A — hidraw-helper sidecar replaces direct USB access |

---

## Equalizer (EQ)

Everything in this section is **Missing** in V3.

| Feature | V2 |
|---|---|
| `…EQ` D-Bus interface (8 methods, 1 signal) | `ArctisManagerDbusEQService` |
| Per-channel (media/chat) enable, simple/advanced mode, preset | `EQSettings.{media,chat}` |
| 10-band (simple) and 15-band (advanced) LADSPA `mbeq_1197` pipeline | `EQManager` |
| Preset library (YAML files in `eq_presets/`) | `list_presets()`, `EQPreset` |
| App-aware overrides (match by stream / executable / Steam game) | `EQAppOverride` |
| PulseAudio stream monitor for app override activation | `EQManager.start_stream_monitor()` |
| `GetSteamGames` (Steam library scan) | `steam_library.py` |
| `GetRunningStreams` (PulseAudio client list) | `get_running_streams()` |

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
| General | `QSettingsWidget(section='general')` | **Missing** — `general` section always empty |
| Device | `QSettingsWidget(section='device')` | **Done** — renders sliders/toggles from V3 settings_config |
| Equalizer | `QEQWidget` | **Missing** — no EQ D-Bus interface |
| Microphone (NC + VC) | `QMicWidget`, `QNCWidget`, `QVCWidget` | **Missing** — no NC/VC interfaces |
