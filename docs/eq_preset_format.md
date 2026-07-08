# EQ Preset Format Reference

## Overview

EQ presets are YAML files stored in `~/.config/arctis_manager/eq_presets/`. The file name is used as a slug; the `name` field inside the file is the human-readable label shown in the UI and referenced by `eq_settings.yaml`.

Presets are portable: they can be shared between users and will work as-is in a future preset repository.

## Preset File Schema

```yaml
name: Bass Boost           # Human-readable preset name (referenced in eq_settings.yaml)
mode: simple               # 'simple' (10 bands) or 'advanced' (15 bands)
description: |             # Optional free-text description
  Emphasises low frequencies for music listening.
bands:
  - frequency: 50          # Band centre frequency in Hz (informational)
    gain: 6.0              # Gain in dB; range approximately -70 to +30; 0.0 = flat
  - frequency: 100
    gain: 4.0
  # … one entry per band (10 for simple mode, 15 for advanced mode)
```

### Fields

| Field         | Type   | Required | Description |
|---------------|--------|----------|-------------|
| `name`        | string | yes      | Preset name; must match the `preset_name` value in `eq_settings.yaml` |
| `mode`        | string | yes      | `simple` or `advanced` |
| `description` | string | no       | Free-text description |
| `bands`       | list   | yes      | One entry per band; see table below |

### Band entry fields

| Field       | Type  | Description |
|-------------|-------|-------------|
| `frequency` | int   | Centre frequency in Hz (informational; must match the expected frequencies for the mode) |
| `gain`      | float | Gain in dB; 0.0 = flat; range approx −70 to +30 |

## Simple Mode Band Frequencies (10 bands)

Simple mode presets must contain exactly 10 band entries. The frequencies are:

| Index | Frequency |
|-------|-----------|
| 0     | 50 Hz     |
| 1     | 100 Hz    |
| 2     | 156 Hz    |
| 3     | 220 Hz    |
| 4     | 311 Hz    |
| 5     | 440 Hz    |
| 6     | 622 Hz    |
| 7     | 880 Hz    |
| 8     | 1250 Hz   |
| 9     | 10000 Hz  |

## Advanced Mode Band Frequencies (15 bands)

Advanced mode presets must contain exactly 15 band entries, corresponding directly to the 15 `mbeq_1197` LADSPA plugin control ports:

| Index | Frequency  |
|-------|------------|
| 0     | 50 Hz      |
| 1     | 100 Hz     |
| 2     | 156 Hz     |
| 3     | 220 Hz     |
| 4     | 311 Hz     |
| 5     | 440 Hz     |
| 6     | 622 Hz     |
| 7     | 880 Hz     |
| 8     | 1250 Hz    |
| 9     | 1750 Hz    |
| 10    | 2500 Hz    |
| 11    | 3500 Hz    |
| 12    | 5000 Hz    |
| 13    | 10000 Hz   |
| 14    | 20000 Hz   |

## Simple-Mode to mbeq_1197 Band Mapping

`mbeq_1197` always requires 15 control values. In simple mode the 10 user-facing bands map to the following mbeq band indices; the remaining 5 are fixed at 0.0 dB:

| Simple band index | mbeq band index | Frequency  |
|-------------------|-----------------|------------|
| 0                 | 0               | 50 Hz      |
| 1                 | 1               | 100 Hz     |
| 2                 | 2               | 156 Hz     |
| 3                 | 3               | 220 Hz     |
| —                 | 4               | 311 Hz (always 0.0 dB) |
| 4                 | 5               | 440 Hz     |
| —                 | 6               | 622 Hz (always 0.0 dB) |
| 5                 | 7               | 880 Hz     |
| —                 | 8               | 1250 Hz (always 0.0 dB) |
| 6                 | 9               | 1750 Hz    |
| —                 | 10              | 2500 Hz (always 0.0 dB) |
| 7                 | 11              | 3500 Hz    |
| —                 | 12              | 5000 Hz (always 0.0 dB) |
| 8                 | 13              | 10000 Hz   |
| 9                 | 14              | 20000 Hz   |

In Python terms: `SIMPLE_BAND_INDICES = [0, 1, 2, 3, 5, 7, 9, 11, 13, 14]`

## EQ Settings File Schema (`eq_settings.yaml`)

`~/.config/arctis_manager/eq_settings.yaml` controls which preset is active for each channel and defines per-application overrides.

```yaml
media:
  enabled: true            # true to activate EQ on the media channel
  mode: simple             # 'simple' or 'advanced'
  preset_name: Bass Boost  # must match the 'name' field of a preset file; null = flat
chat:
  enabled: false
  mode: simple
  preset_name: null
app_overrides:
  - matcher_type: stream      # 'stream' | 'executable' | 'steam'
    value: Firefox            # used for stream and executable matchers
    preset_name: Voice
    channel: media            # 'media' or 'chat'
  - matcher_type: executable
    value: mpv
    preset_name: Cinema
    channel: media
  - matcher_type: steam
    steam_app_id: 271590      # numeric Steam App ID
    steam_game_name: Grand Theft Auto V   # informational label only
    preset_name: Gaming
    channel: media
```

### AppMatcher types

| `matcher_type` | Match field                     | Required YAML field |
|----------------|---------------------------------|---------------------|
| `stream`       | `application.name` PA property  | `value`             |
| `executable`   | `application.process.binary` PA property | `value`    |
| `steam`        | `SteamGameId` env var or game executable name | `steam_app_id` |

## Portability

Preset files are fully self-contained YAML and do not reference any system path or device. They can be shared between users on different machines. A future preset repository will distribute preset files in this format without modification.
