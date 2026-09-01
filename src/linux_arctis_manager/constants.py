from pathlib import Path

# /DBus
DBUS_BUS_NAME = 'name.giacomofurlan.ArctisManager.Next'
DBUS_OBJECT_BASE_PATH = '/name/giacomofurlan/ArctisManager/Next'

DBUS_SETTINGS_INTERFACE_NAME = f'{DBUS_BUS_NAME}.Settings'
DBUS_SETTINGS_OBJECT_PATH = f'{DBUS_OBJECT_BASE_PATH}/Settings'

DBUS_STATUS_INTERFACE_NAME = f'{DBUS_BUS_NAME}.Status'
DBUS_STATUS_OBJECT_PATH = f'{DBUS_OBJECT_BASE_PATH}/Status'

DBUS_CONFIG_INTERFACE_NAME = f'{DBUS_BUS_NAME}.Config'
DBUS_CONFIG_OBJECT_PATH = f'{DBUS_OBJECT_BASE_PATH}/Config'

DBUS_EQ_INTERFACE_NAME = f'{DBUS_BUS_NAME}.EQ'
DBUS_EQ_OBJECT_PATH = f'{DBUS_OBJECT_BASE_PATH}/EQ'

DBUS_NC_INTERFACE_NAME = f'{DBUS_BUS_NAME}.NC'
DBUS_NC_OBJECT_PATH = f'{DBUS_OBJECT_BASE_PATH}/NC'

DBUS_VC_INTERFACE_NAME = f'{DBUS_BUS_NAME}.VC'
DBUS_VC_OBJECT_PATH = f'{DBUS_OBJECT_BASE_PATH}/VC'
# ./DBus

# Systemd
SYSTEMD_SERVICE_NAME = 'lam-daemon.service'
SYSTEMD_HELPER_SERVICE_NAME = 'lam-hidraw-helper.service'
# ./Systemd

PULSE_MEDIA_NODE_NAME = 'Arctis_Media'
PULSE_CHAT_NODE_NAME = 'Arctis_Chat'

STEELSERIES_VENDOR_ID = 0x1038

SETTINGS_FOLDER = Path.home() / '.config' / 'arctis_manager' / 'settings'

HOME_LANG_FOLDER = Path.home() / '.config' / 'arctis_manager' / 'lang'

HOME_CONFIG_FOLDER = Path.home() / '.config' / 'arctis_manager' / 'devices'
EQ_PRESETS_FOLDER: Path = Path.home() / '.config' / 'arctis_manager' / 'eq_presets'
SRC_CONFIG_FOLDER = Path(__file__).parent / 'devices'

DEVICES_CONFIG_FOLDER: list[Path] = [HOME_CONFIG_FOLDER, SRC_CONFIG_FOLDER]

UDEV_RULES_PATHS = [
    '/etc/udev/rules.d/91-steelseries-arctis.rules',
    '/usr/lib/udev/rules.d/91-steelseries-arctis.rules',
]

# Voice-changer output sink name.  Lives here (not in the VC chain modules)
# so core can reference it at daemon boot without importing modules that
# require the AI environment (numpy etc.), which is activated lazily.
ARCTIS_VC_SINK = 'Arctis_VC_Sink'
