# <img src="./src/linux_arctis_manager/gui/images/app_icon.svg" style="height: 1em; vertical-align: middle;" /> Linux Arctis Manager

An open-source replacement for SteelSeries GG, to manage your Arctis headset on Linux!

[![GitHub Release](https://img.shields.io/github/v/release/elegos/Linux-Arctis-Manager?label=Latest%20Release&color=brightgreen&logo=github&logoColor=white)](https://github.com/elegos/Linux-Arctis-Manager/releases)
[![Release build](https://img.shields.io/github/actions/workflow/status/elegos/Linux-Arctis-Manager/release.yaml?event=push&label=Release%20build&logo=github&logoColor=white)](https://github.com/elegos/Linux-Arctis-Manager/actions/workflows/release.yaml)
[![Python](https://img.shields.io/python/required-version-toml?tomlFilePath=https://raw.githubusercontent.com/elegos/Linux-Arctis-Manager/develop/pyproject.toml&logo=python&logoColor=white&label=Python)](https://www.python.org/)

[![Discord](https://img.shields.io/badge/Discord-join-7289DA?logo=discord&logoColor=white)](https://discord.gg/FXfvUXWXt4)
[![Fluxer](https://img.shields.io/badge/Fluxer-join-5d5cfe?logo=data:image/svg%2bxml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0ibm9uZSIgc3Ryb2tlPSJ3aGl0ZSIgc3Ryb2tlLXdpZHRoPSIzLjIiIHN0cm9rZS1saW5lY2FwPSJyb3VuZCI+PHBhdGggZD0iTTQgOC41YzIuNjYtMi42NiA1LjMzLTIuNjYgOCAwczUuMzMgMi42NiA4IDAiLz48cGF0aCBkPSJNNCAxNS41YzIuNjYtMi42NiA1LjMzLTIuNjYgOCAwczUuMzMgMi42NiA4IDAiLz48L3N2Zz4=)](https://chat.giacomofurlan.name/invite/f8vnlZGp)

## Repositories 💿

### Stable

[![Copr build status](https://copr.fedorainfracloud.org/coprs/elegos/linux-arctis-manager/package/linux-arctis-manager/status_image/last_build.png)](https://copr.fedorainfracloud.org/coprs/elegos/linux-arctis-manager/package/linux-arctis-manager/)
[![AUR Version](https://img.shields.io/aur/version/linux-arctis-manager?label=AUR%20Package&logo=arch-linux&logoColor=white&color=1793d1)](https://aur.archlinux.org/packages/linux-arctis-manager)

### Testing

[![Copr build status (testing)](https://copr.fedorainfracloud.org/coprs/elegos/linux-arctis-manager-testing/package/linux-arctis-manager/status_image/last_build.png)](https://copr.fedorainfracloud.org/coprs/elegos/linux-arctis-manager-testing/package/linux-arctis-manager/)

## Want to support? 🙏

<a href="https://www.paypal.com/donate/?hosted_button_id=W68BN982DMU9U"><img alt="Paypal donation" src="docs/images/paypal-donate-blue.svg" height="72"></a>

## 🎚️ Key Points

- Control ChatMix - enable and control balance between `Media` and `Chat` audio streams
- Configure any device via a simple configuration file
- Enable per-device features by adding them in the relative configuration file
- D-Bus based communication, to support different clients (alternative clients, Plasma extensions, etc)

## 🎧 Supported Devices

Cross-checked against SteelSeries' own device specs, not just the previous v2 list — see [`docs/device_compatibility.md`](docs/device_compatibility.md) for the auto-generated, per-capability, per-PID breakdown of everything below marked ✅.

| Device                                                                      | Supported | Notes                                                                                                                                                                                                                                                                                                                                                      |
| --------------------------------------------------------------------------- | :-------: | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Arctis 1 Wireless (+ Xbox, + Cyberpunk 2077 Edition)                        |     ✅     | Hardware EQ is adjustable, just not firmware-computed — the daemon calculates the filter curve itself and streams it to the AV6X02 DSP chip.                                                                                                                                                                                                               |
| Arctis 5 (+ 2018, Dota 2 Edition, PUBG 2018 Edition)                        |     ✅     | No connection-status or battery readout — the raw protocol reads those over a two-step exchange this project's HID layer doesn't support yet. Mic volume is also not exposed: the vendor spec's own description of that setting is ambiguous.                                                                                                              |
| Arctis 7 (+ 2019 refresh)                                                   |     ✅     | Hardware EQ is adjustable on both revisions, just not firmware-computed — same AV6X02 chip/mechanism as Arctis 1 Wireless.                                                                                                                                                                                                                                 |
| Arctis 7+ / Destiny 2 Edition                                               |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis 7X+ (Xbox)                                                           |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis 7P+ (PlayStation)                                                    |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis 9                                                                    |     ✅     | Hardware EQ isn't adjustable — its register format is entirely undocumented, unlike Arctis 7/1 Wireless/5. Software EQ via PipeWire is the substitute.                                                                                                                                                                                                     |
| Arctis 9X                                                                   |     ❌     | Uses a transport (AVNERA/LIGHTXIO) this project's engine doesn't implement yet.                                                                                                                                                                                                                                                                            |
| Arctis Pro (wired, standalone)                                              |     ✅     | Shares Arctis 5's entire control plane — same chip, same protocol, just a different PID.                                                                                                                                                                                                                                                                   |
| Arctis Pro GameDAC                                                          |     ✅     | RGB lighting, OLED bitmap content, and DTS Headphone:X v2 spatial audio aren't exposed — out of scope, same as every other device. EQ gain units are unconfirmed (no dB-per-unit formula anywhere in the vendor spec). Mic volume and the per-band live EQ push only refresh on reconnect — the DSL's live-update path only reads a single byte per field. |
| Arctis Pro Wireless                                                         |     ✅     | Live updates from the base station's own OLED menu/dial aren't reflected until reconnect — the raw protocol's push-event report isn't documented well enough to wire up safely.                                                                                                                                                                            |
| Arctis Nova 3 (wired)                                                       |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova 3 / 3X Wireless                                                 |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova 4 / 4X                                                          |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova 5 / 5X / 5X White                                               |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova 7 / 7X / WoW / Diablo IV (original firmware)                    |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova 7 / 7X / 7P Gen 2 (native hardware *or* Gen 2 upgrade firmware) |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova 7P (original firmware)                                          |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova Elite (+ SNG SKU)                                               |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova Pro (wired, GameDAC Gen 2, incl. v2 / Xbox)                     |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova Pro Omni                                                        |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis Nova Pro Wireless (+ X, + X White)                                   |     ✅     | —                                                                                                                                                                                                                                                                                                                                                          |
| Arctis GameBuds (+ X)                                                       |     ✅     | Button remapping isn't exposed — no per-gesture-action UI/capability exists in this project yet. The 2.4G/BT parametric EQ has two independent 10-band profiles, both adjustable.                                                                                                                                                                          |
| Arctis GameBuds Case (+ X)                                                  |     ✅     | Battery/charging status and lid-open/closed only — the case has no audio settings of its own.                                                                                                                                                                                                                                                              |

> **Note:** software EQ (10/15-band, applied via PipeWire regardless of what the headset itself supports) is always available as a substitute for hardware EQ. "No hardware EQ" above only means that headset's own onboard curve isn't adjustable — not that EQ itself is unavailable.

### Legend

| Symbol | Description                             |
| :----: | --------------------------------------- |
|   ✅    | Supported by the v3 (Rust) daemon today |
|   ❌    | Not ported yet                          |

## ⌨️ Components

- `lam-daemon`: the background service (Rust) that communicates with your headset, managed by systemd
- `lam-hidraw-helper`: a minimal privileged sidecar that opens `/dev/hidraw*` on the daemon's behalf — the only process that needs elevated capability
- `lam-gui`: the graphical interface (Python/Qt6) to configure your headset and view its status

## 📦 Install

The short version for each distro. Details, signature verification and the other methods are in
**[docs/install.md](docs/install.md)**.

| Your system | Install |
|---|---|
| Arch Linux | `yay -S linux-arctis-manager` |
| Fedora / Bazzite | `sudo dnf copr enable elegos/linux-arctis-manager && sudo dnf install linux-arctis-manager` |
| Debian / Ubuntu / Mint | download the `.deb` for your distro from the [releases page](https://github.com/elegos/Linux-Arctis-Manager/releases), then `sudo apt install ./<file>.deb` |
| SteamOS, or anything else | the user-level installer below (no root, installs into `~/.local`) |

The distro packages ship the services but don't start them; enable them once:

```bash
systemctl --user daemon-reload
systemctl --user enable --now lam-hidraw-helper.service lam-daemon.service
```

### User-level installer

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/elegos/Linux-Arctis-Manager/releases/latest/download/install.sh | sh
```

> [!IMPORTANT]
> **Until the first stable release** that URL returns *404*: `releases/latest` ignores
> pre-releases, and every v3 release so far is one. For now, run the installer from a tag (the
> newest one on the [releases page](https://github.com/elegos/Linux-Arctis-Manager/releases)) and
> add `--pre-release`:
>
> ```bash
> TAG=v3.0.0-beta3   # the newest tag on the releases page
> curl --proto '=https' --tlsv1.2 -fsSL "https://github.com/elegos/Linux-Arctis-Manager/releases/download/$TAG/install.sh" | sh -s -- --pre-release
> ```

Installing again upgrades; it starts the services itself. See
[docs/install.md](docs/install.md#user-level-installer-steamos-and-any-distro) for the options.

### Build it yourself

Fastest is a container, which needs only Podman or Docker and produces a real package for your distro:

```bash
git clone https://github.com/elegos/Linux-Arctis-Manager.git && cd Linux-Arctis-Manager
make container-build-debian-deb      # or -ubuntu-deb, container-build-rpm, container-build-pkg
```

Bare-metal builds, `BASE_IMAGE=` and the rest: [docs/build-from-source.md](docs/build-from-source.md).
To remove it again: [docs/uninstall.md](docs/uninstall.md).

## 🛠️ Development

- Run the daemon: `cargo run --release --manifest-path daemon/Cargo.toml --bin lam-daemon`
- Run the GUI against it: `uv run lam-gui --no-enforce-systemd` (skips the systemd-managed-daemon check, since you're running one by hand)
- Run the daemon's tests: `cargo test --manifest-path daemon/Cargo.toml`

Architecture, the device DSL, the D-Bus interface and the release pipeline are documented in
[docs/](docs/README.md).

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
