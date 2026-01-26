# D-Bus messaging

The daemon interacts with the clients (CLI, UI, etc) via D-Bus.

- **Bus name**: "name.giacomofurlan.ArctisManager.Next"

## name.giacomofurlan.ArctisManager.Next.Settings

- **Interface Path**: /name/giacomofurlan/ArctisManager/Next/Settings

### Method: GetListOptions
- **Parameters**: list_name (string)
- **Response format**: JSON
- **Specs**:

The response varies depending on the list, but it will always return a list of objects.

**List name: pulse_audio_devices**:

```json
[{ "id": "string", "name": "string" }]
```

### Method: GetSettings
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

### Method: SetSettings
- **Parameters**: setting: string, value: string (JSON format)
- **Response format**: JSON
- **Specs**:

Writes the setting. Searches the setting first in the general settings and then, if not found, in the device's.

Returns boolean (true: setting saved, false: setting not found / not saved)

## name.giacomofurlan.ArctisManager.Next.Status

- **Interface Path**: /name/giacomofurlan/ArctisManager/Next/Status

### Method: GetStatus

- **Parameters**: (none)
- **Response format**: JSON
- **Specs**:

Returns an object with the series of configured status values, in the mapped values as defined in **YAML's device.status_parse.[status_name] types** and categorized in **YAML's device.status.representation**.

If no device is connected, an empty object will return.

Each setting has two attributes:

- `value`: the (parsed, but not translated) value
- `type`: "label" in case of string, or the relative type, as defined in `device.status_parse.[status].type`

```json
{
    "headset": {
        "headset_power_status": {
            "value": "online",
            "type": "label"
        },
        "headset_battery_charge": {
            "value": 87,
            "type": "percentage"
        },
        "noise_cancelling": {
            "value": "off",
            "type": "label"
        },
        "transparent_noise_cancelling_level": {
            "value": 100,
            "type": "percentage"
        },
        "auto_off_time_minutes": {
            "value": 30,
            "type": "int_int_mapping"
        }
    },
    ...
}
```

## name.giacomofurlan.ArctisManager.Next.Config

- **Interface Path**: /name/giacomofurlan/ArctisManager/Next/Config

### Method: ReloadConfigs

- **Parameters**: (none)
- **Response format**: JSON
- **Specs**:

Reload the configuration files, useful to test new devices support during the development phase. Return boolean (success / failure).
