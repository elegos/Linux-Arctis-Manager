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
- Rust daemon (v3): Arctis Nova 5 Wireless device support (Nova 5, Nova 5X, Nova 5X white) — mic volume, sidetone, volume limiter, mic-mute LED brightness, wireless mode (speed/range), inactivity timer, battery/connection/charging sync, hardware ChatMix, and now also the 10-band parametric EQ (frequency/filter-type/gain/Q-factor per band, RF and Bluetooth domains).
- Rust daemon (v3): generalised the Arctis Nova 7 Gen2 family's parametric EQ write builtins (previously hardcoded to that device's constants) into a shared, parameterised primitive, then used it to ship Nova 5's own parametric EQ — a 2-message write (no commit step, unlike Gen2's 3-message one) with a different gain byte encoding. Also generalised the fixed-frequency graphic EQ builtins (Nova Pro family / Arctis 7+) the same way. No behavior change for already-shipped devices; removes the Rust-side cost of porting future Nova-family EQ devices (only new device YAML will be needed).
- Rust daemon (v3): Arctis 7+ family device support, split into three device files by real capability tier rather than v2's single flat file — standard/Destiny 2 (chatmix + sidetone + 10-band EQ), 7X/Xbox (no chatmix), 7P/PlayStation (no chatmix, no sidetone). Also adds mic volume, the 10-band graphic EQ, and mic-mute LED brightness, none of which v2 exposed for this family.
- Rust daemon (v3): Arctis Nova Pro (wired) device support — standard, v2 (firmware-revision successor, not covered by v2), and Xbox PIDs in one device file, reusing Nova Pro Wireless's EQ/high-gain/dim-timer logic as-is (no new code). Corrects two v2 config mistakes: `line_out_mode`'s values are "chatmix"/"stream" per the vendor spec, not "Speaker"/"Stream"; the physical volume knob has no live push in the spec, so it's read at connect time only.
- Rust daemon (v3): Arctis Nova Elite device support (standard + "sng" SKU) — ANC (off/transparent/on, each with its own level), independent boom-mic and on-ear-mic sidetone, mic volume/mute-LED brightness, Bluetooth defaults, station display (home screen, screensaver, brightness), battery/connection/charging sync, hardware ChatMix, and a full startup snapshot for mic volume/sidetone/line-out/stream-mix/chatmix via a 1036-byte HID feature-report read. EQ (all domains) is not supported on this device yet — it has no simple preset-select command at all, presets are named slots written as a whole curve — tracked as a follow-up story, not shipped in this pass.
- Fixed several Rust daemon (v3) device configs (Nova 5, Arctis 7+, Nova Elite) where a live status update and the equivalent value read at startup used different JSON field names for the same setting, which would have produced two differently-shaped D-Bus events for what GUI clients expect to be one signal. Added a regression test that checks this automatically for every device config going forward.
- Rust daemon (v3): Arctis Nova 7 family device support (gen1 protocol) — standard, WoW Edition, Diablo IV Edition, 7X, and 7X white in one full-capability device file (chatmix + sidetone + mic volume + 10-band graphic EQ + mute-LED brightness), 7P in a second file without sidetone (its raw spec has no sidetone struct at all, unlike Arctis 7+'s PlayStation tier it does keep hardware chatmix). Reuses the Nova Pro family's EQ gain formula as-is.
- Rust daemon (v3): Arctis Nova 7 Gen2 device support, covering both real Gen 2 hardware and gen1 units running the "Gen 2" upgrade firmware (9 PIDs, one device file — same protocol on both). Adds percentage battery, live mic-mute and Bluetooth-link status, and a 3-domain (2.4GHz/Bluetooth/mic) 10-band parametric EQ writable as a 3-message sequence — the first device needing more than one physical HID write per setting change, which motivated three new engine primitives: a device-config write can now be an ordered sequence of steps (each its own HID write or a timed pause) instead of one message, and a new `varstring` field type for variable-length values like an EQ preset name.
- Rust daemon (v3): also fixed while building the above — Nova Elite never had a `save_to_flash` write at all (settings never survived a power cycle), and its shutdown sequence called two lifecycle hooks (`disable_chatmix`, `disable_sonar`) whose backing structs had never been added, so both would have failed at runtime. All three now present.
- Rust daemon (v3): Arctis Nova Pro Wireless X White (`0x225D`) added as a third variant — protocol-identical to the existing two, was simply never added.
- Rust daemon (v3): Arctis Nova Pro Omni device support — same underlying hardware/protocol as Nova Elite (confirmed from its own raw spec), reusing ANC, Bluetooth, sidetone, station display, and battery/connection sync unchanged. Adds mic noise reduction (not on Nova Elite at all); has no on-ear mic, unlike Nova Elite. EQ is not supported on this device yet, same as Nova Elite — tracked as a follow-up story, not shipped in this pass.
- Rust daemon (v3): a new `TriggerAction` D-Bus method for one-shot, fire-and-forget device commands (RF/BT pairing, factory reset) that have no persisted value to remember or restore, unlike every other setting `SetSetting` handles — wired to Arctis 7+/7x+/7p+'s `pairing_mode` and Nova Elite/Nova Pro Omni/Nova 7 Gen2's `restore_factory_default`, all previously declared in their device YAML but unreachable from D-Bus. GUI wiring (a "Pair"/"Factory reset" button) not included in this pass.
- Rust daemon (v3): a new signed-integer field type (`int8`/`int16`/`int32`) in the device-config DSL, closing the one gap left in Arctis Nova 7 Gen2's parametric EQ ([E7-S11]) — its readback wire format encodes gain as a plain signed byte, which every field type until now decoded wrong. Used to add `read_eq_preset_name`/`get_eq_preset_data` (RF/BT/mic domains) to that device, matching the write side already shipped. Like every other shipped EQ device's readback, it's declared for structural completeness but not wired into live sync — the daemon always resets EQ to a known state on init rather than restoring the hardware's persisted curve.
- Rust daemon (v3): Nova Elite and Nova Pro Omni's EQ (wireless parametric, mic/Bluetooth gain-only), which unlike every other device writes a whole named preset slot — curve plus a short alias and free-text name — in one message instead of selecting a preset by id. Reading a slot back isn't modelled yet, same convention as every other shipped EQ device. Also fixed a related device-config DSL gap found while shipping it: a fixed-width string field (`varstring` with a `size` cap) is now zero-padded to that size on write instead of just capped, needed for devices where such a field isn't the last thing in its message; wire-equivalent for every already-shipped device, since the padding used to come from the transport's own chunk-size fill instead.
- Rust daemon (v3): a new `docs/device_compatibility.md`, generated straight from every device config's own `capabilities:` list (`cargo run -p device-config --bin lam-gen-device-matrix`) instead of hand-maintained, with CI (`--check` mode) failing the build if it drifts from the YAML. Found and fixed three device configs whose `capabilities:` list hadn't been updated when their EQ support shipped in an earlier pass (Nova Elite, Nova Pro Omni, Nova 5 were all missing `custom_eq`).

### Changed

- Voice Changer's Enable control is now a tri-state switch (off / on for this session / on, starts automatically with the daemon) instead of a plain checkbox that silently reset to inactive on every daemon restart while still showing "on."
- The background daemon has been rewritten from Python to Rust; the GUI is unchanged.
- EQ changes apply live to the running audio graph instead of reloading it, eliminating playback interruptions when adjusting gains or switching presets.
- Noise Cancellation now runs as a single audio device instead of one per effect stage.
- Device detection now matches primarily by product name (falling back to product ID), so a firmware update that changes the PID is still recognised automatically.
- Side navigation now uses icon buttons instead of text labels.
- The Voice Changer panel is labelled "(Preview)" — usable, not yet considered production quality.

### Fixed

- Rust daemon (v3): fixed a byte-order mismatch that corrupted every non-zero custom EQ gain sent to hardware on Nova Pro Wireless, Nova Pro Wired, Arctis 7+, and Nova 7 — the gain-conversion builtins decoded incoming bytes little-endian while the codec actually serialises them big-endian. Invisible in existing tests, which only ever exercised 0.0 dB (endianness-blind).
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
