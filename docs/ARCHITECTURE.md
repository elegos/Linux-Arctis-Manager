# Linux Arctis Manager v3 — Architecture

## Overview

v3 replaces the Python engine with a Rust daemon while keeping the Python GUI and existing D-Bus interface. The primary goals are:

- **Full protocol fidelity**: implement every command and event defined in the official SteelSeries device specifications, rather than the subset that was previously reverse-engineered ad-hoc.
- **System-wide service**: one daemon instance serves all logged-in users; no per-user process required.
- **No udev rules**: privilege is granted to the engine binary itself, not to device nodes.
- **Capability-driven device support**: devices declare which protocol capabilities they use; the engine provides the implementation for each capability generically.

## Component Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│  User session                                                   │
│                                                                 │
│  ┌──────────────────┐      D-Bus (session bus)                 │
│  │  GUI (Python/Qt) │ ◄──────────────────────────────────┐    │
│  └──────────────────┘                                     │    │
│                                                           │    │
│  ┌──────────────────┐      D-Bus (session bus)            │    │
│  │  CLI / other     │ ◄──────────────────────────────────┤    │
│  └──────────────────┘                                     │    │
│                                                           │    │
│  ┌────────────────────────────────────────────────────────┴──┐ │
│  │  arctis-engine  (Rust, systemd user service)              │ │
│  │                                                           │ │
│  │  ┌─────────────┐  ┌──────────────┐  ┌─────────────────┐  │ │
│  │  │ Device      │  │ Config       │  │ D-Bus server    │  │ │
│  │  │ Manager     │  │ (YAML DSL)   │  │ (zbus)          │  │ │
│  │  └──────┬──────┘  └──────────────┘  └─────────────────┘  │ │
│  │         │                                                  │ │
│  │  ┌──────▼──────┐  ┌──────────────┐                        │ │
│  │  │ HID event   │  │ Capability   │                        │ │
│  │  │ dispatcher  │  │ modules      │                        │ │
│  │  └─────────────┘  └──────────────┘                        │ │
│  │                                                           │ │
│  │  ┌─────────────────────────────────────────────────────┐  │ │
│  │  │  hid-transport  (hidapi / hidraw)                   │  │ │
│  │  └───────────────────────┬─────────────────────────────┘  │ │
│  └──────────────────────────│───────────────────────────────┘ │
└─────────────────────────────│─────────────────────────────────┘
                              │ fd (opened by helper)
┌─────────────────────────────▼─────────────────────────────────┐
│  arctis-hid-opener  (tiny setcap binary, ~100 LOC)            │
│  Validates VID+PID → opens /dev/hidraw* → passes fd via socket│
└─────────────────────────────┬─────────────────────────────────┘
                              │
              ┌───────────────▼───────────────┐
              │  /dev/hidraw*   (kernel)       │
              └───────────────────────────────┘

  ┌──────────────────────────────────────────┐
  │  Voice changer  (Python, separate proc)  │
  │  PipeWire filter-chain management        │
  │  Communicates via D-Bus (device state)   │
  └──────────────────────────────────────────┘
```

## Privilege Model

Linux does not expose a capability scoped to HID devices specifically. The two viable approaches are:

- **udev rules** matching VID+PID → set group ownership on device nodes.
- **Privileged helper** with `CAP_DAC_OVERRIDE` that opens device nodes on behalf of the engine.

v3 uses the **privileged helper** approach (`arctis-hid-opener`) to avoid requiring udev rules during installation and to centralise the privilege surface.

### `arctis-hid-opener`

A minimal binary (~100 lines of Rust or C) installed with:

```
chown root:root /usr/local/libexec/arctis-hid-opener
chmod 755       /usr/local/libexec/arctis-hid-opener
setcap cap_dac_override+eip /usr/local/libexec/arctis-hid-opener
```

`setcap` stores the capability in an extended attribute on the inode. Replacing the binary creates a new inode that does **not** inherit the attribute; regaining the capability requires root to re-run `setcap`. A non-root attacker who replaces the binary cannot escalate privileges through it.

The helper enforces two invariants before opening any device node:

1. **Peer authentication**: accepts connections only from processes matching the engine's UID (verified via `SO_PEERCRED` on the Unix domain socket).
2. **VID allowlist**: reads the USB device's `idVendor` sysfs attribute before opening; refuses any device whose VID is not in the compiled-in SteelSeries allowlist (`0x1038`). This prevents the helper from being used to open HID keyboards or mice.

### Engine

The engine itself runs with no elevated privileges as a systemd user service:

```ini
# ~/.config/systemd/user/arctis-engine.service
[Service]
ExecStart=/usr/local/bin/arctis-engine
Restart=on-failure
```

It communicates with the helper over a Unix domain socket at a well-known path. Once a file descriptor is received, all subsequent I/O (read, write, ioctl) operates on the already-open fd — no capability is needed.

## D-Bus Interface

The engine exposes its interface on the **session bus** (`DBUS_SESSION_BUS_ADDRESS`). Because the service runs in the user session, no polkit policy is required for clients in the same session to call methods.

The bus name and interface paths are unchanged from v2 (`name.giacomofurlan.ArctisManager.Next.*`) so the existing Python GUI and CLI require no modification.

See [`dbus.md`](dbus.md) for the full method and signal reference.

## HID Transport

The engine operates two logical channels per device:

| Channel | Direction | Usage |
|---|---|---|
| **Command** | Engine → Device | Send settings, status requests, init sequence |
| **Sync interface** | Device → Engine | Unsolicited push events (knob turns, battery changes, connection state) |

Both channels are opened on the same hidraw file descriptor; they are distinguished by HID report IDs and the transport type used (`HID_IO` 64-byte reports vs `HID_FEATURE` reports up to 1024 bytes).

Hot-plug detection uses `tokio-udev` to receive `add`/`remove` events from the kernel without polling. When a known VID+PID appears, the engine requests the fd from the helper, runs the device init sequence, and begins listening on the sync interface.

## Configuration System

Device behaviour is described in YAML files following a DSL that mirrors the structure of the official SteelSeries `.device` specification files. The DSL is split across two file types:

- **Base files** (`base_arctis_nova_pro_wireless.yaml`, etc.) define the full protocol for a device family: structs, APIs, value transforms, event dispatch, and lifecycle hooks.
- **Device files** (`arctis_nova_pro_wireless.yaml`, etc.) extend a base file and specify USB identifiers, firmware versions, and any per-variant overrides.

The Rust engine is a generic interpreter of this DSL. Adding a new device requires only YAML — no Rust code — unless the device family introduces a new protocol that is not covered by any existing base.

See [`DEVICE_DSL.md`](DEVICE_DSL.md) for the complete DSL reference.

## Capability Framework

Every device file declares a list of capabilities — named protocol features supported by that device. The engine uses this list to:

- Determine which settings to expose on D-Bus.
- Drive the sync-read bulk poll at startup (only query structs relevant to declared capabilities).
- Dispatch only the sync events that the device can emit.

Capabilities are defined by the base file. A device file enables or disables them. Examples:

| Capability | Protocol | Notes |
|---|---|---|
| `mic_volume` | cmd `0x37` | mic gain level |
| `sidetone` | cmd `0x39` | four discrete levels |
| `eq_10band` | cmd `0x33` | ±10 dB, 0.5 dB steps |
| `eq_preset` | cmd `0x2E` | 0–18 preset slots |
| `noise_cancelling` | `wireless_settings` field | read; write path TBD |
| `transparent_level` | `wireless_settings` field | 1–10 |
| `chatmix_infinite` | events `0x45` | knob-driven, software mix |
| `battery_headset` | event `0xB7` | 9-step → % |
| `battery_charger` | event `0xB7` | 9-step → % |
| `battery_percentage` | event `0xB7` | 1–100 direct (Gen2) |
| `bluetooth_startup` | cmd `0xB2` | power-on default |
| `bt_call_behavior` | cmd `0xB3` | nothing / −12 dB / mute |
| `oled_brightness` | cmd `0x85` | 1–10 |
| `oled_dim_timer` | cmd `0x83` | inactivity dim |
| `oled_home_screen_type` | cmd `0x89` | detailed / simple |
| `oled_draw` | cmd `0x93` / `0x95` | arbitrary 128×64 bitmap |
| `power_inactivity_timer` | cmd `0xC1` | auto-off |
| `wireless_mode` | cmd `0xC3` | speed / range |
| `line_out_mode` | cmd `0x43` | chatmix / stream-mix |
| `stream_mix` | cmd `0x47` | main / aux / mic levels |
| `software_chatmix` | cmd `0x49` | enable/disable hw chatmix |
| `save_to_flash` | cmd `0x09` | persist settings to NVRAM |

## Firmware and Bootloader PIDs

Every device defines two USB Product IDs:

- **App PID**: used during normal operation.
- **Bootloader PID**: the device re-enumerates with this PID when entering firmware update mode.

The engine registers both PIDs for each device variant. When the bootloader PID appears, the engine enters a restricted mode: it does not run the init sequence or expose settings on D-Bus, and only accepts firmware-update API calls.

Some device families use a third "upgrade" PID for units that have received a major firmware revision that permanently changes their protocol (e.g., Arctis Nova 7 → Nova 7 Gen2). These are treated as distinct device variants with their own YAML file and capability list.

## Voice Changer and PipeWire

The voice changer and PipeWire filter-chain management remain in Python and run as a separate process. They interact with the engine exclusively through the D-Bus interface (subscribing to device state signals such as mic-mute status and headset online/offline events). No changes to this subsystem are required by the v3 engine rewrite.
