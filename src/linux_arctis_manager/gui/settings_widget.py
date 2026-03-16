from threading import Lock
from typing import Callable

from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (QComboBox, QHBoxLayout, QLabel, QSlider,
                               QVBoxLayout, QWidget)

from linux_arctis_manager.config import ConfigSetting, SettingType
from linux_arctis_manager.gui.dbus_wrapper import DbusWrapper
from linux_arctis_manager.gui.qt_widgets.q_dual_state import QDualState
from linux_arctis_manager.i18n import I18n


class QSettingsWidget(QWidget):
    sig_list_received = Signal(object)

    main_layout: QVBoxLayout

    title: str
    dbus_settings_section: str
    settings: dict[str, int|bool|str]
    settings_config: dict[str, ConfigSetting]

    def __init__(self, parent: QWidget, i18n_section_name: str, dbus_settings_section: str):
        super().__init__(parent)

        layout = QVBoxLayout()
        layout.setAlignment(Qt.AlignmentFlag.AlignTop)
        self.setLayout(layout)

        title = I18n.get_instance().translate('ui', i18n_section_name)
        title_widget = QLabel(title)
        title_font = title_widget.font()
        title_font.setBold(True)
        title_font.setPointSize(16)
        title_widget.setFont(title_font)
        layout.addWidget(title_widget)

        self.main_layout = QVBoxLayout()
        layout.addLayout(self.main_layout)

        self.title = I18n.get_instance().translate('ui', i18n_section_name)
        self.dbus_settings_section = dbus_settings_section
        self.settings = {}
        self._settings_widgets: dict[str, QWidget] = {}
        self._option_lists: dict[str, list[dict[str, str]]] = {}

        self.sig_list_received.connect(self.on_options_list_received)

        self.refresh_lock = Lock()
    
    def on_options_list_received(self, option_list: dict[str, str|list[dict[str, str]]]):
        name = option_list['name']
        lst = option_list['list']

        if isinstance(name, str) and isinstance(lst, list):
            self._option_lists[name] = lst
        
        self.refresh_panel()
    
    def refresh_panel(self):
        with self.refresh_lock:
            # Clear all the previous settings
            keys_to_remove = list(self._settings_widgets.keys())
            for key in keys_to_remove:
                self._settings_widgets[key].deleteLater()
                del self._settings_widgets[key]

            # Mapp all the settings
            for name, value in self.settings.items():
                if not name in self._settings_widgets:
                    widget = self.get_widget(self.settings_config[name], value, self.on_settings_updated)

                    if widget is None:
                        continue

                    self._settings_widgets[name] = widget
                    self.main_layout.addWidget(self._settings_widgets[name])
    
    def update_settings(self, new_settings: dict):
        self.settings_config = {}
        for config_name, kwargs in new_settings.get('settings_config', {}).items():
            self.settings_config[config_name] = ConfigSetting(name=config_name, **kwargs)
            if self.settings_config[config_name].type == SettingType.SELECT \
                and self.settings_config[config_name].options_source not in self._option_lists:
                DbusWrapper.request_list_options(self.settings_config[config_name].options_source, self.sig_list_received)

        settings: dict[str, int|bool|str]|None = new_settings.get(self.dbus_settings_section, None)
        if settings is None or settings == self.settings:
            return

        # Clear all the previous settings that don't apply anymore (device disconnected? new device? etc)
        remove_keys = [key for key in self._settings_widgets if key not in settings]
        for key in remove_keys:
            self._settings_widgets[key].deleteLater()
            del self._settings_widgets[key]
        
        self.settings = settings

        self.refresh_panel()


    def on_settings_updated(self, config: ConfigSetting, value: int|str|bool):
        self.settings[config.name] = value

        dbus_value = value
        if config.type == SettingType.TOGGLE:
            dbus_value = config.values.get('on', True) if value else config.values.get('off', False)

        DbusWrapper.change_setting(config.name, dbus_value)

    def get_widget(self, config: ConfigSetting, value: bool|str|int, callback: Callable) -> QWidget|None:
        main_widget = QWidget()
        main_layout = QHBoxLayout()
        main_widget.setLayout(main_layout)

        widget: QWidget|None = None
        if config.type == SettingType.TOGGLE:
            widget = QDualState(
                off_text=I18n.get_instance().translate('settings_values', config.values.get('off_label', 'off')),
                on_text=I18n.get_instance().translate('settings_values', config.values.get('on_label', 'on')),
                init_state='right' if value == config.values.get('on') else 'left',
            )
            widget.checkStateChanged.connect(lambda state: callback(config, state == Qt.CheckState.Checked))
        elif config.type == SettingType.SLIDER:
            widget = QWidget()
            widget_layout = QHBoxLayout()
            widget.setLayout(widget_layout)

            slider = QSlider(Qt.Orientation.Horizontal)
            slider.setMinimum(config.min)
            slider.setMaximum(config.max)
            slider.setSingleStep(config.step)
            slider.setValue(int(float(value)))
            widget_layout.addWidget(slider)

            def slider_value_callback(config: ConfigSetting) -> Callable[[bool|str|int], str]:
                def get_slider_value(value: bool|str|int) -> str:
                    return I18n.get_instance().translate(
                        'settings_values',
                        config.get_kwargs().get('values_mapping', {}).get(f'{value}', value)
                    )

                return get_slider_value

            slider_value = slider_value_callback(config)
            widget_value_label = QLabel(slider_value(value))
            widget_layout.addWidget(widget_value_label)

            slider.valueChanged.connect(lambda value: widget_value_label.setText(slider_value(value)))
            slider.valueChanged.connect(lambda value: callback(config, value))
        elif config.type == SettingType.SELECT:
            widget = QComboBox()
            options = self._option_lists.get(config.options_source, [])
            if options:
                widget.addItems([o['name'] for o in options])
                option = next((o for o in options if o['id'] == value), None)
                widget.setCurrentIndex(options.index(option or options[0]))
            widget.currentIndexChanged.connect(lambda index: callback(config, self._option_lists[config.options_source][index]['id']))
        else:
            widget = QLabel(f'UNKNOWN TYPE: {config.type}')

        if widget:
            main_layout.addWidget(QLabel(I18n.get_instance().translate('settings', config.name)))
            main_layout.addWidget(widget)
        
        return main_widget if widget else None
