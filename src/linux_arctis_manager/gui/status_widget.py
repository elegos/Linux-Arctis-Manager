from PySide6.QtCore import Qt
from PySide6.QtWidgets import QLabel, QScrollArea, QVBoxLayout, QWidget

from linux_arctis_manager.i18n import I18n


class QStatusWidget(QWidget):
    main_layout: QVBoxLayout

    def __init__(self, parent: QWidget):
        super().__init__(parent)
        self._settings_config: dict = {}

        outer = QVBoxLayout()
        outer.setContentsMargins(0, 0, 0, 0)
        self.setLayout(outer)

        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QScrollArea.Shape.NoFrame)
        scroll_content = QWidget()
        self.main_layout = QVBoxLayout()
        self.main_layout.setAlignment(Qt.AlignmentFlag.AlignTop)
        scroll_content.setLayout(self.main_layout)
        scroll.setWidget(scroll_content)
        outer.addWidget(scroll)

    def update_settings_config(self, settings: dict) -> None:
        self._settings_config = settings.get('settings_config', {})

    def clean_layout(self):
        while self.main_layout.count():
            item = self.main_layout.takeAt(0)
            widget = item.widget()
            if widget is not None:
                widget.deleteLater()

    @staticmethod
    def format_value(status: str, status_o: dict[str, str|int], settings_config: dict) -> str:
        val = status_o['value']
        dtype = status_o.get('type')
        if dtype == 'percentage':
            return f"{val}%"
        if dtype == 'on_off':
            return I18n.translate('status_values', 'on' if val else 'off')
        if isinstance(val, (int, float)) and dtype in ('uint8', 'uint16', 'uint32'):
            cfg = settings_config.get(status, {})
            vm = cfg.get('values_mapping', {})
            label_key = vm.get(str(int(val)), str(int(val))) if vm else str(int(val))
            return I18n.translate('status_values', label_key)
        return I18n.translate('status_values', val)

    def update_status(self, new_status: dict[str, dict[str, dict[str, str|int]]]):
        if hasattr(self, 'status') and new_status == self.status:
            return

        self.status = new_status

        self.clean_layout()
        if not self.status:
            label = QLabel(I18n.get_instance().translate('ui', 'no_device_detected'))
            label.font().setBold(True)
            self.main_layout.addWidget(label)

            return

        index = 0
        for category, status_obj in self.status.items():
            if index > 0:
                line_separator = QWidget()
                line_separator.setFixedHeight(2)
                self.main_layout.addWidget(line_separator)
            index += 1

            category_label = QLabel(I18n.get_instance().translate('status', category))
            category_font = category_label.font()
            category_font.setBold(True)
            category_font.setPointSize(16)
            category_label.setFont(category_font)
            self.main_layout.addWidget(category_label)

            skip_fields: set[str] = set()
            transparency_mode = status_obj.get('transparency_mode', {}).get('value', '')
            if transparency_mode != 'transparent':
                skip_fields.add('transparent_level')

            for status, status_o in status_obj.items():
                if status in skip_fields:
                    continue
                display = self.format_value(status, status_o, self._settings_config)
                label = QLabel(f"{I18n.translate('status', status)}: {display}")
                self.main_layout.addWidget(label)
