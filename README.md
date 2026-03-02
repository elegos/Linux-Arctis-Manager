# Linux Arctis Manager

A replacement for SteelSeries GG software, to manage your Arctis device on Linux!

[![CI](https://github.com/elegos/Linux-Arctis-Manager/actions/workflows/wheel-install-test.yaml/badge.svg?branch=develop)](https://github.com/elegos/Linux-Arctis-Manager/actions/workflows/wheel-install-test.yaml)

## 👍 Key points

- Enable the Media and Chat audio streams
- Configure any device via a simple configuration file
- Enable per-device features by adding them in the relative configuration file
- D-Bus based communication, to support different clients (alternative clients, Plasma extensions, etc)

## 🎧 Supported devices table

|Device|Channels mix|Advanced features|Product ID(s)|
|------|------------|-----------------|-------------|
|Arctis 1 / Xbox                      |❌|❌|12b3, 12b6|
|Arctis 1 Wireless                    |❌|❌||
|Arctis 3 Console Edition             |❌|❌||
|Arctis 7 / Gen 2                     |❌|❌|1260, 12ad|
|Arctis 7+ / PS5 / Xbox / Destiny     |❌|❌|220e, 2212, 2216, 2236|
|Arctis Nova 3                        |❌|❌|12ec|
|Arctis Nova 3P Wireless / 3X Wireless|❌|❌|2269, 226d|
|Arctis Nova 5                        |❌|❌|2232, 2253|
|Arctis Nova 7P                       |❌|❌|220a|
|Arctis Nova 7X                       |❌|❌|12d7|
|Arctis Nova 9                        |❌|❌|12c2|
|Arctis Nova Elite                    |❌|❌||
|Arctis Nova Pro Wireless / X         |✅|✅|12e0, 12e5|
|Arctis Nova Pro                      |❌|❌||
|Arctis Pro GameDAC                   |❌|❌|1280|
|Arctis Pro Wireless                  |❌|❌|1290|
|Arctis Pro                           |❌|❌|1252|


\*: experimental support, needs test by the community. Open an issue in case of problem(s)

## CLI commands

- `lam-daemon`: executed by user-level systemd.
- `lam-cli`: to run utilities like udev rules generation, desktop entries installation etc.
- `lam-gui`: the graphical user interface, to alter settings and see the device's status.

Each command can be called with `-h` or `--help` to get all the options for the commands and subcommands.

## 🖥️ Install & setup

### Distrobox

Note on Distrobox (immutable distros like Bazzite, just to be **aware**): distrobox executes applications within containers, mounting `/var`, `/etc` and your `/home` in the container itself. This application heavily relies on your `/home`, thus distrobox is not really used but for the installation process.

```bash
curl -LsSf https://raw.githubusercontent.com/elegos/Linux-Arctis-Manager/refs/heads/develop/scripts/distrobox.sh | sh
```
### Arch Linux (AUR)

Arch Linux users can install the community-maintained package from the Arch User Repository (AUR). Please report any packaging-specific issues directly to the AUR maintainers [@tonitch](https://aur.archlinux.org/account/tonitch) and [@Aiyahhh](https://aur.archlinux.org/account/Aiyahhh)

- [Linux Arctis Manager on AUR](https://aur.archlinux.org/packages/linux-arctis-manager)

### Manual install

#### Prerequisites

- `uv` ([installation guide](https://docs.astral.sh/uv/getting-started/installation/))
- `pip` or `pipx` (some distros will **REQUIRE** pipx). **pipx is recommended** for dependencies isolation, while pip will have a smaller footprint.

#### Commands

```bash
# Prerequisites
curl -LsSf https://astral.sh/uv/install.sh | sh
# install pipx with your package manager
mkdir -p $HOME/.local/share/applications

# Download the sources and enter them
git clone https://github.com/elegos/Linux-Arctis-Manager.git # first time only
cd Linux-Arctis-Manager
git pull # to get the latest version available

# Build the .whl package. Skip this if downloading from releases page
rm -rf dist
uv build

# Install the .whl package
find ./dist -name "*.whl" | head -n1 | xargs pipx install --force
# ALT using pip: find ./dist -name "*.whl" | head -n1 | xargs pip install --user --force-reinstall

# Setup
lam-cli desktop write # Only to produce the desktop entries; optional after first installation
lam-cli udev write-rules --force --reload # Required for first installation or new devices support only
```

## ⛔ Uninstall / cleanup
- `lam-cli desktop remove` (remove the desktop menu entries)
- `systemctl --user disable --now arctis-manager` (disables and stops arctis-manager service)
- `rm ~/.config/systemd/user/arctis-manager.service` (removes the systemd's user-level arctis-manager service file)
- `rm ~/.config/arctis_manager` (user preferences and custom device / lang files)
- `sudo rm /usr/lib/udev/rules.d/91-steelseries-arctis.rules` (remove udev rules)
- `pipx uninstall linux_arctis_manager` (pipx)
- `pip uninstall linux_arctis_manager` (pip)

## ⌨️ Development

### Basic commands

- Run the daemon: `uv run lam-daemon`
- Run the CLI: `uv run lam-cli`
- Run the GUI: `uv run lam-gui [--no-enforce-systemd]` - use the option to avoid force enabling the daemon, in case you're working on it

### Documentation

- [How to add support to a new device](docs/device_support.md)
- [Wireshark quick tutorial](https://www.youtube.com/watch?v=zWbdnHwTr3M)
- [Device configuration specs](docs/device_configuration_file_specs.md)
- [Dbus messaging](docs/dbus.md)
