"""Icon and palette handling for light and dark desktops (offscreen Qt)."""

import os

import pytest
from PySide6.QtCore import Qt
from PySide6.QtGui import QColor, QIcon, QPainter, QPalette, QPixmap
from PySide6.QtWidgets import QApplication

from linux_arctis_manager.gui import theme

WHITE = QColor('#ffffff')


@pytest.fixture(scope='module')
def app():
    os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')
    return QApplication.instance() or QApplication([])


def _icon(*stripes: str) -> QIcon:
    pixmap = QPixmap(64, 64)
    pixmap.fill(Qt.GlobalColor.transparent)
    painter = QPainter(pixmap)
    width = 48 // len(stripes)
    for index, color in enumerate(stripes):
        painter.fillRect(8 + index * width, 8, width, 48, QColor(color))
    painter.end()
    return QIcon(pixmap)


def _center(icon: QIcon) -> str:
    return icon.pixmap(64, 64).toImage().pixelColor(32, 32).name()


def test_monochrome_icon_takes_the_given_color(app):
    tinted = theme.tint_monochrome(_icon('#232629'), WHITE)

    assert _center(tinted) == '#ffffff'
    assert tinted.pixmap(64, 64).toImage().pixelColor(0, 0).alpha() == 0


def test_coloured_icon_is_left_alone(app):
    icon = _icon('#d02020')

    assert _center(theme.tint_monochrome(icon, WHITE)) == '#d02020'


def test_shaded_grey_icon_is_left_alone(app):
    icon = _icon('#101010', '#f0f0f0')

    assert theme.tint_monochrome(icon, WHITE) is icon


def test_null_icon_stays_null(app):
    assert theme.tint_monochrome(QIcon(), WHITE).isNull()


def test_tray_icon_is_the_themed_symbolic_one_so_the_panel_can_recolor_it(app, monkeypatch):
    requested = []

    def from_theme(name, fallback):
        requested.append(name)
        return fallback

    monkeypatch.setattr(theme.QIcon, 'fromTheme', staticmethod(from_theme))

    assert not theme.tray_icon().isNull()
    assert requested == ['arctis-manager-symbolic']


def _palette(window: str) -> QPalette:
    palette = QPalette()
    palette.setColor(QPalette.ColorRole.Window, QColor(window))
    return palette


@pytest.mark.parametrize(('scheme', 'window', 'expected'), [
    (Qt.ColorScheme.Dark, '#fafafa', True),
    (Qt.ColorScheme.Dark, '#242424', False),
    (Qt.ColorScheme.Light, '#fafafa', False),
    (Qt.ColorScheme.Unknown, '#fafafa', False),
])
def test_dark_palette_is_needed_only_when_a_dark_desktop_left_the_app_light(scheme, window, expected):
    assert theme.needs_dark_palette(scheme, _palette(window)) is expected


def test_dark_palette_is_dark_with_light_text():
    palette = theme.dark_palette()

    assert palette.color(QPalette.ColorRole.Window).lightness() < 80
    assert palette.color(QPalette.ColorRole.WindowText).lightness() > 180
