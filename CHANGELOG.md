# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [3.0.0]

### Added

- AI Voice Changer (RVC): real-time voice conversion with a guided calibration wizard, model management (HuggingFace search/download or local files), and auto-tune.
- Noise Cancellation panel with presets and full manual control.
- Hardware EQ support for the Nova Pro family, with a per-channel backend selector (Auto / Software / Hardware).
- Per-application EQ and Noise Cancellation overrides, including switching by focused window on Hyprland, Sway, and X11.
- EQ presets (10-band and 15-band) with a curve editor.
- Audio output can be redirected automatically on headset connect/disconnect.
- Device settings now persist across daemon restarts.
- GUI shows the daemon version and upgrades/starts the background service automatically.
- Rust daemon (v3): rewrite of the background service, with support for the entire device lineup (Nova 5, Arctis 7+, Nova Pro Wired/Wireless/Omni, Nova Elite, Nova 7, Nova 7 Gen2, Nova 3/3X/4/4X, Arctis 7, Arctis 1 Wireless, Arctis 5, Arctis 9, Arctis Pro/Pro Wireless/Pro GameDAC, Arctis GameBuds) — see `docs/device_compatibility.md` for the full capability matrix.
- KDE Plasma 6 widget (plasmoid): status and a configurable set of quick-access controls in the Plasma panel, positioned by Plasma itself, plus a button to open the main app window. Ships as its own `linux-arctis-manager-plasma-widget` package (Fedora subpackage, Debian binary package, Arch split package) across the same build matrix as the main package.
- GNOME Shell extension (45+): same status + configurable quick settings + open-main-app button, in the top panel. Ships as its own `linux-arctis-manager-gnome-extension` package, same build matrix as the Plasma widget.

### Changed

- Voice Changer's Enable control is now a tri-state switch (off / on for this session / on, starts with the daemon).
- The background daemon has been rewritten from Python to Rust; the GUI is unchanged.
- EQ changes apply live to the running audio graph instead of reloading it.
- Noise Cancellation now runs as a single audio device instead of one per effect stage.
- Device detection now matches primarily by product name, falling back to product ID.
- Side navigation now uses icon buttons instead of text labels.
- The Voice Changer panel is labelled "(Preview)".
- README's Supported Devices table rewritten and re-checked against SteelSeries' own device specs.
- CI now enforces Python code quality (`ruff`, `basedpyright`, coverage floor) alongside the existing Rust checks (`cargo-deny` added).

### Removed

- The old Python daemon and everything only reachable from it, now that the Rust daemon covers its functionality. `lam-cli` is trimmed to its device-introspection subcommand.

### Fixed

- Byte-order mismatch that corrupted custom EQ gain on Nova Pro Wireless, Nova Pro Wired, Arctis 7+, and Nova 7.
- GUI's automatic service restart now targets the actual installed v3 systemd units.
- Noise Cancellation no longer swallows quiet consonants.
- `SetNCSettings` no longer force-persists the preset as "off": Noise Cancellation has no autostart/session-only tri-state like Voice Changer — any preset other than "off" is always active and now correctly reported back over D-Bus (was previously always reported as "off", regardless of the selected preset, and lost on daemon restart).
- EQ preset/gain changes no longer interrupt other apps' playback or leak PipeWire modules.
- Settings with many options no longer overflow the window; sliders no longer stutter while dragging.
- `lam-hidraw-helper` and the daemon's HTTP user agent now report the real project version instead of the crate-internal `0.1.0`/`CARGO_PKG_VERSION` placeholder.
- `pyproject.toml`'s version is now synced from the shared `VERSION` file before packaging (`make sync-version`), so a stale `pyproject.toml` can no longer ship a mismatched GUI version.

## [2.4.1]

## Added
- Support for Arctis Nova Pro Wireless (225d)
- Added install script by @HelpfulSoft1207

## [2.4.0]

## Added
- Support for Nova Pro Wired - @HelpfulSoft1207
- Support for Nova 7 Plus - @debbiedi
- Support for Nova 5X (variant 2264)

## Fixed

- After USB error teardown, the daemon now actively re-detects the device instead of relying solely on systemd to restart it (fixes [#23](https://github.com/elegos/Linux-Arctis-Manager/issues/23))
- `CoreEngine.device_settings` is no longer uninitialized when no recognized device is connected; `lam-gui` no longer crashes with `AttributeError` in that scenario (fixes [#27](https://github.com/elegos/Linux-Arctis-Manager/issues/27))
- Systray app's name set to "Arctis Manager", instead of the anonymous "lam-gui"

## [2.3.1]

## Added

- Single-instance enforcement for `lam-daemon` via PID file in `XDG_RUNTIME_DIR`
- `--replace` flag for `lam-daemon` to stop running instance and start a new one

## Fixed

- USB I/O errors (errno 5/32) after system suspend/resume no longer cause infinite log spam and 100% CPU usage; the daemon now tears down the stale USB handle and waits for the device to re-enumerate, exiting cleanly for systemd to restart if recovery fails (fixes [#23](https://github.com/elegos/Linux-Arctis-Manager/issues/23))

## [2.3.0]

## Added

- `discrete_map` setting type
- `lam-cli setup` all-in-one setup script

## Changed

- Updated `uv` and relative build tools to version 0.10.11
- Updated `slider` configurations to `descrete_map` ones where a mapping was set

## [2.2.1]

# Fixed

- Proper udev file content generation

## [2.2.0]

## Added

- Support for devices communicating on control endpoint (0x00)
- Support for Arctis Nova 7 family (thanks villain @ Discord!)
- Support for Arctis Nova 5 family (thanks @nrwlia!)
- `StatusChanged` and `SettingsChanged` Dbus signals (subscription model instead of polling one)

## Changed

- GUI now subscribes to Dbus signals instead of continuously poll the Dbus interfaces

## Fixed

- Re-initialize device on system wake up (after sleep)
- Ensure applications directory exists before creating the desktop entry
- Proper USB device claim
- Fix an issue incorrectly initializing the TOGGLE UI widget

## [2.1.0] - 4 March 2026

### Fixed

- Initialize device on awake after sleep
