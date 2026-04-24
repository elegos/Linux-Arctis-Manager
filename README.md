# Linux Arctis Manager

An open-source replacement for SteelSeries GG, to manage your Arctis headset on Linux!

[![GitHub Release](https://img.shields.io/github/v/release/elegos/Linux-Arctis-Manager?label=Latest%20Release&color=brightgreen&logo=github&logoColor=white)](https://github.com/elegos/Linux-Arctis-Manager/releases)
[![AUR Version](https://img.shields.io/aur/version/linux-arctis-manager?label=AUR%20Package&logo=arch-linux&logoColor=white&color=1793d1)](https://aur.archlinux.org/packages/linux-arctis-manager)
[![Python](https://img.shields.io/python/required-version-toml?tomlFilePath=https://raw.githubusercontent.com/elegos/Linux-Arctis-Manager/develop/pyproject.toml&logo=python&logoColor=white&label=Python)](https://www.python.org/)
[![Build](https://img.shields.io/github/actions/workflow/status/elegos/Linux-Arctis-Manager/wheel-install-test.yaml?branch=develop&label=Build&logo=github&logoColor=white)](https://github.com/elegos/Linux-Arctis-Manager/actions/workflows/wheel-install-test.yaml)
[![Discord](https://img.shields.io/badge/Discord-join-7289DA?logo=discord&logoColor=white)](https://discord.gg/FXfvUXWXt4)
[![Fluxer](https://img.shields.io/badge/Fluxer-join-5d5cfe?logo=data:image/svg%2bxml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSJ3aGl0ZSIgc3Ryb2tlLXdpZHRoPSIzLjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCI+PHBhdGggZD0iTTQgOC41YzIuNjYtMi42NiA1LjMzLTIuNjYgOCAwczUuMzMgMi42NiA4IDAiLz48cGF0aCBkPSJNNCAxNS41YzIuNjYtMi42NiA1LjMzLTIuNjYgOCAwczUuMzMgMi42NiA4IDAiLz48L3N2Zz4=)](https://fluxer.gg/beALFGJK)

## 🎚️ Key Points

- Control ChatMix - enable and control balance between `Media` and `Chat` audio streams
- Configure any device via a simple configuration file
- Enable per-device features by adding them in the relative configuration file
- D-Bus based communication, to support different clients (alternative clients, Plasma extensions, etc)

## 🎧 Supported Devices

| Device | ChatMix | Advanced Features | Product ID(s) |
|---|:---:|:---:|---|
| Arctis 1 / Xbox | ❌ | ❌ | `12b3`, `12b6` |
| Arctis 1 Wireless | ❌ | ❌ | ❓ |
| Arctis 3 Console Edition | ❌ | ❌ | ❓ |
| Arctis 7 / 7 2019 / Pro 2019 / Pro GameDAC | ❌ | ❌ | `1260`, `12ad`, `1252`, `1280` |
| Arctis 9 | ❌ | ❌ | `12c2` |
| Arctis Pro Wireless | ❌ | ❌ | `1290` |
| Arctis Nova 3 | ❌ | ❌ | `12ec` |
| Arctis Nova 3P Wireless / 3X Wireless | ❌ | ❌ | `2269`, `226d` |
| Arctis Nova 5 | ➖ | ✅ | `2232`, `2253` |
| Arctis Nova 7 / 7X / Diablo IV / Gen 2 (% battery) | ✅ | ✅ | `22a1`, `227e`, `2258`, `229e`, `22a9`, `22a5` |
| Arctis Nova 7 / 7X / Diablo IV / Gen 2 (discrete battery) | ✅ | ✅ | `2202`, `2206`, `223a`, `227a`, `22a4` |
| Arctis Nova 7+ / PS5 / Xbox / Destiny | ❌ | ❌ | `220e`, `2212`, `2216`, `2236` |
| Arctis Nova 7P | ❌ | ❌ | `220a` |
| Arctis Nova Elite | ❌ | ❌ | ❓ |
| Arctis Nova Pro Wireless / X | ✅ | ✅ | `12e0`, `12e5` |
| Arctis Nova Pro | ✅ | ✅ | `12cd` (GameDAC Gen2) |

### Legend

| Symbol | Description |
| :---: | --- |
| ✅ | **Supported:** supported and fully implemented |
| ❌ | **Not Implemented:** support not yet available |
| ➖ | **N/A:** not physically supported by this headset model |
| ❓ | **Missing Data:** product ID is not yet known. [Help us find it!](https://github.com/elegos/Linux-Arctis-Manager/blob/develop/docs/device_support.md) |

## ⌨️ CLI Commands

- `lam-daemon`: the background service that communicates with your headset, managed by systemd
- `lam-cli`: command-line utilities for setup tasks like installing udev rules and desktop entries
- `lam-gui`: the graphical interface to configure your headset and view its status

> [!TIP]
> Not sure what a command does? Run it with `-h` or `--help` to see all available options.

## 📦 Install & Setup

Choose the installation method that fits your setup:

- **[Distrobox](#distrobox)** - recommended for immutable distros (Bazzite, Fedora Silverblue, etc.)
- **[Arch Linux (AUR)](#arch-linux-aur)** - community-maintained package for Arch users
- **[Manual install](#manual-install)** - for all other Linux distros

---

### Distrobox

Run the following script to install:

```bash
curl -LsSf https://raw.githubusercontent.com/elegos/Linux-Arctis-Manager/refs/heads/develop/scripts/distrobox.sh | sh
```

> [!NOTE]
> For Immutable Distros (Bazzite, Fedora Silverblue, etc.), the app behaves like a native installation rather than an isolated container. Because Distrobox mounts your `/home`, `/var`, and `/etc` directly, the manager can interact with the system services and configuration files it needs to function.

---

### Arch Linux (AUR)

Arch Linux users can install the community-maintained package from the [Arch User Repository (AUR)](https://aur.archlinux.org/packages/linux-arctis-manager):

Install with your preferred AUR helper:

```bash
yay -S linux-arctis-manager

# using paru: paru -S linux-arctis-manager
```

> For packaging-specific issues, report directly to the AUR maintainers: [@tonitch](https://aur.archlinux.org/account/tonitch) and [@Aiyahhh](https://aur.archlinux.org/account/Aiyahhh).

---

### Manual Install

> [!NOTE]
> `pip` can be used instead of `pipx`, but `pipx` is recommended for better dependency isolation. Some distros will require `pipx`.

#### Prerequisites

Install `pipx` with your package manager.

#### Option A: Install from Release (recommended)

1. Download the latest `.whl` from the [releases page](../../releases)
2. From the directory you downloaded it to, install it:

   ```bash
   pipx install linux_arctis_manager-*.whl

   # using pip: pip install --user linux_arctis_manager-*.whl
   ```

3. Continue to [Final Setup](#final-setup)

#### Option B: Install from Source

1. Install `uv` ([installation guide](https://docs.astral.sh/uv/getting-started/installation/)) and create the applications directory:

   ```bash
   mkdir -p $HOME/.local/share/applications
   ```

2. Get the source:

   ```bash
   git clone https://github.com/elegos/Linux-Arctis-Manager.git
   cd Linux-Arctis-Manager
   git pull
   ```

3. Build:

   ```bash
   rm -rf dist
   uv build
   ```

4. Install:

   ```bash
   find ./dist -name "*.whl" | head -n1 | xargs pipx install --force

   # using pip: find ./dist -name "*.whl" | head -n1 | xargs pip install --user --force-reinstall
   ```

#### Final Setup

```bash
lam-cli setup --start-now
```

> [!TIP]
> To launch the system tray icon automatically on login:
>
> ```bash
> lam-cli setup --systray-autostart
> ```
>
> You can also setup everything at once in one line:
>
> ```bash
> lam-cli setup --systray-autostart --start-now
> ```

## 🧹 Uninstall / Cleanup

1. Stop and disable the service:

   ```bash
   systemctl --user disable --now arctis-manager
   rm ~/.config/systemd/user/arctis-manager.service
   ```

2. Remove leftover files:

   ```bash
   # desktop menu entries
   lam-cli desktop remove

   # udev rules
   sudo rm -f /etc/udev/rules.d/91-steelseries-arctis.rules
   sudo rm -f /usr/lib/udev/rules.d/91-steelseries-arctis.rules

   # user preferences and device/lang files
   rm -rf ~/.config/arctis_manager
   ```

3. Uninstall the package:

   ```bash
   pipx uninstall linux_arctis_manager

   # using pip: pip uninstall linux_arctis_manager
   ```

## 🛠️ Development

### Basic Commands

- Run the daemon: `uv run lam-daemon`
- Run the CLI: `uv run lam-cli`
- Run the GUI: `uv run lam-gui [--no-enforce-systemd]` (use this option to avoid force enabling the daemon, in case you're working on it)

### Documentation

- [How to add support for a new device](docs/device_support.md)
- [Wireshark tutorial](https://www.youtube.com/watch?v=zWbdnHwTr3M)
- [Device configuration specs](docs/device_configuration_file_specs.md)
- [Dbus messaging](docs/dbus.md)

## ⚠️ Troubleshooting

- App or headset becomes unresponsive: `systemctl --user restart --now arctis-manager`
- Newly supported device does not appear after an update: `lam-cli setup`
- App fails to start with a Qt xcb platform error: install `libxcb-cursor0` (Debian/Ubuntu) or `xcb-util-cursor` (Arch/Fedora). Required on non-Qt desktop environments like Cinnamon.

## 💬 Community & Support

Linux Arctis Manager is a community-driven project - the more hardware data and feedback we get, the better support becomes for everyone.

Join us on:

- [Discord](https://discord.gg/FXfvUXWXt4)
- [Fluxer](https://fluxer.gg/beALFGJK)

### Missing a Device?

If your headset isn't listed in the support table, we likely just need your hardware IDs to get started. See our [Hardware Support Guide](docs/device_support.md) for instructions on how to find and submit your Product ID (PID).

---

Linux Arctis Manager is licensed under the [GPL-3.0](LICENSE) and is not affiliated with or endorsed by [SteelSeries ApS](https://steelseries.com). SteelSeries, Arctis, ChatMix, and SteelSeries GG are trademarks of their respective owners.
