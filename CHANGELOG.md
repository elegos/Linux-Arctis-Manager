# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
