# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- AI Voice Changer (RVC): real-time voice conversion to a chosen model, with a guided calibration wizard (record a short sample, pick pitch and dynamics by ear, refine as needed), per-model tuning with auto-tune, and FAISS feature retrieval for more stable output on out-of-distribution input.
- Voice model management: search and download models from HuggingFace, or use local files. Base AI models and any missing AI dependencies (PyTorch/ONNX/cuDNN) are downloaded/installed automatically, checksum-verified, and always with explicit consent before anything is installed.
- Noise Cancellation panel: five presets (Off, Light, Standard, Studio, Custom) plus full manual gate/compressor control, with an input device selector.
- Hardware EQ support for devices with onboard EQ (Nova Pro family), with a per-channel backend selector (Auto / Software / Hardware).
- Per-application EQ and Noise Cancellation overrides: route a specific app, executable, or Steam game to a chosen preset automatically, including switching by focused window on Hyprland, Sway, and X11.
- EQ presets (10-band and 15-band), saved as YAML, with a curve editor and per-band reset to 0 dB.
- Audio output can be redirected automatically on headset connect/disconnect (General settings).
- Device settings now persist across daemon restarts.
- The daemon now recognises a device that re-enumerates in firmware update mode (bootloader PID) instead of treating it as an unknown device: no init sequence is run and no settings are exposed while it's in that state, and a new D-Bus signal (`DeviceFirmwareUpdateMode`) lets the GUI warn against unplugging it. Not present in v2.
- GUI shows both the UI and daemon versions and upgrades the background service automatically on a version mismatch; starts the service itself if it isn't already running.

### Changed

- Voice Changer's Enable control is now a tri-state switch (off / on for this session / on, starts automatically with the daemon) instead of a plain checkbox that silently reset to inactive on every daemon restart while still showing "on."
- The background daemon has been rewritten from Python to Rust; the GUI is unchanged.
- EQ changes apply live to the running audio graph instead of reloading it, eliminating playback interruptions when adjusting gains or switching presets.
- Noise Cancellation now runs as a single audio device instead of one per effect stage.
- Device detection now matches primarily by product name (falling back to product ID), so a firmware update that changes the PID is still recognised automatically.
- Side navigation now uses icon buttons instead of text labels.
- The Voice Changer panel is labelled "(Preview)" — usable, not yet considered production quality.

### Fixed

- The GUI's automatic service restart on a version mismatch now restarts the actual installed `lam-daemon`/`lam-hidraw-helper` systemd units instead of authoring and restarting a stale, differently-named unit left over from v2.
- Noise Cancellation no longer swallows quiet consonants (nasals, word-final sounds).
- EQ preset/gain changes no longer interrupt other apps' playback or leak PipeWire modules over a session.
- Settings with many options no longer overflow the window (now a dropdown instead of a button row); sliders no longer stutter while dragging.

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
