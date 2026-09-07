"""Tests for QSystrayApp's dynamic tooltip (battery/chat-mix at a glance).

Builds a bare instance via __new__ (skipping __init__) so these stay plain
unit tests: no QApplication, no D-Bus connection. Status fields are looked
up by the daemon-assigned `role` (see ascii_bar.py's module doc and
device-configs/*.yaml), not by raw field name — those vary a lot per device
(headset_batt_level, battery, battery_level, battery_status, ...).
"""

from linux_arctis_manager.gui.systray_app import QSystrayApp


def _tray_with_status(status: dict) -> QSystrayApp:
    tray = QSystrayApp.__new__(QSystrayApp)
    tray.last_device_status = status
    return tray


def test_tooltip_has_no_extra_lines_without_battery_or_chatmix():
    tray = _tray_with_status({'headset': {'noise_cancelling': {'value': True, 'type': 'on_off'}}})

    assert tray._build_tooltip() == 'Arctis Manager'


def test_tooltip_includes_headset_battery_line_when_present():
    tray = _tray_with_status({
        'headset': {'headset_batt_level': {'value': 82, 'type': 'percentage', 'role': 'battery_headset'}},
    })

    tooltip = tray._build_tooltip()

    assert '82%' in tooltip
    assert '█' in tooltip


def test_tooltip_includes_dock_battery_when_present():
    tray = _tray_with_status({
        'headset': {'charger_batt_level': {'value': 40, 'type': 'percentage', 'role': 'battery_dock'}},
    })

    assert '40%' in tray._build_tooltip()


def test_tooltip_shows_earbud_batteries_instead_of_headset_when_no_single_battery():
    tray = _tray_with_status({'headset': {
        'left_battery_percent': {'value': 70, 'type': 'percentage', 'role': 'battery_left'},
        'right_battery_percent': {'value': 65, 'type': 'percentage', 'role': 'battery_right'},
        'case_battery_percent': {'value': 90, 'type': 'percentage', 'role': 'battery_case'},
    }})

    tooltip = tray._build_tooltip()

    assert '70%' in tooltip
    assert '65%' in tooltip
    assert '90%' in tooltip


def test_tooltip_includes_chatmix_line_when_both_game_and_chat_present():
    tray = _tray_with_status({'headset': {
        'chatmix_game': {'value': 50, 'type': 'uint8', 'role': 'chatmix_game'},
        'chatmix_chat': {'value': 50, 'type': 'uint8', 'role': 'chatmix_chat'},
    }})

    tooltip = tray._build_tooltip()

    assert '●' in tooltip


def test_tooltip_omits_chatmix_line_when_only_one_side_present():
    # A daemon bug/incomplete emit shouldn't crash the tooltip build — just
    # skip the line since a lone value can't be turned into a balance.
    tray = _tray_with_status({'headset': {
        'chatmix_game': {'value': 50, 'type': 'uint8', 'role': 'chatmix_game'},
    }})

    assert '●' not in tray._build_tooltip()


def test_tooltip_aligns_battery_labels_to_the_longest_one():
    tray = _tray_with_status({'headset': {
        'headset_batt_level': {'value': 75, 'type': 'percentage', 'role': 'battery_headset'},
        'charger_batt_level': {'value': 100, 'type': 'percentage', 'role': 'battery_dock'},
    }})

    lines = tray._build_tooltip().splitlines()
    label_widths = {len(line.split(':', 1)[0]) for line in lines[1:]}

    # Both labels padded to the same character width so the ':' lines up.
    assert len(label_widths) == 1


def test_tooltip_omits_chatmix_on_devices_without_hw_chatmix():
    tray = _tray_with_status({
        'headset': {'headset_batt_level': {'value': 60, 'type': 'percentage', 'role': 'battery_headset'}},
    })

    tooltip = tray._build_tooltip()

    assert '●' not in tooltip
    assert '60%' in tooltip
