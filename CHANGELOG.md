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
- Packaged systemd user services (`lam-daemon.service`, `lam-hidraw-helper.service`) could ship with a stale hardcoded `/usr/local` `ExecStart` path instead of the package's actual install prefix (`/usr`), if a leftover generated unit file from a prior local `make install` was picked up by the packaging build; the generation rule now always reruns instead of trusting file timestamps.
- `linux-arctis-manager-lang` now depends on `linux-arctis-manager` (rpm/deb/Arch), so uninstalling the main package always removes it too instead of leaving it behind as an orphan.
- The bundled Python venv's `python3` could be a dangling symlink on install (`lam-gui: ... File o directory non esistente`), on any system whose `/usr/sbin` isn't a symlink to `/usr/bin` (e.g. installs predating Fedora's `/usr`-merge): the RPM build container resolves plain `python3` to `/usr/sbin/python3` first (PATH order), and `python3 -m venv` bakes that unresolved path in verbatim. `make install-python` now resolves `python3` to its canonical, real path before creating the venv.
- The tray icon's SVG is now parsed with `defusedxml` instead of the stdlib `xml` module (XXE hardening flagged by code scanning; the icon is a bundled local asset, but the fix removes the risk class outright instead of relying on a suppression comment).
- GNOME Shell extension's `metadata.json` now declares support up to GNOME 50 (was capped at 49), so it no longer shows as incompatible on Fedora 44.
- Focus monitor no longer spams `starting xprop` / `xprop exited` every 2 seconds on GNOME Wayland: `DISPLAY` is set there for XWayland compatibility even though Mutter never mirrors `_NET_ACTIVE_WINDOW`/`_NET_CLIENT_LIST` onto that root window, so the X11 backend was being selected and immediately failing in a loop; it's now correctly reported as unsupported like other GNOME Wayland sessions. The X11 backend also now gives up and logs once instead of retrying forever if `xprop -spy -root` keeps exiting immediately for any other reason.
- Devices whose stream-volume/chat-mix values only ever arrive as unsolicited notifications (e.g. Nova Elite) could show only their polled settings (e.g. just OLED Brightness) after a cold daemon start, only gaining the rest once the headset was later power-cycled: a notification buffered ahead of a sync-read reply during startup was silently discarded instead of being dispatched, so its value never reached the settings panel until a later reconnect happened to deliver it while the event loop was already running.
- Arctis Nova 7 family (Gen 1, Gen 2, and Gen 2-firmware-on-Gen-1-hardware; e.g. PID `0x22A1`, Nova 7P) could get stuck logging "headset not ready, waiting for wireless event" forever, even with the headset genuinely on, paired, and playing audio: the device's unsolicited/wireless-connection-changed notifications arrive on a distinct HID interface (5) from its command channel (3) — confirmed against the decoded vendor spec (identical across all 15 Nova 7 family TX variants, no per-SKU exception) and a real bug report's hidraw capture — but the daemon only ever listened on the command interface. The engine now opens a device's declared `sync_interface` as a second hidraw connection whenever it differs from `command_interface`, forwarding its reports into the same status/D-Bus pipeline; this is generic support, not Nova 7-specific — the same fix also silently activates already-correct `sync_interface` config that was previously parsed but never used on Nova 4, Nova 5, Nova 3 Wireless, both Arctis GameBuds variants, Arctis 9, and Arctis Pro Wireless. The separate Arctis 7+ family shows the same raw-spec-vs-v2 discrepancy but wasn't changed — no equivalent real-hardware confirmation yet.
- Arctis Nova 7 family, second half of the same hang: even once the interface above woke it up, `device_init` still never succeeded, because the command interface's own replies were being silently discarded too. Root cause: this HID interface has no Report ID in its descriptor, and Linux's hidraw needs a synthetic leading report-id byte on writes regardless (`hidraw_write` always treats the first byte as a report id to strip) but never adds one back on reads — so every real reply arrived one byte short of what the engine expected, every time, forever. Confirmed with a real `hid-recorder` capture on a bug reporter's own hardware (a second, independent reader on the same hidraw node running alongside the stuck daemon) and decoded field-by-field against the byte shift — every value sane (paired/connected, 100% battery, discharging). Devices can now declare `hid.unnumbered_reports: true` to have the engine synthesize the missing byte back onto every read, transparently, so no struct or byte-offset elsewhere in that device's config needs to account for the asymmetry. Applied to the whole Nova 7 family; the Gen 1/7P instances are inferred by same-hardware-family analogy rather than independently captured, but fail loud (not silently) if that inference is ever wrong, so it's safe to ship ahead of separate confirmation.
- Arctis Nova 7 family: once connected, the hardware chatmix dial had no effect on audio at all, even though the same bug reporter's `hid-recorder` capture proved the device was sending correct, live `0x45` chatmix events on the wire. The engine's live audio-rebalancing and virtual-sink wireless-connect/disconnect lifecycle only ever look at fields literally named `chatmix_game`/`chatmix_chat`/`radio_connection_status` in the device's reported status (the convention already used by the Nova Pro Wireless/Omni and Nova Elite configs), but this family's `headset_status` struct instead called them `game_chatmix_level`/`chat_chatmix_level`/`connection_status` — so every value reached the status map under a key nothing was ever looking for. Renamed to match the shared convention across all three Nova 7 device files; an audit found the identical mismatch, same field names, also present (independently of this bug report, no per-family confirmation needed — it's a pure string-match bug, not a protocol one) on Nova 4, Nova 5, and Arctis Nova 3 Wireless, fixed the same way in the same pass.

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
