# Equalizer

Two independent channels — **media** and **chat** — can each run their own EQ,
with their own backend, band layout, active preset, and per-app overrides.

## Band modes

| Mode | Bands | Layout | Used by |
|---|---|---|---|
| `fixed_10` | 10 | Fixed frequencies, gain only | Nova Pro family |
| `parametric_10` | 10 | Free frequency + filter type per band | Nova 3/5/7 Gen2, Nova Elite |
| `fixed_5` | 5 | Fixed frequencies, gain only | Arctis 5 |

A preset's `band_mode` must match one of these; band count and (for
`parametric_10`) per-band `frequency`/`filter_type` are validated accordingly.

## Backend

Every channel picks a backend independently:

- **`hardware`** — sends EQ commands straight to the headset over HID. Only
  works if the device declares `custom_eq` for the active band mode.
- **`ladspa`** — software EQ via PipeWire's `mbeq_1197` (fixed 15-band plugin,
  always available regardless of what the headset supports). Preset bands are
  mapped onto its 15 fixed control ports (`fixed_10`/`fixed_5`: direct index
  mapping; `parametric_10`: nearest-frequency, gains summed on collision).
- **`auto`** (default) — hardware when the device supports it, LADSPA
  otherwise.

## Presets

Stored as individual YAML files under `~/.config/arctis_manager/eq_presets/`:

```yaml
name: Bass Boost
band_mode: fixed_10
bands:
  - gain: 6.0
  - gain: 4.0
  # ... one entry per band (10 for fixed_10/parametric_10, 5 for fixed_5)
```

`parametric_10` bands additionally require `frequency` (Hz) and `filter_type`
(`low_shelf` / `peaking` / `high_shelf`) per band.

Channel settings — which backend, band mode, and preset are active — live in
`~/.config/arctis_manager/eq_settings.yaml`, one section per channel.

## Per-app overrides

Each channel can route specific apps/games to a different preset (and
optionally a different backend), matched by:

| Matcher | Matches on |
|---|---|
| `stream` | PipeWire stream's application name |
| `executable` | Full path of the audio-producing process |
| `steam_game` | Steam AppID (resolved from the process, no `vdf`/manifest parsing needed) |

## D-Bus

Exposed on the `EQ` interface (`GetEqCapabilities`, `GetEqSettings`,
`SetEqSetting`, `SetEqChannelSettings`, `ListPresets`, `GetPreset`,
`SavePreset`, `DeletePreset`, `ApplyHwPreset`; `EqChanged` signal) — see
[`dbus.md`](dbus.md) for the general D-Bus conventions shared across daemon
interfaces.
