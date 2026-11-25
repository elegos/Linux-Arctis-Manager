# Documentation

## Using it

- [Installing](install.md): every install method, signature verification, desktop integration
- [Building from source](build-from-source.md): in a container (recommended) or bare metal
- [Uninstalling](uninstall.md)
- [Equalizer](eq.md): band modes, backends, presets, per-app overrides
- [Voice Changer](voice-changing-feature.md): usage and architecture
- [Migrating from v2 to v3](migration-v2-to-v3.md)
- [Device compatibility matrix](device_compatibility.md): auto-generated from the v3 device configs; the README's table is the human-curated summary, this is the exhaustive per-capability one

## Hacking on it

- [Desktop environment testing](desktop-testing.md): screenshot tests on Cinnamon, KDE Plasma and GNOME
Basic commands:

- Run the daemon: `cargo run --release --manifest-path daemon/Cargo.toml --bin lam-daemon`
- Run the GUI against it: `uv run lam-gui --no-enforce-systemd` (skips the systemd-managed-daemon check, since you're running one by hand)
- Run the daemon's tests: `cargo test --manifest-path daemon/Cargo.toml`

Reference:

- [Architecture overview](ARCHITECTURE.md)
- [Device DSL reference](DEVICE_DSL.md): the YAML format used to describe a device
- [D-Bus interface reference](dbus.md)
- [Release & distribution pipeline](release-pipeline.md): versioning/channels, cutting a release, the COPR/PPA/OBS publishing design, and the manual setup checklist
- [Wireshark tutorial](https://www.youtube.com/watch?v=zWbdnHwTr3M)
