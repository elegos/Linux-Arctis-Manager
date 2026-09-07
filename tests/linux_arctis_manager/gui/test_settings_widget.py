"""Tests for QSettingsWidget.get_widget()'s available/unavailable_reason handling."""

import sys

from PySide6.QtWidgets import QApplication

# One QApplication for the whole module (required for QObject subclasses).
_app = QApplication.instance() or QApplication(sys.argv)


def _make_widget():
    from linux_arctis_manager.gui.settings_widget import QSettingsWidget
    return QSettingsWidget(None, 'general', 'general')


def _noop_callback(*_args, **_kwargs):
    pass


def test_available_toggle_is_enabled_with_no_tooltip():
    from linux_arctis_manager.config import ConfigSetting

    widget = _make_widget()
    config = ConfigSetting(
        name='redirect_audio_on_connect',
        type='toggle',
        default_value=False,
        values={'on': True, 'off': False, 'on_label': 'on', 'off_label': 'off'},
    )

    main_widget = widget.get_widget(config, False, _noop_callback)

    assert main_widget is not None
    assert main_widget.isEnabled()
    assert main_widget.toolTip() == ''


def test_unavailable_toggle_is_disabled_with_translated_tooltip():
    from linux_arctis_manager.config import ConfigSetting

    widget = _make_widget()
    config = ConfigSetting(
        name='hide_physical_sink',
        type='toggle',
        default_value=False,
        available=False,
        unavailable_reason='wireplumber_required',
        values={'on': True, 'off': False, 'on_label': 'on', 'off_label': 'off'},
    )

    main_widget = widget.get_widget(config, False, _noop_callback)

    assert main_widget is not None
    assert not main_widget.isEnabled()
    assert main_widget.toolTip() == 'Install WirePlumber to use this option'
