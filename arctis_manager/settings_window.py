import re
from typing import Callable, Optional

from PyQt6.QtCore import Qt
from PyQt6.QtGui import QCloseEvent, QIcon
from PyQt6.QtWidgets import (QComboBox, QFormLayout, QHBoxLayout, QLabel,
                             QLayout, QListWidget, QSlider, QStackedWidget,
                             QVBoxLayout, QWidget)

from arctis_manager.config_manager import ConfigManager
from arctis_manager.custom_widgets.q_toggle import QToggle
from arctis_manager.device_manager.device_manager import DeviceManager
from arctis_manager.device_manager.device_settings import (SliderSetting,
                                                           ToggleSetting)
from arctis_manager.device_manager.device_status import DeviceStatus
from arctis_manager.i18n_helpers import get_translated_menu_entries
from arctis_manager.pactl_utils import PactlDevice, pactl_get_available_sinks
from arctis_manager.qt_utils import get_icon_pixmap
from arctis_manager.translations import Translations


class SettingsWindow(QWidget):
    manager: DeviceManager

    def __init__(self, manager: DeviceManager, status: DeviceStatus, parent: QWidget = None):
        super().__init__(parent=parent)

        self.manager = manager

        i18n = Translations.get_instance()

        self.setWindowTitle(i18n.get_translation('app.settings_window_title'))
        # Note: Wayland does not support window icons (yet?)
        self.setWindowIcon(QIcon(get_icon_pixmap()))

        # Set the minimum size and adjust to screen geometry
        self.setMinimumSize(800, 600)
        available_geometry = self.screen().availableGeometry()
        self.resize(min(800, available_geometry.width()), min(600, available_geometry.height()))

        sections = manager.get_configurable_settings(status)

        # Create a list widget for sections on the left
        section_list = QListWidget()
        section_list.addItem(i18n.get_translation('sections', 'device_status'))
        section_list.addItem(i18n.get_translation('sections', 'general_settings'))
        section_list.addItems([i18n.get_translation('sections', section) for section in sections.keys()])
        section_list.setFixedWidth(max(section_list.sizeHintForColumn(0), 200))
        section_list.currentRowChanged.connect(self.change_panel)

        # Create a stacked widget for panels on the right
        panel_stack = QStackedWidget()

        # Status panel
        self._status_panel = QWidget()
        layout = QFormLayout()
        layout.setAlignment(Qt.AlignmentFlag.AlignTop)

        for section, status_strings in get_translated_menu_entries(status).items():
            section_label = QLabel(str(section))
            label_font = section_label.font()
            label_font.setBold(True)
            section_label.setFont(label_font)
            layout.addWidget(section_label)

            for status_string in status_strings:
                layout.addRow('', QLabel(str(status_string)))

        self._status_panel.setLayout(layout)
        panel_stack.addWidget(self._status_panel)

        # General settings panel
        self._general_panel = self._setup_general_settings()
        panel_stack.addWidget(self._general_panel)

        # Settings panels
        for settings in sections.values():
            panel = QWidget()
            layout = QFormLayout()
            layout.setAlignment(Qt.AlignmentFlag.AlignTop)

            for setting in settings:
                widget_layout = QWidget()
                w_layout = QHBoxLayout()
                widget_layout.setLayout(w_layout)
                w_layout.addWidget(QLabel(f'{setting.name}: '))

                widget_layout: QLayout = None

                if isinstance(setting, SliderSetting):
                    widget_layout = self.get_slider_configuration_widget(
                        setting.min_value, setting.max_value, setting.step,
                        setting.current_state, setting.min_label, setting.max_label,
                        setting.on_value_change
                    )
                elif isinstance(setting, ToggleSetting):
                    widget_layout = self.get_checkbox_configuration_widget(
                        setting.untoggled_label, setting.toggled_label, setting.current_state,
                        setting.on_value_change
                    )

                if widget_layout is not None:
                    layout.addRow(setting.name, widget_layout)
            panel.setLayout(layout)

            panel_stack.addWidget(panel)

        # Window layout

        # Main body widget
        body_widget = QWidget()
        body_layout = QHBoxLayout()
        body_layout.addWidget(section_list)
        body_layout.addWidget(panel_stack)
        body_widget.setLayout(body_layout)

        # Device name widget
        device_name_label = QLabel(manager.get_device_name())
        font = device_name_label.font()
        font.setBold(True)
        font.setPointSize(16)
        device_name_label.setFont(font)

        # Main widget
        main_layout = QVBoxLayout()

        # Add widgets to the layout
        main_layout.addWidget(device_name_label)
        main_layout.addWidget(body_widget)
        self.setLayout(main_layout)

    def update_status(self, status: DeviceStatus):
        layout = self._status_panel.layout()
        # Clear the layout from the previous status
        while layout.count():
            item = layout.takeAt(0)
            widget = item.widget()
            if widget is not None:
                widget.deleteLater()

        for section, status_strings in get_translated_menu_entries(status).items():
            section_label = QLabel(str(section))
            label_font = section_label.font()
            label_font.setBold(True)
            section_label.setFont(label_font)
            layout.addWidget(section_label)

            for status_string in status_strings:
                layout.addRow('', QLabel(str(status_string)))

    def change_panel(self, index):
        # Change the panel based on the selected section
        self.findChild(QStackedWidget).setCurrentIndex(index)

    def get_slider_configuration_widget(
        self, min: int, max: int, step: int, default_value: int, min_label: str, max_label: str, on_value_changed: Optional[Callable[[int], None]]
    ) -> QLayout:
        layout = QHBoxLayout()

        controller = QSlider(orientation=Qt.Orientation.Horizontal)
        controller.setMinimum(min)
        controller.setMaximum(max)
        controller.setSingleStep(step)
        controller.setValue(default_value)

        # layout.addWidget(QLabel(f'{name}: '))
        layout.addWidget(QLabel(min_label))
        layout.addWidget(controller)
        layout.addWidget(QLabel(max_label))

        if on_value_changed is not None:
            controller.sliderReleased.connect(lambda: on_value_changed(controller.value()))

        return layout

    def get_checkbox_configuration_widget(
        self, off_label: str, on_label: str, toggled: bool, on_value_changed: Optional[Callable[[int], None]]
    ) -> QLayout:
        layout = QHBoxLayout()
        layout.setAlignment(Qt.AlignmentFlag.AlignLeft)

        controller = QToggle()
        controller.setChecked(toggled)

        layout.addWidget(QLabel(off_label))
        layout.addWidget(controller)
        layout.addWidget(QLabel(on_label))

        if on_value_changed is not None:
            controller.stateChanged.connect(lambda: on_value_changed(controller.isChecked()))

        return layout

    def _setup_general_settings(self) -> QWidget:
        panel = QWidget()
        layout = QFormLayout()
        layout.setAlignment(Qt.AlignmentFlag.AlignTop)

        i18n = Translations.get_instance()

        # Device switch
        ds_widget = QWidget()
        ds_layout = QHBoxLayout()
        ds_widget.setLayout(ds_layout)
        ds_widget = self.get_checkbox_configuration_widget(
            i18n.get_translation('device_status_values', 'on_off', 'off').capitalize(),
            i18n.get_translation('device_status_values', 'on_off', 'on').capitalize(),
            ConfigManager.get_instance().get_general_config().get('device_switch', True),
            self.on_device_switch_changed,
        )

        self.add_general_setting_row(layout, i18n.get_translation('general_settings', 'device_switch'),
                                     i18n.get_translation('general_settings', 'device_switch_description'), ds_widget)

        # Smart device switch
        ds_widget = QWidget()
        ds_layout = QHBoxLayout()
        ds_widget.setLayout(ds_layout)
        ds_widget = self.get_checkbox_configuration_widget(
            i18n.get_translation('device_status_values', 'on_off', 'off').capitalize(),
            i18n.get_translation('device_status_values', 'on_off', 'on').capitalize(),
            ConfigManager.get_instance().get_general_config().get('smart_device_switch', True),
            self.on_smart_device_switch_changed,
        )

        self.add_general_setting_row(layout, i18n.get_translation('general_settings', 'smart_device_switch'),
                                     i18n.get_translation('general_settings', 'smart_device_switch_description'), ds_widget)

        # Fallback device
        fallback_device_widget = QWidget()
        fallback_device_layout = QHBoxLayout()
        fallback_device_widget.setLayout(fallback_device_layout)

        arctis_re = re.compile(r'.*[Aa]rctis.*')
        devices = [device for device in pactl_get_available_sinks() if not arctis_re.match(device.description)]
        current_fallback_device_name = ConfigManager.get_instance().get_general_config().get('fallback_device')
        if current_fallback_device_name is None:
            self.on_fallback_device_changed(devices[0])
            current_fallback_device_name = devices[0].description

        select_box = QComboBox()
        for device in devices:
            select_box.addItem(device.description, device)
            if device.description == current_fallback_device_name:
                select_box.setCurrentText(device.description)
        select_box.currentTextChanged.connect(lambda text: self.on_fallback_device_changed(next(device for device in devices if device.description == text)))
        fallback_device_layout.addWidget(select_box)

        self.add_general_setting_row(layout, i18n.get_translation('general_settings', 'fallback_device'),
                                     i18n.get_translation('general_settings', 'fallback_device_description'), fallback_device_widget)

        panel.setLayout(layout)

        return panel

    @staticmethod
    def on_device_switch_changed(enabled: bool) -> None:
        manager = ConfigManager.get_instance()
        config = manager.get_general_config()
        config['device_switch'] = enabled

        manager.save_general_config(config)

    @staticmethod
    def on_smart_device_switch_changed(enabled: bool) -> None:
        manager = ConfigManager.get_instance()
        config = manager.get_general_config()
        config['smart_device_switch'] = enabled

        manager.save_general_config(config)

    @staticmethod
    def on_fallback_device_changed(device: PactlDevice) -> None:
        manager = ConfigManager.get_instance()
        config = manager.get_general_config()
        config['fallback_device'] = device.description

        manager.save_general_config(config)

    def closeEvent(self, event: Optional[QCloseEvent]):
        self.hide()
        event.ignore()

    @staticmethod
    def add_general_setting_row(layout: QFormLayout, name: str, description: str, action: QWidget) -> None:
        label = QLabel(name)
        font = label.font()
        font.setBold(True)
        label.setFont(font)

        layout.addRow(label, action)

        description_label = QLabel(description)
        font = description_label.font()
        font.setItalic(True)
        description_label.setFont(font)
        description_label.setWordWrap(True)

        layout.addRow(description_label)
