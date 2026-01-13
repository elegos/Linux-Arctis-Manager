# Linux Arctis Manager

A replacement for SteelSeries GG software, to manage your Arctis device on Linux!

## 👍 Key points

- Enable the Media and Chat audio streams
- Configure any device via a simple configuration file
- Enable per-device features by adding them in the relative configuration file
- D-Bus based communication, to support different clients (alternative clients, Plasma extensions, etc)

## 🎧 Currently supported devices

- SteelSeries Arctis Nova Pro Wireless

## ⌨️ Development

### Basic commands

- Run the daemon: `uv run lam-daemon`
- Run the CLI: `uv run lam-cli`*
- Run the GUI: `uv run lam-gui`*

\*: to be developed

### How to add support to a new device

- Add a new udev rule following the same pattern of the ones already existing (`/usr/lib/udev/rules.d/91-steelseries-arctis.rules`)
- Add a new device configuration file in `~/.config/arctis_manager/devices/`, according to the results of the reverse engineering (the temporary configuration file allows for instant device support, waiting for a software's new version to come out)

In case of software limitations for any reason, some coding might be required (for example to support a new status or setting type).

Once the configuration is completed locally, a new ticket can be raised to add both the new udev rules and configuration file. A pull request into the development branch is very welcome, specially if adding new code, too.

### Device configuration specs

Device configuration files are in the YAML file format, as follows:

```yaml
device:
  name: Friendly name # for example: SteelSeries Arctis Nova Pro Wireless
  vendor_id: 0x1038 # Should always be 0x1038, but double check via lsusb
  product_ids: [0x1234] # At least one identifier. Multiple identifiers might apply for different SKUs
  command_padding: # This defines the command's message length and the filler. Typically 64 bytes zero-padded.
    length: 64
    position: end
    filler: 0x00

  command_interface_index: [4, 0] # USB interface number and alternate setting to write commands (status request, device initialization). The former can be analyzed via usb-devices, or by chance. The latter should typically be 0.
  listen_interface_indexes: [4] # USB interface number to listen for the status.

  device_init: # OPTIONAL, the list of commands to initialize the device, for example the GameDAC
    - [0x06, 0x20]
    # ...
    - [0x06, 0xc3, 'settings.wireless_mode']      # Can accept also values from the settings (settings.NAME_OF_THE_SETTING)
    - ['status.request']                          # Can accept the special 'status.request' to send device.status.request's value
  status:
    request: 0x06b0 # Message to be sent to request the device's status
    response_mapping: # A list of objects having
                      # (1) starts_with (the message sent from the device must start with the given sequence)
                      # (2, n) the status' name and its position in the message
      - starts_with: 0x0725 # The whole device's message will be 0x0725NN (where NN is the station_volume)
        station_volume: 0x02

      - starts_with: 0x0745 # Message: 0x0745XXYY
        media_mix: 0x02     # ⚠️ media_mix is a special name for the channels mixer, it is required to use this one and not similar ones!
        chat_mix: 0x03      # ⚠️ chat_mix is a special name for the channels mixer, it is required to use this one and not similar ones!

      - starts_with: 0x06b0 # Message: 0x06b0XXYYZZKK...
        bluetooth_powerup_state: 0x02
        bluetooth_auto_mute: 0x03
        bluetooth_power_status: 0x04
        bluetooth_connection: 0x05
        # ...

  settings: # OPTIONAL section
    microphone:                # Settings section
      mic_volume:              # Setting's name
        type: slider           # Setting's type
        default: 0x0a          # Setting's default value
        min: 0x01              # Type-specific parameter
        max: 0x0a              # Type-specific parameter
        step: 1                # Type-specific parameter
        min_label: mic_muted   # Type-specific parameter
        max_label: perc_100    # Type-specific parameter
    another_section:
      another_setting:
        type: ...

  status_parse:               # Each status found in device.status.response_mapping[n] requires a counterpart in this section
    station_volume:
      type: percentage
      perc_min: -56
      perc_max: 0
    media_mix:
      type: percentage
      perc_min: 0
      perc_max: 100
    # ...

```

⚠️ As highlighted in the example above, `media_mix` and `chat_mix` status entries are key to the mixer management. Please avoid using different names, like `game_mix` in order to enable proper channels mixing.

⚠️ The `status_parse` section of `media_mix` and `chat_mix` requires the values to be normalized between 0 and 100.

### YAML's device.settings.[section].[setting] types

Linux Arctis Manager supports out of the box the following settings. Additional types need to be implemented in the code.

```yaml
# Linear multi-value setting (for example 1..n)
type: slider
min: int                      # the minimum valid value
max: int                      # the maximum valid valud
step: 1                       # the number each step sums to the next one (ex. min 1, max 5, step 2; values: 1, 3, 5)
min_label: slider_setting_min # As found in the language ini file
max_label: slider_setting_max # As found in the language ini file
default: 0x0a                 # The value set if none was before
```

```yaml
# Boolean values (with custom on/off values and labels)
type: toggle
values:
  off: 0x00       # Value in the "off" position
  on: 0x01        # Value in the "on" position
  off_label: high # As found in the language ini file
  on_label: low   # As found in the language ini file
default: 0x01     # The value set if none was before
```

```yaml
# Drop-downs
type: select
default: null
options_source: pulse_audio_devices                        # pulse_audio_devices only for now
options_mapping: { value: id, label: description } # Depends on options_source. "pulse_audio_devices": id and description only for now
```

### YAML's device.status_parse.[status_name] types

Linux Arctis Manager supports out of the box the following status types. Additional types need to be implemented in the code.

To add a new parser, define the function in the [status_parser_fn.py](./src/linux_arctis_manager/status_parser_fn.py) file and add the tests in the [relative file](./tests/test_status_parser_fn.py). New functions will be added to the application automatically.

```yaml
# Percentage (0-100%)
type: percentage
perc_min: -56    # The absolute value of 0%
perc_max: 0      # The absolute value of 100%
```

```yaml
# Togglable (on/off)
type: on_off
off: 0x00       # The value to represent the off status
on: 0x01        # The value to represent the on status
```

```yaml
# Value (int) to string (label)
type: int_str_mapping
values:
  0x00: off    # Value: label (label as found in the language ini file)
  0x01: -12db
  0x02: on
  ...
```

```yaml
# Value (int) to mapped int
type: int_int_mapping
values:
  0x00: 0 
  0x01: 1
  0x02: 5
  ...
```

### D-BUS messaging

The daemon interacts with the clients (CLI, UI, etc) via D-Bus.

- Bus name: "name.giacomofurlan.ArctisManager.Next"

#### name.giacomofurlan.ArctisManager.Next.Settings/GetSettings
- **Parameters**: (none)
- **Response format**: JSON
- **Specs**:

The response has three sections:
- `general`: for general (cross-device) settings.
- `device`: for device-specific settings. Will be an empty object if no device is connected.
- `settings_config`: the definition for each setting, defining type, default_value and other arguments depending on the type. See **YAML's device.settings.[section].[setting] types**.

The clients shouldn't hard-core the settings, but read them and parse them depending on the `settings_config` section.

```json
{
    "general": {
        "toggle_setting": 0,
        "select_setting": null,
        "slider_setting": 10
    },
    "device": {
        "setting_a": 10,
        "setting_b": 0,
        "setting_c": 10
    },
    "settings_config": {
        "toggle_setting": {
            "type": "toggle",
            "default_value": false,
            "values": {
                "on": true,
                "off": false,
                "off_label": "off",
                "on_label": "on"
            }
        },
        "select_setting": {
            "type": "select",
            "default_value": null,
            "options_source": "pulse_audio_devices",
            "options_mapping": {
                "value": "id",
                "label": "description"
            }
        },
        "slider_setting": {
            "type": "slider",
            "default_value": 10,
            "min": 1,
            "max": 10,
            "step": 1,
            "min_label": "mic_muted",
            "max_label": "perc_100"
        },
        "setting_a": ...,
        "setting_b": ...,
        "setting_c": ...
    }
}
```

#### name.giacomofurlan.ArctisManager.Next.Settings/SetSettings
- **Parameters**: setting: string, value: string (JSON format)
- **Response format**: JSON
- **Specs**:

Writes the setting. Searches the setting first in the general settings and then, if not found, in the device's.

Returns boolean (true: setting saved, false: setting not found / not saved)

#### name.giacomofurlan.ArctisManager.Next.Status/GetStatus

- **Parameters**: (none)
- **Response format**: JSON
- **Specs**:

Returns an object with the series of configured status values, in the mapped values as defined in **YAML's device.status_parse.[status_name] types**.

If no device is connected, an empty string will return.

```json
{"bluetooth_powerup_state": "off", "bluetooth_auto_mute": "off", "bluetooth_power_status": "off", "bluetooth_connection": "off", "headset_battery_charge": 25, "charge_slot_battery_charge": 100, "transparent_noise_cancelling_level": 100, "mic_status": "unmuted", "noise_cancelling": "off", "mic_led_brightness": 100, "auto_off_time_minutes": 30, "wireless_mode": "speed", "wireless_pairing": "connected", "headset_power_status": "online"}
```

#### Method: name.giacomofurlan.ArctisManager.Next.Config/ReloadConfigs

- **Parameters**: (none)
- **Response format**: JSON
- **Specs**:

Reload the configuration files, useful to test new devices support during the development phase. Return boolean (success / failure).
