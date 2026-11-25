"""Keeps icons and colours readable on light and dark desktops."""

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor, QIcon, QImage, QPainter, QPalette, QPixmap
from PySide6.QtWidgets import QApplication

from linux_arctis_manager.gui.ui_utils import get_icon_pixmap

TRAY_ICON_NAME = 'arctis-manager-symbolic'

_SAMPLE_SIZE = 64
# A pixel is grey when its channels differ by less than this.
_GREY_SPREAD = 40
# Pixels this far from the median lightness count as a different shade.
_SHADE_SPREAD = 48
_UNIFORM_SHARE = 0.9


def tray_icon() -> QIcon:
    """The app's tray icon, by theme name wherever it is installed.

    Only the panel knows its own colour: given the name of a "-symbolic" icon,
    it recolours it. The bitmap, painted with the application's text colour, is
    for systems where the icon is not installed.
    """
    return QIcon.fromTheme(TRAY_ICON_NAME, QIcon(get_icon_pixmap()))


def _is_monochrome(image: QImage) -> bool:
    lightness = []
    visible = 0
    for y in range(image.height()):
        for x in range(image.width()):
            color = image.pixelColor(x, y)
            # Faint pixels are anti-aliasing, whose colour is imprecise.
            if color.alpha() < 128:
                continue
            visible += 1
            channels = (color.red(), color.green(), color.blue())
            if max(channels) - min(channels) < _GREY_SPREAD:
                lightness.append(color.lightness())
    if not lightness:
        return False
    median = sorted(lightness)[len(lightness) // 2]
    uniform = sum(abs(value - median) <= _SHADE_SPREAD for value in lightness)
    return uniform >= visible * _UNIFORM_SHARE


def tint_monochrome(icon: QIcon, color: QColor) -> QIcon:
    """Paints a single-colour icon with `color`; returns any other icon as is.

    Qt does not recolour symbolic theme icons by itself, so a dark one would
    disappear on a dark window.
    """
    pixmap = icon.pixmap(_SAMPLE_SIZE, _SAMPLE_SIZE)
    if pixmap.isNull() or not _is_monochrome(pixmap.toImage()):
        return icon
    tinted = QPixmap(pixmap)
    painter = QPainter(tinted)
    painter.setCompositionMode(QPainter.CompositionMode.CompositionMode_SourceIn)
    painter.fillRect(tinted.rect(), color)
    painter.end()
    return QIcon(tinted)


def needs_dark_palette(scheme: Qt.ColorScheme, palette: QPalette) -> bool:
    """True when the desktop asks for dark but the palette stayed light.

    Happens with Qt's GTK platform theme on GNOME, whose dark style only sets
    the colour scheme and leaves the GTK theme alone.
    """
    return scheme == Qt.ColorScheme.Dark and palette.color(QPalette.ColorRole.Window).lightness() > 128


def dark_palette() -> QPalette:
    palette = QPalette()
    role = QPalette.ColorRole
    colors = {
        role.Window: '#2b2b2f', role.WindowText: '#eeeeee',
        role.Base: '#1f1f22', role.AlternateBase: '#27272b', role.Text: '#eeeeee',
        role.Button: '#36363b', role.ButtonText: '#eeeeee', role.BrightText: '#ffffff',
        role.Light: '#55555c', role.Midlight: '#45454b', role.Mid: '#27272b',
        role.Dark: '#18181a', role.Shadow: '#000000',
        role.Highlight: '#3584e4', role.HighlightedText: '#ffffff',
        role.ToolTipBase: '#1f1f22', role.ToolTipText: '#eeeeee',
        role.PlaceholderText: '#9a9a9f', role.Link: '#78aeed', role.LinkVisited: '#c0a0e8',
    }
    for color_role, value in colors.items():
        palette.setColor(color_role, QColor(value))
    for color_role in (role.WindowText, role.Text, role.ButtonText):
        palette.setColor(QPalette.ColorGroup.Disabled, color_role, QColor('#77777d'))
    return palette


def follow_dark_scheme(app: QApplication) -> None:
    """Applies a dark palette at startup if the platform theme failed to."""
    if needs_dark_palette(app.styleHints().colorScheme(), app.palette()):
        # The native style may ignore palettes; Fusion honours them.
        app.setStyle('Fusion')
        app.setPalette(dark_palette())
