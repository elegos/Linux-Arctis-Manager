# Linux Arctis Manager

A replacement for SteelSeries GG software, to manage your Arctis device on Linux!

## 👍 Key points

- Enable the Media and Chat audio streams
- Configure any device via a simple configuration file
- Enable per-device features by adding them in the relative configuration file

## 🎧 Currently supported devices

- SteelSeries Arctis Nova Pro Wireless

## ⌨️ Development

### Basic commands

- Run the daemon: `uv run lam-daemon`
- Run the CLI: `uv run lam-cli`*
- Run the GUI: `uv run lam-gui`*

\*: to be developed

### Add a new configuration (no code)

Simply add a new device configuration file in `~/.config/arctis_manager/devices/`. Device configuration files are in the YAML file format, as follows:

```yaml
device:
  name: Friendly name # for example: SteelSeries Arctis Pro Wireless
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
        media_mix: 0x02
        chat_mix: 0x03

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
        min: 0x01
        max: 0x0a
        step: 1
        min_label: mic_muted
        max_label: perc_100
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
