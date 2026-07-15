import os

os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QApplication, QLabel

from linux_arctis_manager.gui.settings_widget import QSettingsWidget


def test_settings_widget_keeps_dense_controls_visible():
    app = QApplication.instance() or QApplication([])
    settings = {f'setting_{index}': 5 for index in range(18)}
    settings_config = {
        name: {
            'type': 'slider',
            'default_value': 5,
            'min': 1,
            'max': 10,
            'step': 1,
            'min_label': 'perc_10',
            'max_label': 'perc_100',
        }
        for name in settings
    }

    widget = QSettingsWidget(None, 'device', 'device')
    widget.resize(768, 474)
    widget.update_settings({'device': settings, 'settings_config': settings_config})
    widget.show()
    app.processEvents()

    first_row = widget._settings_widgets['setting_0']
    first_label = first_row.findChild(QLabel)
    assert first_label is not None
    assert first_label.height() > 0
    assert widget.settings_scroll.horizontalScrollBarPolicy() == Qt.ScrollBarPolicy.ScrollBarAlwaysOff
