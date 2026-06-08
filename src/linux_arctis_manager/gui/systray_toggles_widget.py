from PySide6.QtCore import Qt
from PySide6.QtWidgets import QCheckBox, QLabel, QVBoxLayout, QWidget

from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.i18n import I18n


class QSystrayTogglesWidget(QWidget):
    def __init__(self, parent: QWidget):
        super().__init__(parent)

        layout = QVBoxLayout()
        layout.setAlignment(Qt.AlignmentFlag.AlignTop)
        self.setLayout(layout)

        title = I18n.get_instance().translate('ui', 'systray_toggles')
        title_widget = QLabel(title)
        title_font = title_widget.font()
        title_font.setBold(True)
        title_font.setPointSize(16)
        title_widget.setFont(title_font)
        layout.addWidget(title_widget)

        hint = QLabel(I18n.get_instance().translate('ui', 'systray_toggles_hint'))
        hint.setWordWrap(True)
        layout.addWidget(hint)

        self.main_layout = QVBoxLayout()
        layout.addLayout(self.main_layout)

        self._checkboxes: dict[str, QCheckBox] = {}
        self._rendered: dict[str, bool] | None = None

    def update_settings(self, new_settings: dict):
        settings_config = new_settings.get('settings_config', {}) or {}
        device_settings = new_settings.get('device', {}) or {}
        pinned = new_settings.get('systray_toggles', []) or []

        # Only device-scoped TOGGLE settings are eligible for the tray
        toggle_names = [
            name for name, config in settings_config.items()
            if config.get('type') == 'toggle' and name in device_settings
        ]

        # Skip the rebuild when nothing relevant changed
        desired = {name: name in pinned for name in toggle_names}
        if desired == self._rendered:
            return
        self._rendered = desired

        # Clear the previous contents (remove layout items, not just the widgets)
        while self.main_layout.count():
            item = self.main_layout.takeAt(0)
            if item.widget():
                item.widget().deleteLater()
        self._checkboxes = {}

        if not toggle_names:
            empty = QLabel(I18n.get_instance().translate('ui', 'systray_toggles_empty'))
            empty.setWordWrap(True)
            self.main_layout.addWidget(empty)
            return

        for name in toggle_names:
            checkbox = QCheckBox(I18n.get_instance().translate('settings', name))
            checkbox.setChecked(desired[name])

            def _on_toggled(checked, n=name):
                DbusWrapper.set_systray_toggle(n, checked)

            checkbox.toggled.connect(_on_toggled)
            self._checkboxes[name] = checkbox
            self.main_layout.addWidget(checkbox)
