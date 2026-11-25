from linux_arctis_manager.i18n import I18n
from tests.desktop.common import sample_data


def _fields():
    return {name: field for category in sample_data.STATUS.values() for name, field in category.items()}


def test_status_has_the_fields_the_tray_shells_summarise():
    roles = {field.get('role') for field in _fields().values()}

    assert {'battery_headset', 'chatmix_game', 'chatmix_chat'} <= roles


def test_status_names_are_translated():
    for category, fields in sample_data.STATUS.items():
        for name in (category, *fields):
            assert I18n.translate('status', name) != name, name


def test_pinned_settings_are_defined_and_have_a_value():
    for name in sample_data.PINNED_SETTINGS:
        assert name in sample_data.SETTINGS['settings_config']
        assert name in sample_data.SETTINGS['device']
        assert I18n.translate('settings', name) != name, name
