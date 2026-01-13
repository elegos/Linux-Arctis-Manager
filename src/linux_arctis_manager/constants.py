from pathlib import Path

PREFIX='/usr/local'

VERSION='dev'

# /DBUS
DBUS_BUS_NAME = 'name.giacomofurlan.ArctisManager.Next'

DBUS_SETTINGS_INTERFACE_NAME = f'{DBUS_BUS_NAME}.Settings'
DBUS_SETTINGS_OBJECT_PATH = '/'

DBUS_STATUS_INTERFACE_NAME = f'{DBUS_BUS_NAME}.Status'
DBUS_STATUS_OBJECT_PATH = '/'

DBUS_CONFIG_INTERFACE_NAME = f'{DBUS_BUS_NAME}.Config'
DBUS_CONFIG_OBJECT_PATH = '/'
# ./DBUS

PULSE_MEDIA_NODE_NAME = 'Arctis_Media'
PULSE_CHAT_NODE_NAME = 'Arctis_Chat'

STEELSERIES_VENDOR_ID = '0x1038'

SETTINGS_FOLDER = Path.home() / '.config' / 'arctis_manager' / 'settings'

DEVICES_CONFIG_FOLDER = [
    Path.home() / '.config' / 'arctis_manager' / 'devices',
    Path(PREFIX) / 'arctis_manager' / 'devices',

    Path(__file__).parent.parent / 'devices',
]
