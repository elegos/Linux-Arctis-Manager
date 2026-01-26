# Linux Arctis Manager

A replacement for SteelSeries GG software, to manage your Arctis device on Linux!

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


## 🖥️ Install & setup

### Build wheel from sources

Installation of uv (one-timer, then restart the terminal and ensure that `uv` is in `$PATH`)
- `pip install --user --upgrade uv`

Wheel file creation
- `rm -rf dist`
- `uv build`

### From wheel (built or from releases page)

Wheel installation
- `pip install --user path/to/linux_arctis_manager-....whl`

### One-time after install

- `lam-cli desktop write` (add the desktop menu entries. It's not strictly required to update them at each update.)

If you want, you can add the systray app to the startup applications. The daemon will be enabled at the first application's run, and will start when the graphical session starts indipendently from the GUI (which is a client to the daemon).

### At each update

- `lam-cli udev write-rules --force --reload` (add / update the udev rules file and reload them)


## Uninstall / cleanup
- `lam-cli desktop remove` (remove the desktop menu entries)
- `systemctl --user disable --now arctis-manager` (disables and stops arctis-manager service)
- `rm ~/.config/systemd/user/arctis-manager.service` (removes the systemd's user-level arctis-manager service file)
- `sudo rm /usr/lib/udev/rules.d/91-steelseries-arctis.rules` (remove udev rules)

## ⌨️ Development

### Basic commands

- Run the daemon: `uv run lam-daemon`
- Run the CLI: `uv run lam-cli`
- Run the GUI: `uv run lam-gui`

### Documentation

- [How to add support to a new device](docs/device_support.md)
- [Wireshark quick tutorial](https://www.youtube.com/watch?v=zWbdnHwTr3M)
- [Device configuration specs](docs/device_configuration_file_specs.md)
- [Dbus messaging](docs/dbus.md)
