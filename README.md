# Linux Arctis Manager

An open-source replacement for SteelSeries GG, to manage your Arctis headset on Linux!

[![GitHub Release](https://img.shields.io/github/v/release/elegos/Linux-Arctis-Manager?label=Latest%20Release&color=brightgreen&logo=github&logoColor=white)](https://github.com/elegos/Linux-Arctis-Manager/releases)
[![AUR Version](https://img.shields.io/aur/version/linux-arctis-manager?label=AUR%20Package&logo=arch-linux&logoColor=white&color=1793d1)](https://aur.archlinux.org/packages/linux-arctis-manager)
[![Python](https://img.shields.io/python/required-version-toml?tomlFilePath=https://raw.githubusercontent.com/elegos/Linux-Arctis-Manager/develop/pyproject.toml&logo=python&logoColor=white&label=Python)](https://www.python.org/)
[![Build](https://img.shields.io/github/actions/workflow/status/elegos/Linux-Arctis-Manager/install-test.yaml?branch=develop&label=Build&logo=github&logoColor=white)](https://github.com/elegos/Linux-Arctis-Manager/actions/workflows/install-test.yaml)
[![Discord](https://img.shields.io/badge/Discord-join-7289DA?logo=discord&logoColor=white)](https://discord.gg/FXfvUXWXt4)
[![Fluxer](https://img.shields.io/badge/Fluxer-join-5d5cfe?logo=data:image/svg%2bxml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSJ3aGl0ZSIgc3Ryb2tlLXdpZHRoPSIzLjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCI+PHBhdGggZD0iTTQgOC41YzIuNjYtMi42NiA1LjMzLTIuNjYgOCAwczUuMzMgMi42NiA4IDAiLz48cGF0aCBkPSJNNCAxNS41YzIuNjYtMi42NiA1LjMzLTIuNjYgOCAwczUuMzMgMi42NiA4IDAiLz48L3N2Zz4=)](https://fluxer.gg/beALFGJK)

## 🎚️ Key Points

- Control ChatMix - enable and control balance between `Media` and `Chat` audio streams
- Configure any device via a simple configuration file
- Enable per-device features by adding them in the relative configuration file
- D-Bus based communication, to support different clients (alternative clients, Plasma extensions, etc)

## 🎧 Supported Devices

Cross-checked against SteelSeries' own device specs, not just the previous v2 list — see [`docs/device_compatibility.md`](docs/device_compatibility.md) for the auto-generated, per-capability, per-PID breakdown of everything below marked ✅.

| Device | Supported | Notes |
|---|:---:|---|
| Arctis 1 Wireless (+ Xbox, + Cyberpunk 2077 Edition) | ✅ | Hardware EQ is adjustable, just not firmware-computed — the daemon calculates the filter curve itself and streams it to the AV6X02 DSP chip. |
| Arctis 5 (+ 2018, Dota 2 Edition, PUBG 2018 Edition) | ✅ | No connection-status or battery readout — the raw protocol reads those over a two-step exchange this project's HID layer doesn't support yet. Mic volume is also not exposed: the vendor spec's own description of that setting is ambiguous. |
| Arctis 7 (+ 2019 refresh) | ✅ | Hardware EQ is adjustable on both revisions, just not firmware-computed — same AV6X02 chip/mechanism as Arctis 1 Wireless. |
| Arctis 7+ / Destiny 2 Edition | ✅ | — |
| Arctis 7X+ (Xbox) | ✅ | — |
| Arctis 7P+ (PlayStation) | ✅ | — |
| Arctis 9 | ✅ | Hardware EQ isn't adjustable — its register format is entirely undocumented, unlike Arctis 7/1 Wireless/5. Software EQ via PipeWire is the substitute. |
| Arctis 9X | ❌ | Uses a transport (AVNERA/LIGHTXIO) this project's engine doesn't implement yet. |
| Arctis Pro (wired, standalone) | ✅ | Shares Arctis 5's entire control plane — same chip, same protocol, just a different PID. |
| Arctis Pro GameDAC | ✅ | RGB lighting, OLED bitmap content, and DTS Headphone:X v2 spatial audio aren't exposed — out of scope, same as every other device. EQ gain units are unconfirmed (no dB-per-unit formula anywhere in the vendor spec). Mic volume and the per-band live EQ push only refresh on reconnect — the DSL's live-update path only reads a single byte per field. |
| Arctis Pro Wireless | ✅ | Live updates from the base station's own OLED menu/dial aren't reflected until reconnect — the raw protocol's push-event report isn't documented well enough to wire up safely. |
| Arctis Nova 3 (wired) | ✅ | — |
| Arctis Nova 3 / 3X Wireless | ✅ | — |
| Arctis Nova 4 / 4X | ✅ | — |
| Arctis Nova 5 / 5X / 5X White | ✅ | — |
| Arctis Nova 7 / 7X / WoW / Diablo IV (original firmware) | ✅ | — |
| Arctis Nova 7 / 7X / 7P Gen 2 (native hardware *or* Gen 2 upgrade firmware) | ✅ | — |
| Arctis Nova 7P (original firmware) | ✅ | — |
| Arctis Nova Elite (+ SNG SKU) | ✅ | — |
| Arctis Nova Pro (wired, GameDAC Gen 2, incl. v2 / Xbox) | ✅ | — |
| Arctis Nova Pro Omni | ✅ | — |
| Arctis Nova Pro Wireless (+ X, + X White) | ✅ | — |
| Arctis GameBuds (+ X) | ✅ | Button remapping isn't exposed — no per-gesture-action UI/capability exists in this project yet. The 2.4G/BT parametric EQ has two independent 10-band profiles, both adjustable. |
| Arctis GameBuds Case (+ X) | ✅ | Battery/charging status and lid-open/closed only — the case has no audio settings of its own. |

> **Note:** software EQ (10/15-band, applied via PipeWire regardless of what the headset itself supports) is always available as a substitute for hardware EQ. "No hardware EQ" above only means that headset's own onboard curve isn't adjustable — not that EQ itself is unavailable.

### Legend

| Symbol | Description |
| :---: | --- |
| ✅ | Supported by the v3 (Rust) daemon today |
| ❌ | Not ported yet |

## ⌨️ Components

- `lam-daemon`: the background service (Rust) that communicates with your headset, managed by systemd
- `lam-hidraw-helper`: a minimal privileged sidecar that opens `/dev/hidraw*` on the daemon's behalf — the only process that needs elevated capability
- `lam-gui`: the graphical interface (Python/Qt6) to configure your headset and view its status

## 📦 Install & Setup

Choose the installation method that fits your setup:

- **[Arch Linux (AUR)](#arch-linux-aur)** - community-maintained package for Arch users
- **[Build from source](#build-from-source)** - for all other Linux distros (Fedora, Bazzite, Debian, Ubuntu, ...)

---

### Arch Linux (AUR)

Arch Linux users can install the community-maintained package from the [Arch User Repository (AUR)](https://aur.archlinux.org/packages/linux-arctis-manager):

Install with your preferred AUR helper:

```bash
yay -S linux-arctis-manager

# using paru: paru -S linux-arctis-manager
```

The package's post-install hook applies the required capability to
`lam-hidraw-helper` and prints the two commands to enable the services:

```bash
systemctl --user daemon-reload
systemctl --user enable --now lam-hidraw-helper.service lam-daemon.service
```

> [!TIP]
> To launch the system tray app automatically on login:
>
> ```bash
> ln -sf /usr/share/applications/ArctisManagerSystray.desktop ~/.config/autostart/
> ```

> For packaging-specific issues, report directly to the AUR maintainers: [@tonitch](https://aur.archlinux.org/account/tonitch) and [@Aiyahhh](https://aur.archlinux.org/account/Aiyahhh).

---

### Build from source

Prebuilt RPM/deb packages aren't published yet (Fedora/Bazzite users can build
the `.spec` in `packaging/fedora/` locally with `make container-build-rpm`,
which produces a `.rpm` under `dist/`). Until then, building straight from
source with the provided `Makefile` is the supported path everywhere except
Arch.

#### Prerequisites

- [`cargo`/`rustc`](https://rustup.rs/) (stable toolchain)
- [`uv`](https://docs.astral.sh/uv/getting-started/installation/)
- Python 3.10+
- `libcap` (for `setcap`) — usually already installed
- Kernel headers for `hidraw`/`udev`: `libudev-dev` (Debian/Ubuntu), `systemd-devel` (Fedora), `udev` (Arch, already present)

#### Install

```bash
git clone https://github.com/elegos/Linux-Arctis-Manager.git
cd Linux-Arctis-Manager

make build            # cargo build --release + uv sync
sudo make install     # installs binaries, systemd units, desktop entries; applies setcap
make enable           # enable + start lam-hidraw-helper and lam-daemon (no sudo)
```

`make install` accepts the usual `PREFIX`/`DESTDIR` overrides — see `make help`
for the full variable list, or the packaging recipes in `packaging/arch/PKGBUILD`
and `packaging/fedora/linux-arctis-manager.spec` for reference.

> [!TIP]
> To launch the system tray app automatically on login:
>
> ```bash
> ln -sf /usr/share/applications/ArctisManagerSystray.desktop ~/.config/autostart/
> # or, if installed with PREFIX=/usr/local:
> ln -sf /usr/local/share/applications/ArctisManagerSystray.desktop ~/.config/autostart/
> ```

## 🧹 Uninstall / Cleanup

Choose the method that matches your installation method:

- **[Arch Linux (AUR)](#arch-linux-aur-1)**
- **[Build from source](#build-from-source-1)**

### Arch Linux (AUR)
Use the system package manager — the pre-removal hook stops and disables the services for you:

```bash
sudo pacman -Rns linux-arctis-manager
```

### Build from source

From the cloned repository:

```bash
make disable          # stop + disable the services (no sudo)
sudo make uninstall   # remove binaries, systemd units, desktop entries
```

Then remove your local settings if you don't intend to reinstall:

```bash
rm -rf ~/.config/arctis_manager
```

> [!NOTE]
> If you're coming from a v2 install, also remove the old udev rule — v3 doesn't need one:
> `sudo rm -f /etc/udev/rules.d/91-steelseries-arctis.rules /usr/lib/udev/rules.d/91-steelseries-arctis.rules`

## 🛠️ Development

### Basic Commands

- Run the daemon: `cargo run --release --manifest-path daemon/Cargo.toml --bin lam-daemon`
- Run the GUI against it: `uv run lam-gui --no-enforce-systemd` (skips the systemd-managed-daemon check, since you're running one by hand)
- Run the daemon's tests: `cargo test --manifest-path daemon/Cargo.toml`

### Documentation

- [Architecture overview](docs/ARCHITECTURE.md)
- [Device DSL reference](docs/DEVICE_DSL.md) — the YAML format used to describe a device
- [D-Bus interface reference](docs/dbus.md)
- [Device compatibility matrix](docs/device_compatibility.md) — auto-generated from the v3 device configs; the table above is the human-curated summary, this is the exhaustive per-capability one
- [Equalizer](docs/eq.md) — band modes, backends, presets, per-app overrides
- [Voice Changer](docs/voice-changing-feature.md) — usage and architecture
- [Wireshark tutorial](https://www.youtube.com/watch?v=zWbdnHwTr3M)
- [Migrating from v2 to v3](docs/migration-v2-to-v3.md)

## ⚠️ Troubleshooting

- App or headset becomes unresponsive: `systemctl --user restart lam-hidraw-helper.service lam-daemon.service`
- Newly supported device does not appear after an update: `systemctl --user daemon-reload && systemctl --user restart lam-daemon.service`, or call the `ReloadConfigs` D-Bus method to pick up new/changed device YAML files without a restart.
- App fails to start with a Qt xcb platform error: install `libxcb-cursor0` (Debian/Ubuntu) or `xcb-util-cursor` (Arch/Fedora). Required on non-Qt desktop environments like Cinnamon.

## 💬 Community & Support

Linux Arctis Manager is a community-driven project - the more hardware data and feedback we get, the better support becomes for everyone.

Join us on:

- [Discord](https://discord.gg/FXfvUXWXt4)
- [Fluxer](https://fluxer.gg/beALFGJK)

### Missing a Device?

If your headset isn't listed in the support table, we likely just need your hardware IDs to get started. Run `lam-cli tools arctis-devices` and share the output on [Discord](https://discord.gg/FXfvUXWXt4) or [Fluxer](https://fluxer.gg/beALFGJK).

---

Linux Arctis Manager is licensed under the [GPL-3.0](LICENSE) and is not affiliated with or endorsed by [SteelSeries ApS](https://steelseries.com). SteelSeries, Arctis, ChatMix, and SteelSeries GG are trademarks of their respective owners.
