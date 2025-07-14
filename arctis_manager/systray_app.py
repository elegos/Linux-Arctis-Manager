import locale
import logging
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any, Callable, Optional

from PyQt6 import QtSvg
from PyQt6.QtCore import Qt
from PyQt6.QtGui import QAction, QIcon, QImage, QPainter, QPalette, QPixmap
from PyQt6.QtWidgets import QApplication, QMenu, QSystemTrayIcon

from arctis_manager.device_manager import DeviceStatus
from arctis_manager.device_manager.device_manager import DeviceManager
from arctis_manager.i18n_helpers import get_translated_menu_entries
from arctis_manager.qt_utils import get_icon_pixmap
from arctis_manager.settings_window import SettingsWindow
from arctis_manager.translations import Translations


class SystrayApp:
    log: logging.Logger

    app: QApplication
    tray_icon: QSystemTrayIcon
    menu: QMenu
    last_device_status: Optional[DeviceStatus] = None

    def get_systray_icon_pixmap(self, path: Path) -> QPixmap:
        brush_color = QApplication.palette().color(QPalette.ColorRole.Text)

        xml_tree = ET.parse(path.absolute().as_posix())
        xml_root = xml_tree.getroot()

        for path in xml_root.findall('.//{http://www.w3.org/2000/svg}path'):
            path.set('fill', brush_color.name())

        xml_str = ET.tostring(xml_root)

        svg_renderer = QtSvg.QSvgRenderer(xml_str)

        # Create the empty image
        image = QImage(64, 64, QImage.Format.Format_ARGB32_Premultiplied)
        image.fill(Qt.GlobalColor.transparent)

        # Initialize the painter
        painter = QPainter(image)
        painter.setBrush(brush_color)
        painter.setPen(Qt.PenStyle.NoPen)

        # Render the image on the QImage
        svg_renderer.render(painter)

        # Rendering end
        painter.end()

        pixmap = QPixmap.fromImage(image)

        return pixmap

    def __init__(self, app: QApplication, log_level: int):
        self.setup_logger(log_level)
        self.app = app

        pixmap = get_icon_pixmap()

        self.tray_icon = QSystemTrayIcon(QIcon(pixmap), parent=self.app)
        self.tray_icon.setToolTip('Arctis Manager')

        lang_code, _ = locale.getdefaultlocale()
        lang_code = lang_code.split('_')[0]

        self.menu = QMenu()
        self.tray_icon.setContextMenu(self.menu)

    def setup_logger(self, log_level: int):
        self.log = logging.getLogger('SystrayApp')
        self.log.setLevel(log_level)

    async def start(self):
        self.log.info('Starting Systray app.')
        self.tray_icon.show()

        self.app.exec()

    def stop(self):
        if hasattr(self, '_stopping') and self._stopping:
            return
        self._stopping = True

        self.log.debug('Received shutdown signal, shutting down.')
        self.app.quit()

    def on_device_status_update(self, device_manager: DeviceManager, status: DeviceStatus) -> None:
        if device_manager is None or status is None:
            return

        self.menu.clear()
        if not hasattr(self, '_menu_actions'):
            self._menu_actions = {}

        menu_sections = get_translated_menu_entries(status).values()
        menu_sections_flatlist = [v for sublist in menu_sections for v in sublist]

        has_previous_section = False
        for section in menu_sections:
            if has_previous_section:
                self.menu.addSeparator()
            has_previous_section = True

            for item in section:
                if item.dot_notation_key not in self._menu_actions:
                    self._menu_actions[item.dot_notation_key] = QAction(str(item))
                    self._menu_actions[item.dot_notation_key].setEnabled(False)
                else:
                    self._menu_actions[item.dot_notation_key].setText(str(item))

                self.menu.addAction(self._menu_actions[item.dot_notation_key])

        if has_previous_section:
            self.menu.addSeparator()

        self._device_manager = device_manager
        self.last_device_status = status

        if len(device_manager.get_configurable_settings(self.last_device_status).keys()) > 0:
            if not '_settings' in self._menu_actions:
                self._menu_actions['_settings'] = QAction(Translations.get_instance().get_translation('app.settings_label'))
                self._menu_actions['_settings'].triggered.connect(self.open_settings_window)
            self.menu.addAction(self._menu_actions['_settings'])

        # Menu cleanup
        expected_menu_keys = ['_settings', *[t.dot_notation_key for t in menu_sections_flatlist]]
        for key in self._menu_actions.keys():
            if key not in expected_menu_keys:
                self.menu.removeAction(self._menu_actions[key])
                del self._menu_actions[key]

        # Update values in (opened) settings window
        if hasattr(self, '_settings_window'):
            self._settings_window.update_status(self.last_device_status)

    def open_settings_window(self):
        if hasattr(self, '_settings_window') and self._settings_window.isVisible():
            self._settings_window.raise_()
            return

        self._settings_window = SettingsWindow(self._device_manager, self.last_device_status)
        self._settings_window.setWindowFlags(Qt.WindowType.Window)

        self._settings_window.show()
