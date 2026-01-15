from pathlib import Path

PREFIX='/usr/local'

# /DBUS
DBUS_BUS_NAME = 'name.giacomofurlan.ArctisManager.Next'
DBUS_OBJECT_BASE_PATH = '/name/giacomofurlan/ArctisManager/Next'

DBUS_SETTINGS_INTERFACE_NAME = f'{DBUS_BUS_NAME}.Settings'
DBUS_SETTINGS_OBJECT_PATH = f'{DBUS_OBJECT_BASE_PATH}/Settings'

DBUS_STATUS_INTERFACE_NAME = f'{DBUS_BUS_NAME}.Status'
DBUS_STATUS_OBJECT_PATH = f'{DBUS_OBJECT_BASE_PATH}/Status'

DBUS_CONFIG_INTERFACE_NAME = f'{DBUS_BUS_NAME}.Config'
DBUS_CONFIG_OBJECT_PATH = f'{DBUS_OBJECT_BASE_PATH}/Config'
# ./DBUS

PULSE_MEDIA_NODE_NAME = 'Arctis_Media'
PULSE_CHAT_NODE_NAME = 'Arctis_Chat'

STEELSERIES_VENDOR_ID = '0x1038'

SETTINGS_FOLDER = Path.home() / '.config' / 'arctis_manager' / 'settings'

HOME_CONFIG_FOLDER = Path.home() / '.config' / 'arctis_manager' / 'devices'
PREFIXED_CONFIG_FOLDER = Path(PREFIX) / 'arctis_manager' / 'devices'
SRC_CONFIG_FOLDER = Path(__file__).parent.parent / 'devices'

DEVICES_CONFIG_FOLDER: list[Path] = [HOME_CONFIG_FOLDER, PREFIXED_CONFIG_FOLDER, SRC_CONFIG_FOLDER]

UDEV_RULES_PATH = '/usr/lib/udev/rules.d/91-steelseries-arctis.rules'
