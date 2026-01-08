from pathlib import Path


PREFIX='/usr/local'

DBUS_MESSAGE_BUS_NAME = 'name.giacomofurlan.ArctisManager.Next'
DBUS_INTERFACE_PATH = '/name/giacomofurlan/ArctisManager/Next'

PULSE_MEDIA_NODE_NAME = 'Arctis_Media'
PULSE_CHAT_NODE_NAME = 'Arctis_Chat'

STEELSERIES_VENDOR_ID = '0x1038'

SETTINGS_FOLDER = Path.home() / '.config' / 'arctis_manager' / 'settings'

DEVICES_CONFIG_FOLDER = [
    Path.home() / '.config' / 'arctis_manager' / 'devices',
    Path(PREFIX) / 'arctis_manager' / 'devices',

    Path(__file__).parent.parent / 'devices',
]
