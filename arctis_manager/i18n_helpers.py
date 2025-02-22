from arctis_manager.device_manager.device_status import DeviceStatus
from arctis_manager.translations import TranslatableText


def str_or_none(v):
    return str(v) if v is not None else None


def else_none(value, callback: callable):
    return None if value is None else callback()


def get_translated_menu_entries(status: DeviceStatus) -> dict[TranslatableText, list[TranslatableText]]:
    '''
    Get the translated menu entries for the given device status.
    '''

    result = {
        TranslatableText('sections.battery'): [
            else_none(status.headset_power_status, lambda: TranslatableText(
                'menu.headset_power_status').format(status=str_or_none(status.headset_power_status))),
            else_none(status.headset_battery_charge, lambda: TranslatableText(
                'menu.headset_battery_charge').format(status=str_or_none(status.headset_battery_charge * 100))),
            else_none(status.headset_battery_charge, lambda: TranslatableText(
                'menu.charge_slot_battery_charge').format(status=str_or_none(status.charge_slot_battery_charge * 100))),
        ],
        TranslatableText('sections.microphone'): [
            else_none(status.headset_battery_charge, lambda: TranslatableText('menu.mic_status').format(status=str_or_none(status.mic_status))),
            else_none(status.mic_led_brightness, lambda: TranslatableText('menu.mic_led_brightness').format(status=str_or_none(status.mic_led_brightness * 100))),
        ],
        TranslatableText('sections.anc'): [
            else_none(status.noise_cancelling, lambda: TranslatableText('menu.noise_cancelling').format(status=str_or_none(status.noise_cancelling))),
            else_none(status.transparent_noise_cancelling_level, lambda: TranslatableText(
                'menu.transparent_noise_cancelling_level').format(status=str_or_none(status.transparent_noise_cancelling_level * 100))),
        ],
        TranslatableText('sections.wireless_mode'): [
            else_none(status.wireless_pairing, lambda: TranslatableText('menu.wireless_pairing').format(status=str_or_none(status.wireless_pairing))),
            else_none(status.wireless_mode, lambda: TranslatableText('menu.wireless_mode').format(mode=str_or_none(status.wireless_mode))),
        ],
        TranslatableText('sections.bluetooth'): [
            else_none(status.bluetooth_powerup_state, lambda: TranslatableText(
                'menu.bluetooth_powerup_state').format(status=str_or_none(status.bluetooth_powerup_state))),
            else_none(status.bluetooth_power_status, lambda: TranslatableText(
                'menu.bluetooth_power_status').format(status=str_or_none(status.bluetooth_power_status))),
            else_none(status.bluetooth_auto_mute, lambda: TranslatableText(
                'menu.bluetooth_auto_mute').format(status=str_or_none(status.bluetooth_auto_mute))),
            else_none(status.bluetooth_connection, lambda: TranslatableText(
                'menu.bluetooth_connection').format(status=str_or_none(status.bluetooth_connection))),
        ]
    }

    # Filter empty sections
    result = {k: v for k, v in result.items() if any(v)}

    # Filter empty section entries
    for k, v in result.items():
        result[k] = [x for x in v if x is not None]

    return result
