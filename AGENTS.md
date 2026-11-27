# AGENTS.md

Guide for coding agents (Claude Code, Codex, Cursor, ...) working in this
repository. There are usually two jobs: **helping someone get their headset
working**, or **changing the project** (new device, bug fix). Both start
the same way: find out what's actually wrong before changing anything.

## Repository map

| Path | What it is |
|---|---|
| `daemon/engine/` | `lam-daemon`, the Rust daemon (systemd user service): device sessions, hotplug, D-Bus API, audio (virtual sinks, EQ, mic routing) |
| `daemon/hidraw-helper/` | `lam-hidraw-helper`, a tiny sidecar with `cap_dac_override`, the only process allowed to open `/dev/hidraw*` |
| `daemon/device-config/`, `daemon/hid-transport/` | YAML DSL interpreter and HID transport crates |
| `daemon/device-configs/` | **Device support lives here**: one YAML per device family, extending a `base_*.yaml` |
| `src/linux_arctis_manager/` | Python/Qt GUI (`lam-gui`) and dev-only CLI (`uv run lam-cli`) |
| `packaging/` | systemd units, distro packages, Plasma widget, GNOME extension, WirePlumber script |
| `scripts/` | `install.sh` (user-level installer), `doctor.sh` (health check), `collect-debug-info.sh` (support bundle) |
| `docs/` | `ARCHITECTURE.md`, `DEVICE_DSL.md`, `dbus.md`, `migration-v2-to-v3.md`, `v3-backlog.md` |

`src/linux_arctis_manager/devices/*.yaml` are v2-format files left from the
old Python engine; the daemon doesn't read them. Don't add devices there.

## Helping someone get their headset working

1. **Run `scripts/doctor.sh`** as the user, with the headset plugged in and
   powered on. It's read-only. Without a checkout:
   `curl -fsSLO https://raw.githubusercontent.com/elegos/Linux-Arctis-Manager/develop/scripts/doctor.sh && bash doctor.sh`
2. Work through the `FAIL` lines **top to bottom**; each comes with a `fix:`
   line. Later failures are often caused by earlier ones. Re-run after each fix.
3. Usual causes, in rough order of frequency:
   - Not installed, or only half installed: follow the README's
     *Install & Setup* for their distro.
   - `lam-hidraw-helper` lacks `cap_dac_override` (it's lost whenever the
     binary is rebuilt or copied): `sudo setcap cap_dac_override+eip <helper path>`.
   - An old v1/v2 `arctis-manager` daemon still running and holding the
     device: `docs/migration-v2-to-v3.md`.
   - `Arctis_Media`/`Arctis_Chat` missing: the headset itself is off (the
     dongle alone isn't enough), or the daemon failed; check its log.
   - Device not supported: see *Adding or fixing a device* below.
4. Still stuck after doctor passes:
   `journalctl --user -u lam-daemon.service -f` while reproducing, then
   `scripts/collect-debug-info.sh` for a bundle to attach to an issue.

**Ask the user first** before running anything with `sudo`, installing
packages, deleting udev rules or config files, or disabling services. Show
them the exact command.

v3 needs **no udev rules**. Don't create any; if old ones exist, they're
leftovers (doctor.sh lists them).

## Adding or fixing a device

1. Identify it: `lsusb -d 1038:` gives the product ID; `uv run lam-cli tools
   arctis-devices` (from a checkout) lists HID interfaces and endpoints.
   Check `docs/device_compatibility.md` first: the PID may already be
   covered under a family name (e.g. "Nova 7 (Gen 2 protocol)").
2. Most new devices are a variant of an existing family: add the PID to that
   family's device file in `daemon/device-configs/`. A new family is a new
   device file extending a `base_*.yaml`. The format is in `docs/DEVICE_DSL.md`.
3. To see what a device sends (dial turns, mute button, battery), capture
   every HID interface while the user presses buttons:
   `scripts/collect-debug-info.sh --hidraw-capture=40`. Many headsets push
   these reports unprompted, so no Windows or SteelSeries GG is needed for
   that part.
4. Pick up config changes without reinstalling: call the `ReloadConfigs`
   D-Bus method, or `systemctl --user restart lam-daemon.service`.
5. Regenerate the compatibility matrix (CI fails if it drifts):
   `cd daemon && cargo run -p device-config --bin lam-gen-device-matrix`

**Never send experimental commands to a real headset** without the user's
explicit go-ahead, and never call firmware-update APIs while experimenting.
A device in firmware-update mode re-enumerates with its `bootloader_pid`.
Passive captures (reading) are safe.

## Development commands (same as CI)

```bash
# Python (GUI/CLI)
uv sync --group dev
uv run ruff check src tests
uv run basedpyright
uv run pytest

# Rust (needs libudev, hidapi and libpulse dev packages)
cd daemon
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all

# Shell (the files CI lints, see .github/workflows/install-test.yaml)
shellcheck --severity=warning scripts/install.sh scripts/build-release-tarball.sh \
  tests/installer/dry-run.sh scripts/doctor.sh tests/doctor/run.sh
tests/doctor/run.sh
```

Running locally: `cargo run --release --manifest-path daemon/Cargo.toml --bin lam-daemon`,
then `uv run lam-gui --no-enforce-systemd`. Stop the installed
`lam-daemon.service` first, since two daemons will fight over the device.

## Conventions

- User-visible changes get a line under `## [Unreleased]` in `CHANGELOG.md`
  (Keep a Changelog format).
- Don't commit generated files: the systemd units built from `*.service.in`,
  `packaging/scripts/lam-gui`, the Plasma widget's `contents/code/lang/`
  (see `.gitignore`). `docs/device_compatibility.md` is generated, but it
  *is* committed. Regenerate it rather than editing by hand.
- `docs/ARCHITECTURE.md` explains the privilege model (why a setcap helper,
  not udev). Read it before touching device access.
