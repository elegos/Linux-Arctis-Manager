# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added (v3 daemon)

- v3: `representation` section in device YAML configs maps category names to status field lists; `GetStatus` now returns fields grouped by category instead of all under `"headset"`.
- v3: `display_type` field on `SyncEventField` (in `sync_events`) and `SyncReadMap` (in `sync_read`) lets YAML authors declare the GUI hint (`percentage`, `on_off`, `label`) for each status field; the hint is propagated through `EmitEvent.display_types` and written into the `"type"` key of the status JSON that `GetStatus` / `StatusChanged` emits. Annotated battery, mic volume, sidetone, brightness, gain, and Bluetooth toggle fields in `base_arctis_nova_pro_wireless.yaml`.
- v3: GeneralSettings — `GetSettings` now returns a populated `general` section (`redirect_audio_on_connect`, `redirect_audio_on_disconnect`, `redirect_audio_on_disconnect_device`) and their schemas in `settings_config`. `SetSetting` writes and persists general fields to `~/.config/arctis_manager/general_settings.yaml`. Default sink is redirected to `Arctis_Media` on headset connect (when enabled) and to the chosen device on wireless disconnect (when enabled). `lam-integrity-check general-settings` verifies the D-Bus contract.

### Added

- Software EQ via PulseAudio LADSPA (`mbeq_1197` from `swh-plugins`): per-channel (media and chat) equaliser with simple (10-band) and advanced (15-band) modes, ±12 dB gain range per band.
- EQ presets saved as YAML in `~/.config/arctis_manager/eq_presets/`. EQ settings (enabled, mode, preset, per-app rules) saved in `~/.config/arctis_manager/eq_settings.yaml`.
- Per-application EQ overrides: route a specific running application, executable, or Steam game to a custom EQ preset on a chosen channel. The "Add override" dialog shows currently registered audio clients (including paused apps) for stream matching, a file browser for executable selection, and deduplicates Steam games that appear across multiple library folders.
- EQ curve widget: double-clicking a band dot resets its gain to 0 dB (editable presets only).
- LADSPA plugin availability check: if `mbeq_1197` is not installed, the EQ panel shows a warning banner with per-distro install instructions and a Retry button; all EQ controls are disabled until the plugin is detected.
- GUI daemon availability check on startup: if the background service is not running and `--no-enforce-systemd` is not set, the GUI silently enables and starts the systemd unit before opening; if `--no-enforce-systemd` is set and the daemon is offline, an error dialog is shown and the application exits cleanly.
- New dependency: `vdf>=3.4` (optional at runtime — Steam game matching is skipped gracefully if not installed).
- Daemon exposes a `GetVersion` D-Bus method returning the installed package version via `importlib.metadata`.
- UI footer (bottom-right) shows the running UI version. On a version mismatch the footer shows `UI: vX | Service: vY` while the issue is resolved.
- Automatic service upgrade: if the service version is older than the UI, or if the service does not expose `GetVersion` (pre-feature version), the GUI restarts the service once via systemd, re-writing the unit file so the current binary is used. After restart, the version is re-checked.
- If the UI is older than the service, a warning dialog prompts the user to restart the application instead.
- Noise Cancellation panel (side-menu entry): software microphone noise suppression via a LADSPA plugin chain.
- Five presets: **Off**, **Light** (HPF + RNNoise), **Standard** (+ noise gate), **Studio** (+ compressor), **Custom** (full manual control). Default: Standard.
- **Custom** mode exposes gate parameters (threshold, reduction, attack, release) and compressor parameters (threshold, ratio, makeup gain), each with a per-section reset-to-defaults button.
- Input device selector, defaulting to the detected Arctis microphone source.
- Runtime plugin detection: missing `ladspa-rnnoise-plugin` disables the whole panel with a banner and per-distro install instructions; missing `swh-plugins` disables only HPF/gate/compressor stages with a separate inline banner. Both banners include a Retry button.
- D-Bus NC interface stub (`GetNCCapabilities`, `GetNCSettings`, `SetNCSettings`) on `/NC` object path — daemon-side implementation is a separate task.
- Side navigation replaced with icon buttons (36 px system-theme icons, label underneath) using `QToolButton` with palette-aware highlight on selection and hover.
- RVC voice changer: per-model advanced tuning (envelope mix, F0 smoothing, input drive, output limiter knee, VTLN formant warp) is now saved automatically per loaded model. An "Auto-tune" mode listens while you speak and lowers the input drive/envelope mix until output saturation (clipping) disappears, then saves the result.
- RVC guided voice calibration: a wizard prompts a short edge-case reading (word endings, nasals, sibilants, plosives, questions, trail-offs), renders it offline through three candidate tunings, and lets the user choose by ear (original recording included for reference), refine iteratively with narrower steps, re-record, or save the pick as the model's tuning.
- RVC FAISS feature retrieval (`.index` files): when a feature index sits next to the model (`<model>.index`), an "Index rate" per-model setting blends each voice feature with its nearest training-set neighbours — stabilising out-of-distribution input such as creaky word endings. Model zips and HuggingFace downloads now extract/fetch the index automatically, renaming it to match the model file; models with an index show a "(with index)" suffix in the model list. Requires `faiss-cpu` (added to the AI environment installer).
- RVC "Reset tuning" button next to the calibration wizard: reverts all of the current model's tuning parameters to the defaults after confirmation.
- Base AI models (RMVPE pitch estimator ~180 MB, ContentVec encoder ~360 MB) are now downloaded from the official [`Linux-Arctis-Manager-AI-Models`](https://github.com/elegos/Linux-Arctis-Manager-AI-Models) GitHub release instead of HuggingFace. Each file is verified against a SHA-256 checksum published in the release. A consent dialog informs the user of the download source and limited liability before any file is transferred. The RVC panel is disabled until both models are present. Download progress (filename + percentage) is shown in the application footer.

## Fixed

- Switching EQ presets, gains, or toggling EQ on/off no longer interrupts audio playback in apps such as Spotify, and no longer piles up dozens of unused PipeWire sinks over a session. Previously, every EQ change loaded a fresh LADSPA module and left the old one behind (idle, to avoid a PipeWire sink-removal event resetting playback streams). Changes are now pushed live to the existing LADSPA node instead, so the same sink and module are reused for the whole session — nothing to leak, nothing to reset.
- Internal EQ and noise-cancellation virtual devices are now clearly labelled (e.g. `Arctis Media EQ (internal)`) instead of showing truncated or garbled names in system sound settings.
- RVC voice changer: fixed a range of real-time synthesis artifacts — periodic clicking, dropped/garbled syllables at the start of phrases and after pauses, unintelligible isolated short words, and inconsistent output when repeating the same phrase. These were caused by issues specific to streaming (window-by-window) synthesis: hard cuts between overlapping synthesis windows, silence handling that fed the model artificial padding, and per-window randomness in the synthesizer.
- Noise cancellation: quiet consonants (e.g. nasals, word-final sounds) were sometimes being swallowed by the RNNoise/gate stages before reaching the voice changer, corrupting speech at the source; detection and gate release timing have been tuned to preserve them.
- Sidetone preview no longer stays silent when the underlying virtual audio routing had been rebuilt since the daemon started; the daemon also now self-heals that routing on restart.
- RVC voice changer: phrase endings no longer degrade into random vocals/mumbling. The pipeline gated speech with a fixed absolute threshold, so post-phrase breath and room noise were amplified and "voiced" by the model at full speech level. Gating is now level-adaptive (relative to the running speech level), quiet word-final vowels are preserved via a periodicity check so endings like "…Ginny" keep their final syllable, and phrase-final vocal fry is stabilised with wider F0 gap interpolation plus a speaker-relative pitch floor. The pitch anchor is outlier-robust: keyboard clicks and similar transients can no longer poison it (previously heard as the whole voice shifting up) nor open the gate from silence (heard as vocal blips when typing).
- Stopping the daemon no longer leaves an "audio return" (playback echoing back into the headset): the sidetone preview loopback survived teardown and was silently re-attached by the session manager to the headset's own monitor. The loopback is now pinned to its source, stale instances from crashed sessions are swept on preview start, and the preview restarts automatically when the voice-changer chain rebuilds (e.g. when switching models).
- Fixed UI's settings and status data mixing.
- Fixed i18n's newline processing.

## Changed

- The Voice Changer panel is now labelled "Voice Changer (Preview)" — usable, but not yet considered production quality.
- The noise-cancellation chain now runs as a single native PipeWire filter-chain graph in a dedicated process, exposing exactly one recording device (`Arctis Manager NC Mic`) instead of one source per LADSPA stage plus a null sink and its monitor. Settings changes are pushed live to the running graph (disabled stages are neutralized/bypassed via their controls), so the microphone device never disappears from running apps when tweaking NC parameters. The previous per-plugin PulseAudio module chain is kept as an automatic fallback when PipeWire native tools are unavailable.
- Device detection now uses the USB iProduct string (e.g. `Arctis Nova Pro Wireless`) as the primary identifier, scoped to the SteelSeries vendor ID. Product IDs remain as a tiebreaker when multiple configurations share the same product name (e.g. Nova 7 Wireless discrete vs. percentage battery variants), and as the sole matching method for configurations that do not declare a `product_string`. This allows firmware updates that change the product ID to still be recognised automatically; unknown PIDs that match by name log a warning suggesting udev rules may need updating.

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
