# Software EQ Feature

## Overview

Linux Arctis Manager supports a software equalizer that sits between the virtual PulseAudio sinks (`Arctis_Media` and `Arctis_Chat`) and the physical headset sink. The EQ is implemented using PulseAudio's `module-ladspa-sink` with the `mbeq_1197` plugin (15-band graphic EQ from `swh-plugins`).

EQ is configured per channel (media and/or chat) and can optionally apply different presets on a per-application basis.

EQ is **disabled by default**. All settings must currently be edited manually — a GUI for this feature is not yet implemented.

## Prerequisites

The `mbeq_1197` LADSPA plugin must be installed. Install the `swh-plugins` package for your distribution:

| Distribution       | Package name   |
|--------------------|----------------|
| Arch Linux / AUR   | `swh-plugins`  |
| Debian / Ubuntu    | `swh-plugins`  |
| Fedora             | `swh-plugins`  |

If the plugin is not found when the daemon starts, EQ setup is skipped with a warning and the daemon continues to operate normally.

## EQ Modes

### Simple mode (default)

Exposes 10 user-facing frequency bands that map to a subset of the 15 available `mbeq_1197` bands:

| Band | Frequency |
|------|-----------|
| 1    | 50 Hz     |
| 2    | 100 Hz    |
| 3    | 156 Hz    |
| 4    | 220 Hz    |
| 5    | 311 Hz    |
| 6    | 440 Hz    |
| 7    | 622 Hz    |
| 8    | 880 Hz    |
| 9    | 1250 Hz   |
| 10   | 10000 Hz  |

The remaining 5 mbeq bands not exposed in simple mode are always set to 0.0 dB.

### Advanced mode

Exposes all 15 `mbeq_1197` bands directly. See [EQ Preset Format](eq_preset_format.md) for the full list of frequencies.

## EQ Settings File

Settings are stored in `~/.config/arctis_manager/eq_settings.yaml`. Example:

```yaml
media:
  enabled: true
  mode: simple
  preset_name: Bass Boost
chat:
  enabled: false
  mode: simple
  preset_name: null
app_overrides: []
```

- `enabled`: `true` to activate EQ on this channel.
- `mode`: `simple` (10 bands) or `advanced` (15 bands).
- `preset_name`: Name of a preset file in `~/.config/arctis_manager/eq_presets/`. Set to `null` to use a flat (0 dB) EQ.

## EQ Presets

Presets are YAML files stored in `~/.config/arctis_manager/eq_presets/`. Each file defines a named set of band gains. See [EQ Preset Format](eq_preset_format.md) for the full schema.

## Per-Application EQ Overrides

You can route specific applications to a custom EQ preset using `app_overrides` in `eq_settings.yaml`. Three matcher types are supported:

### Stream name matcher

Matches a PulseAudio stream by its `application.name` property.

```yaml
app_overrides:
  - matcher_type: stream
    value: Firefox
    preset_name: Voice
    channel: media
```

### Executable matcher

Matches a stream by the binary name of the process that opened it (`application.process.binary`).

```yaml
app_overrides:
  - matcher_type: executable
    value: mpv
    preset_name: Cinema
    channel: media
```

### Steam game matcher

Matches a stream by the Steam `SteamGameId` environment variable or, as a fallback, the game's known executable names.

```yaml
app_overrides:
  - matcher_type: steam
    steam_app_id: 271590
    steam_game_name: Grand Theft Auto V
    preset_name: Gaming
    channel: media
```

The `steam_game_name` field is informational only (displayed in future GUI); the matching is done on `steam_app_id`.

### Steam integration

Steam game matching requires the `vdf` Python package (listed in the project's dependencies) and a working Steam installation. The daemon reads `libraryfolders.vdf` to discover all Steam library paths and resolves game information from `appmanifest_*.acf` files.

If `vdf` is not installed, Steam matching is silently disabled and returns no matches (the daemon does not crash).

## Architecture

When EQ is enabled for a channel, the daemon inserts a LADSPA sink between the virtual null-sink and the physical headset output:

```
Application → Arctis_Media (null-sink) → Arctis_Media_EQ (ladspa-sink) → physical headset
Application → Arctis_Chat  (null-sink) → Arctis_Chat_EQ  (ladspa-sink) → physical headset
```

When EQ is disabled for a channel, the loopback goes directly to the physical sink (no LADSPA sink is created).

The stream monitor watches for new PulseAudio sink-inputs and moves matching streams to the appropriate EQ sink based on `app_overrides`.
