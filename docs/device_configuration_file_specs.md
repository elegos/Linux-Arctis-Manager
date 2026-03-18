# Device configuration file specifications

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

  command_interface_index: [4, 0] # USB interface number and alternate setting to write commands (status request, device initialization).
                                  # The former can be analyzed via usb-devices, or by chance. The latter should typically be 0.
                                  #
                                  # !!! IF SET TO [0, x]: communication will happen on endpoint 0x00 (for devices like Arctis Nova 7), with the given {x} wIndex (which is the target interface).
                                  # You can find the wIndex within the USBHID messages in Wireshark, addressing Endpoint = 0x00, under Setup Data
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
    representation:      # Categorization and ordering
      category1:         # Translation in [status] section of language file
        - station_volume # List of settings that apply to the category
        - station_charge # If you want to omiss one or more settings in the representation, it's ok
  
  online_status: # OPTIONAL. Used to detect whether the device is connected AND online (useful for wireless devices). If not defined, it will be considered always online if connected via USB
    status_variable: status_variable_to_check_against     # The settings variable's name
    online_value: status_variable's_mapped_value_to_check # The (mapped) variable's value

  settings: # OPTIONAL section
    microphone:                                # Settings section
      mic_volume:                              # Setting's name
        type: slider                           # Setting's type
        default: 0x0a                          # Setting's default value
        update_sequence: [0x06, 0x37, 'value'] # 'value' is a special token replaced with the setting's value at runtime
        min: 0x01                              # Type-specific parameter
        max: 0x0a                              # Type-specific parameter
        step: 1                                # Type-specific parameter
        min_label: mic_muted                   # Type-specific parameter
        max_label: perc_100                    # Type-specific parameter
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

## YAML's device.settings.[section].[setting] types

Linux Arctis Manager supports out of the box the following settings. Additional types need to be implemented in the code.

```yaml
# Linear multi-value setting (for example 1..n)
type: slider
min: int                               # the minimum valid value
max: int                               # the maximum valid valud
step: 1                                # the number each step sums to the next one (ex. min 1, max 5, step 2; values: 1, 3, 5)
min_label: slider_setting_min          # As found in the language ini file
max_label: slider_setting_max          # As found in the language ini file
default: 0x0a                          # The value set if none was before
update_sequence: [0x06, 0x37, 'value'] # The setting's command update sequence
values_mapping:                        # OPTIONAL: if the slider needs different representation of the values
  0: off                               # Labels as found in [settings_values]
  1: low
  2: medium
  3: high
```

```yaml
# Discrete map - a discrete mapping represented by a group of checkable buttons
type: discrete_map
default: 0x0a                          # The value set if none was before
update_sequence: [0x06, 0x37, 'value'] # The setting's command update sequence
values_mapping:
  0: off                               # Labels as found in [settings_values]
  1: low
  2: medium
  3: high
```

```yaml
# Boolean values (with custom on/off values and labels)
type: toggle
values:
  off: 0x00                            # Value in the "off" position
  on: 0x01                             # Value in the "on" position
  off_label: high                      # As found in the language ini file
  on_label: low                        # As found in the language ini file
default: 0x01                          # The value set if none was before
update_sequence: [0x06, 0x37, 'value'] # The setting's command update sequence
```

```yaml
# Drop-downs
type: select
default: null
options_source: pulse_audio_devices                # pulse_audio_devices only for now
options_mapping: { value: id, label: description } # Depends on options_source. "pulse_audio_devices": id and description only for now
update_sequence: [0x06, 0x37, 'value']             # The setting's command update sequence
```

## YAML's device.status_parse.[status_name] types

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
