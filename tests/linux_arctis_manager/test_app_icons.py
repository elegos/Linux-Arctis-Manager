from pathlib import Path
from xml.etree import ElementTree

REPO = Path(__file__).parents[2]
IMAGES = REPO / 'src/linux_arctis_manager/gui/images'


def test_launcher_icon_carries_its_own_colors():
    # Nothing recolours a launcher icon: it must be readable on light and dark.
    source = (IMAGES / 'launcher_icon.svg').read_text()
    root = ElementTree.fromstring(source)

    assert 'currentColor' not in source
    assert root.find('{http://www.w3.org/2000/svg}rect') is not None


def test_makefile_installs_the_colored_and_the_symbolic_icon():
    makefile = (REPO / 'Makefile').read_text()

    assert '$(LAUNCHER_ICON_SRC) $(DESTDIR)$(ICON_DIR)/arctis-manager.svg' in makefile
    assert '$(ICON_SRC) $(DESTDIR)$(ICON_DIR)/arctis-manager-symbolic.svg' in makefile


def test_gnome_extension_declares_the_current_shell():
    metadata = (REPO / 'packaging/gnome-shell/arctis-manager@giacomofurlan.name/metadata.json').read_text()

    assert '"51"' in metadata
