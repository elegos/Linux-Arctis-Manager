# Device Configuration DSL Reference

Device support in Linux Arctis Manager v3 is driven by YAML files whose structure closely mirrors the official SteelSeries `.device` specification files. Understanding the correspondence between the two formats makes it straightforward to translate any spec file into a working YAML configuration.

## File Hierarchy

```
devices/
  base_arctis_nova_pro_wireless.yaml   ← protocol definition for the family
  arctis_nova_pro_wireless.yaml        ← device file: extends the base, adds PIDs
  base_arctis_nova_7_tx.yaml
  arctis_nova_7.yaml
  arctis_nova_7_gen2.yaml              ← different variant, different capabilities
  ...
```

**Base files** define everything about a protocol family: structs, APIs, transforms, event dispatch, sync-read mapping, and lifecycle hooks. They contain no USB identifiers.

**Device files** extend a base file with `extends:` and add the `device:` section (USB identifiers, firmware versions, enabled capabilities). A device file may override any value from the base using the same key path.

This mirrors the `.device` file pattern of `(include "base_...")`.

---

## Section: `extends`

```yaml
extends: base_arctis_nova_pro_wireless
```

Merges the named base file into this file before applying any local keys. Overrides are applied key-by-key; lists are replaced, not appended.

---

## Section: `constants`

Named scalar values referenced elsewhere in the file.

```yaml
constants:
  report_id: 0x06
  time_between_commands_ms: 50
  init_sleep_ms: 5000
```

**Spec equivalent**: `(define name value)`

Constants can be referenced in struct field `constant:` values using the name prefixed with `$`:

```yaml
- {name: report_id, type: uint8, constant: $report_id}
```

---

## Section: `structs`

Typed message definitions used by `apis:`, `sync_events:`, and `sync_read:`.

**Spec equivalent**: `(struct name (field ...))`

### Basic struct (write-only, single layout)

```yaml
structs:
  save_to_flash:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x09}
```

### Request/response struct (read)

```yaml
structs:
  battery_status:
    outgoing:
      - {name: report_id,             type: uint8, constant: 0x06}
      - {name: command,               type: uint8, constant: 0xB7}
    incoming:
      - {name: report_id,             type: uint8, constant: 0x06}
      - {name: command,               type: uint8, constant: 0xB7}
      - {name: headset_battery_level, type: uint8, range: [0, 8]}
      - {name: charger_battery_level, type: uint8, range: [0, 8]}
      - {name: charging_status,       type: uint8, values: [1, 2, 4, 8]}
```

When `outgoing:` and `incoming:` are both present the struct is bidirectional: the engine serialises `outgoing` to send the request and deserialises `incoming` from the response.

### Field types

| Type | Size | Notes |
|---|---|---|
| `uint8` | 1 byte | unsigned 8-bit integer |
| `uint16` | 2 bytes | big-endian |
| `uint32` | 4 bytes | big-endian |
| `float32` | 4 bytes | IEEE 754, used for EQ gain values |
| `bytearray` | variable | raw bytes; requires `size:` |

### Field constraints

| Key | Meaning |
|---|---|
| `constant: value` | Field is always this value; validated on read, filled on write |
| `range: [min, max]` | Inclusive range; values outside are rejected |
| `values: [v1, v2, ...]` | Enumerated valid values (often bitmask flags) |
| `repeat: n` | Field is an array of `n` elements of the given type |
| `size: n` | Used with `bytearray`; number of bytes |

### Nested struct reference

```yaml
structs:
  custom_eq_setting:
    - {name: gain1,  type: float32, range: [-10.0, 10.0]}
    - {name: gain2,  type: float32, range: [-10.0, 10.0]}
    # ... gains 3–10

  custom_eq:
    - {name: report_id, type: uint8, constant: 0x06}
    - {name: command,   type: uint8, constant: 0x33}
    - {struct: custom_eq_setting}   # expands inline (mirrors fields-from-struct)
```

**Spec equivalent**: `(fields-from-struct name)`

---

## Section: `apis`

Binds a struct to a HID transport operation.

**Spec equivalent**: `(api name (api-read/write transport (chunk TYPE size payload)))`

```yaml
apis:
  save_to_flash:
    write:
      transport: HID_IO
      chunk_size: 64

  battery_status:
    read:
      transport: HID_IO
      chunk_size: 64

  custom_eq:
    write:
      transport: HID_IO
      chunk_size: 64
      payload_transform: builtin:transform_gains_to_firmware_values

  draw_bitmap:
    write:
      transport: HID_FEATURE
      chunk_size: 1024
      payload_transform: builtin:transform_bitmap_sub_payload
```

### Transport types

| Transport | Spec name | Typical use |
|---|---|---|
| `HID_IO` | `HIDIO` | Standard 64-byte interrupt reports |
| `HID_FEATURE` | `HIDFEATURE` | Large payloads (OLED bitmap, up to 1024 bytes) |

### `payload_transform`

When the struct's wire encoding requires non-trivial computation that cannot be expressed as a simple field-level transform, `payload_transform` names a builtin function that receives the fully-serialised struct bytes and returns the bytes to transmit.

See [Builtin transforms](#builtin-transforms) for the complete list.

---

## Section: `transforms`

Named value conversions referenced from `sync_events:`, `sync_read:`, and `apis:`.

**Spec equivalent**: `(define (translate_foo x) (case x ...))`

### `case_int_to_int`

Integer-to-integer lookup table with an optional default for unrecognised values.

```yaml
transforms:
  translate_headset_battery_level:
    type: case_int_to_int
    default: 0
    values: {0: 0, 1: 12, 2: 25, 3: 37, 4: 50, 5: 62, 6: 75, 7: 87, 8: 100}

  transform_dim_timer_to_minutes:
    type: case_int_to_int
    default: 10
    values: {0: 0, 1: 1, 2: 5, 3: 10, 4: 15, 5: 30, 6: 60}

  transform_minutes_to_dim_timer:
    type: case_int_to_int
    default: 3
    values: {0: 0, 1: 1, 5: 2, 10: 3, 15: 4, 30: 5, 60: 6}
```

### `case_int_to_str`

Integer-to-string lookup. Used for status fields exposed as labelled states.

```yaml
transforms:
  translate_charging_status:
    type: case_int_to_str
    default: UNKNOWN_OR_HEADSET_NOT_CONNECTED
    values:
      1: UNKNOWN_OR_HEADSET_NOT_CONNECTED
      2: PLUGGED_IN_CHARGING
      4: PLUGGED_IN_NOT_CHARGING
      8: DISCHARGING

  translate_radio_connection_status:
    type: case_int_to_str
    default: NOT_PAIRED_NOT_SEARCHING
    values:
      1: NOT_PAIRED_NOT_SEARCHING
      2: NOT_PAIRED_SEARCHING
      4: PAIRED_NOT_CONNECTED
      8: PAIRED_CONNECTED
```

### `linear`

Applies `result = (raw_value × scale) + offset`. Used for EQ gain values where the firmware stores an 8-bit integer but the logical value is a float in dB.

```yaml
transforms:
  transform_gain_to_engine_value:
    # Firmware byte b → dB: b/2 − 10
    # Range: b=0 → −10.0 dB, b=20 → 0.0 dB, b=40 → +10.0 dB
    type: linear
    scale: 0.5
    offset: -10.0
```

### Builtin transforms

Some transforms involve multi-field or bitwise operations that cannot be expressed declaratively. They are implemented in Rust and referenced by name.

| Name | Used by | Spec function |
|---|---|---|
| `builtin:transform_gains_to_firmware_values` | `custom_eq` API write | `integer(2 × (10 + gain_dB))` applied to all 10 bands; input is array of float32, output is 10 uint8 bytes |
| `builtin:transform_bitmap_sub_payload` | `draw_bitmap` API write | Splits a bitmap into one or two HID FEATURE payloads depending on whether the data exceeds 512 bytes (`sub-payload` in spec) |
| `builtin:transform_image_to_column_packed` | used internally by `draw_bitmap` | Converts a standard row-major 1-bit bitmap to column-packed LSB-y-flipped format required by the OLED controller |

---

## Section: `sync_events`

Dispatch table for unsolicited reports pushed by the device on the sync interface. Each entry maps a command byte (byte 1 of the incoming report) to one or more named events emitted on D-Bus.

**Spec equivalent**: the `(case ...)` dispatch inside `translate_sync_data`

```yaml
sync_events:

  # (0x27) → high_gain changed
  0x27:
    emit: high_gain
    fields:
      - {name: enabled, byte: 2, transform: transform_device_gain_to_engine_value}

  # (0x2E) → EQ preset changed
  0x2E:
    emit: selected_eq_preset
    fields:
      - {name: id, byte: 2}

  # (0x45) → ChatMix knob moved
  0x45:
    emit: chatmix
    fields:
      - {name: game_attenuation, byte: 2}
      - {name: chat_attenuation, byte: 3}

  # (0xB5) → 2.4 GHz radio connection changed
  0xB5:
    emit: radio_connection
    fields:
      - {name: radio_connection_status, byte: 4, transform: translate_radio_connection_status}
    side_effects:
      - call: send_connection_status
        arg_byte: 4

  # (0xB7) → battery or charging state changed
  0xB7:
    side_effects:
      - call: handle_headset_battery_event, arg_byte: 2
      - call: handle_charger_battery_event, arg_byte: 3
      - call: handle_charging_event,        arg_byte: 4

  # (0xC1) → power inactivity timer changed (on-device)
  0xC1:
    emit: power_inactivity_timer
    fields:
      - {name: minutes, byte: 2, transform: transform_power_inactivity_timer_to_minutes}
```

### Entry keys

| Key | Required | Meaning |
|---|---|---|
| `emit` | no | D-Bus event name to fire; omit if the event is handled entirely via `side_effects` |
| `fields` | no | List of fields to extract from the report and include in the emitted event |
| `side_effects` | no | Named engine-internal calls (battery handlers, connection status forwarding, etc.) |

### Field extractor keys

| Key | Meaning |
|---|---|
| `name` | Field name in the emitted event payload |
| `byte` | 0-indexed byte offset within the full HID report |
| `transform` | Optional transform name applied to the extracted byte before emitting |

---

## Section: `sync_read`

Describes how to populate the full device state at startup by reading bulk-settings structs (audio, UX, wireless). Each source struct is read once and its fields are mapped to named engine events, using the same transforms defined in `transforms:`.

**Spec equivalent**: `sync_settings_function`

```yaml
sync_read:

  - struct: audio_settings
    maps:
      - {emit: high_gain,          field: device_gain,  transform: transform_device_gain_to_engine_value}
      - {emit: selected_eq_preset, field: eq_preset}
      - {emit: custom_eq,          fields: [gain1, gain2, gain3, gain4, gain5,
                                            gain6, gain7, gain8, gain9, gain10],
                                   transform: transform_gain_to_engine_value}
      - {emit: mic_volume,         field: mic_volume}
      - {emit: sidetone,           field: sidetone}
      - {emit: line_out_mode,      field: line_out_mode}
      - {emit: stream_mix,         fields: [stream_main, stream_aux, stream_mic]}
      - {emit: chatmix,            fields: [chatmix_game, chatmix_chat]}

  - struct: ux_settings
    maps:
      - {emit: dim_timer,        field: dim_timer,      transform: transform_dim_timer_to_minutes}
      - {emit: oled_brightness,  field: oled_brightness}
      - {emit: home_screen_type, field: home_screen_type}

  - struct: wireless_settings
    maps:
      - {emit: bluetooth_startup,      field: bt_power_default}
      - {emit: bt_call_default,        field: bt_call_default}
      - {emit: muted_mic_brightness,   field: muted_mic_brightness}
      - {emit: power_inactivity_timer, field: power_inactivity_timer,
                                       transform: transform_power_inactivity_timer_to_minutes}
      - {emit: wireless_mode,          field: wireless_mode}
      - {emit: radio_connection,       field: radio_connection_status,
                                       transform: translate_radio_connection_status}
```

---

## Section: `lifecycle`

Named hook sequences executed at well-defined points in the device's connection lifecycle.

**Spec equivalent**: `(custom-init)`, `(custom-post-init)`, `(shutdown ...)`

```yaml
lifecycle:
  init:
    - call: enable_sonar
    - call: sync_all
    - call: discord_certified_set_attributes
      args: {echo_cancellation: true, noise_suppression: true}

  post_init:
    - call: send_init_wireless_connection_battery_status

  shutdown:
    - call: disable_chatmix
    - call: disable_sonar
    - call: save_to_flash
```

### Lifecycle points

| Hook | Triggered |
|---|---|
| `init` | After the HID fd is opened and the device init sequence has completed |
| `post_init` | After `init` and after the engine's internal state is fully populated |
| `shutdown` | When the device is disconnected or the engine is stopping |

### Built-in calls

| Call | Behaviour |
|---|---|
| `enable_sonar` | Writes `set_sonar_present{is_present: 1}` |
| `disable_sonar` | Writes `set_sonar_present{is_present: 0}` |
| `enable_chatmix` | Writes `software_chatmix_status{status: 1}` |
| `disable_chatmix` | Writes `software_chatmix_status{status: 0}` |
| `sync_all` | Reads all structs listed in `sync_read:` and populates engine state |
| `save_to_flash` | Writes `save_to_flash` struct (persists settings to device NVRAM) |
| `send_init_wireless_connection_battery_status` | Reads `wireless_settings`, emits connection and battery events |
| `discord_certified_set_attributes` | Registers Discord-certified microphone attributes |

---

## Section: `device`

Present only in device files (not base files). Specifies USB identification, HID interface routing, firmware versions, and the list of capabilities this device exposes.

```yaml
device:
  name: "SteelSeries Arctis Nova Pro Wireless"
  vendor_id: 0x1038

  # One or more product ID variants. Bootloader PIDs are listed separately
  # so the engine can detect and handle firmware-update mode correctly.
  variants:
    - name: standard
      product_id: 0x12E0
      bootloader_pid: 0x12E1
    - name: xbox
      product_id: 0x12E5
      bootloader_pid: 0x12E7
    - name: xbox_white
      product_id: 0x225D
      # No separate bootloader PID for this variant

  hid:
    usage_page: 0xFFC0
    usage: 0x0001
    # Interface used to send commands (bInterfaceNumber, bAlternateSetting)
    command_interface: {interface: 4, alternate: 0}
    # Interface on which the device pushes unsolicited events
    sync_interface: {interface: 4, usage_page: 0xFF00, usage: 0x0001}

  firmware:
    tx_dsp: "0.3.82"
    tx_mcu: "1.29.27"
    rx_mcu: "1.22.11"
    rx_bt:  "1.15.4"
    rx_v2_mcu: "2.2.0"
    rx_v2_bt:  "2.1.0"
    required_engine:
      tx_dsp: ">=0.3.82"
      tx_mcu: ">=1.27.0"

  capabilities:
    - mic_volume
    - sidetone
    - mic_led_brightness
    - high_gain
    - eq_10band
    - eq_preset
    - line_out_mode
    - stream_mix
    - software_chatmix
    - chatmix_infinite
    - noise_cancelling
    - transparent_level
    - battery_headset
    - battery_charger
    - bluetooth_startup
    - bt_call_behavior
    - oled_brightness
    - oled_dim_timer
    - oled_home_screen_type
    - oled_draw
    - power_inactivity_timer
    - wireless_mode
    - save_to_flash
```

### Bootloader and upgrade variants

When a device enters firmware update mode it re-enumerates on the USB bus with its `bootloader_pid`. The engine detects this PID, enters a restricted state (no settings, no D-Bus settings interface), and accepts only firmware-update API calls.

Some device families define a permanent "upgrade" variant: units of an older revision that received a major firmware update acquire a new PID and a different capability set. These are treated as distinct device files:

```yaml
# arctis_nova_7.yaml  — original hardware
device:
  variants:
    - {product_id: 0x2202, bootloader_pid: 0x2203}
  capabilities:
    - battery_discrete_5step   # 0/25/50/75/100 %
    ...

# arctis_nova_7_gen2.yaml  — after major firmware upgrade
device:
  variants:
    - {product_id: 0x22A1}
  capabilities:
    - battery_percentage       # 1–100 % continuous
    - bt_call_behavior         # Gen2 adds BT call settings
    ...
```

---

## Complete Example — Nova Pro Wireless

The following is an abbreviated but structurally complete device file pair.

### `base_arctis_nova_pro_wireless.yaml` (excerpt)

```yaml
constants:
  report_id: 0x06
  time_between_commands_ms: 50
  init_sleep_ms: 5000

structs:
  wireless_settings:
    outgoing:
      - {name: report_id,               type: uint8, constant: 0x06}
      - {name: command,                 type: uint8, constant: 0xB0}
    incoming:
      - {name: report_id,               type: uint8, constant: 0x06}
      - {name: command,                 type: uint8, constant: 0xB0}
      - {name: bt_power_default,        type: uint8, range: [0, 1]}
      - {name: bt_call_default,         type: uint8, range: [0, 2]}
      - {name: bt_connection_mode,      type: uint8, values: [1, 2, 4]}
      - {name: bt_connection_status,    type: uint8, values: [1, 2, 4, 8]}
      - {name: headset_batt_level,      type: uint8, range: [0, 8]}
      - {name: charger_batt_level,      type: uint8, range: [0, 8]}
      - {name: transparent_level,       type: uint8, range: [1, 10]}
      - {name: mic_muted,               type: uint8, range: [0, 1]}
      - {name: transparency_mode,       type: uint8, range: [0, 2]}
      - {name: muted_mic_brightness,    type: uint8, range: [1, 10]}
      - {name: power_inactivity_timer,  type: uint8, range: [0, 6]}
      - {name: wireless_mode,           type: uint8, range: [0, 1]}
      - {name: radio_connection_status, type: uint8, values: [1, 2, 4, 8]}
      - {name: headset_batt_status,     type: uint8, values: [1, 2, 4, 8]}

apis:
  wireless_settings:
    read: {transport: HID_IO, chunk_size: 64}

transforms:
  translate_radio_connection_status:
    type: case_int_to_str
    default: NOT_PAIRED_NOT_SEARCHING
    values:
      1: NOT_PAIRED_NOT_SEARCHING
      2: NOT_PAIRED_SEARCHING
      4: PAIRED_NOT_CONNECTED
      8: PAIRED_CONNECTED

  translate_headset_battery_level:
    type: case_int_to_int
    default: 0
    values: {0: 0, 1: 12, 2: 25, 3: 37, 4: 50, 5: 62, 6: 75, 7: 87, 8: 100}

  transform_power_inactivity_timer_to_minutes:
    type: case_int_to_int
    default: 30
    values: {0: 0, 1: 1, 2: 5, 3: 10, 4: 15, 5: 30, 6: 60}

sync_events:
  0xB5:
    emit: radio_connection
    fields:
      - {name: radio_connection_status, byte: 4, transform: translate_radio_connection_status}
    side_effects:
      - call: send_connection_status, arg_byte: 4

  0xB7:
    side_effects:
      - call: handle_headset_battery_event, arg_byte: 2
      - call: handle_charger_battery_event, arg_byte: 3
      - call: handle_charging_event,        arg_byte: 4

  0xC1:
    emit: power_inactivity_timer
    fields:
      - {name: minutes, byte: 2, transform: transform_power_inactivity_timer_to_minutes}

  0xC3:
    emit: wireless_mode
    fields:
      - {name: mode, byte: 2}

sync_read:
  - struct: wireless_settings
    maps:
      - {emit: bluetooth_startup,      field: bt_power_default}
      - {emit: bt_call_default,        field: bt_call_default}
      - {emit: muted_mic_brightness,   field: muted_mic_brightness}
      - {emit: power_inactivity_timer, field: power_inactivity_timer,
                                       transform: transform_power_inactivity_timer_to_minutes}
      - {emit: wireless_mode,          field: wireless_mode}
      - {emit: radio_connection,       field: radio_connection_status,
                                       transform: translate_radio_connection_status}
      - {emit: headset_battery,        field: headset_batt_level,
                                       transform: translate_headset_battery_level}

lifecycle:
  init:
    - call: enable_sonar
    - call: sync_all
    - call: discord_certified_set_attributes
      args: {echo_cancellation: true, noise_suppression: true}
  post_init:
    - call: send_init_wireless_connection_battery_status
  shutdown:
    - call: disable_chatmix
    - call: disable_sonar
    - call: save_to_flash
```

### `arctis_nova_pro_wireless.yaml`

```yaml
extends: base_arctis_nova_pro_wireless

device:
  name: "SteelSeries Arctis Nova Pro Wireless"
  vendor_id: 0x1038

  variants:
    - name: standard
      product_id: 0x12E0
      bootloader_pid: 0x12E1
    - name: xbox
      product_id: 0x12E5
      bootloader_pid: 0x12E7
    - name: xbox_white
      product_id: 0x225D

  hid:
    usage_page: 0xFFC0
    usage: 0x0001
    command_interface: {interface: 4, alternate: 0}
    sync_interface: {interface: 4, usage_page: 0xFF00, usage: 0x0001}

  firmware:
    tx_dsp: "0.3.82"
    tx_mcu: "1.29.27"
    rx_mcu: "1.22.11"
    rx_bt:  "1.15.4"
    rx_v2_mcu: "2.2.0"
    rx_v2_bt:  "2.1.0"
    required_engine:
      tx_dsp: ">=0.3.82"
      tx_mcu: ">=1.27.0"

  capabilities:
    - mic_volume
    - sidetone
    - mic_led_brightness
    - high_gain
    - eq_10band
    - eq_preset
    - line_out_mode
    - stream_mix
    - software_chatmix
    - chatmix_infinite
    - noise_cancelling
    - transparent_level
    - battery_headset
    - battery_charger
    - bluetooth_startup
    - bt_call_behavior
    - oled_brightness
    - oled_dim_timer
    - oled_home_screen_type
    - oled_draw
    - power_inactivity_timer
    - wireless_mode
    - save_to_flash
```

---

## Correspondence Table: Spec DSL → YAML

| `.device` construct | YAML section | Notes |
|---|---|---|
| `(define name value)` | `constants:` | |
| `(struct name (field ...))` | `structs:` | |
| `(struct name (outgoing ...) (incoming ...))` | `structs:` with `outgoing:`/`incoming:` | |
| `(fields-from-struct other)` | `{struct: other}` inside fields list | |
| `(api name (api-read transport (chunk T n payload)))` | `apis:` read | |
| `(api name (api-write transport (chunk T n payload)))` | `apis:` write | |
| `(define (translate_x y) (case y (...)))` | `transforms:` case map | |
| `(define (fn y) (- (/ y 2) 10))` | `transforms:` linear | |
| Complex bit/float manipulation | `transforms: builtin:name` | |
| `(translate_sync_data (case 0xNN ...))` | `sync_events:` | |
| `sync_settings_function` | `sync_read:` | |
| `(custom-init)` | `lifecycle: init:` | |
| `(custom-post-init)` | `lifecycle: post_init:` | |
| `(shutdown ...)` | `lifecycle: shutdown:` | |
| `(include "base_...")` | `extends:` | |
| `(define (app_pid) 0xNNNN)` | `device: variants: product_id:` | |
| `(define (bootloader_pid) 0xNNNN)` | `device: variants: bootloader_pid:` | |
| `(usage-page 0xNNNN)` | `device: hid: usage_page:` | |
| `(sync-interface 0xFF00 0x0001 4)` | `device: hid: sync_interface:` | |
