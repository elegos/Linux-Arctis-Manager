"""Made-up device state served by the fake daemon, so the tray shells have
something to show."""

STATUS = {
    'headset': {
        'battery_level': {'value': 70, 'type': 'percentage', 'role': 'battery_headset'},
        'game_chatmix_level': {'value': 60, 'type': 'percentage', 'role': 'chatmix_game'},
        'chat_chatmix_level': {'value': 40, 'type': 'percentage', 'role': 'chatmix_chat'},
    },
}

PINNED_SETTINGS = ['mic_volume', 'mic_side_tone']

SETTINGS = {
    'general': {},
    'device': {'mic_volume': 7, 'mic_side_tone': 3},
    'settings_config': {
        name: {'type': 'slider', 'default_value': 5, 'min': 0, 'max': 10} for name in PINNED_SETTINGS
    },
}
