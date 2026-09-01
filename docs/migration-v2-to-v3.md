# Migrating from v2 to v3

v3 replaces the Python background daemon with a Rust one (`lam-daemon`), split
from a small privileged helper (`lam-hidraw-helper`) that is the only process
allowed to open `/dev/hidraw*` nodes. The GUI is unchanged and talks to
whichever daemon is running over the same D-Bus interface, so there is no
settings-format migration to do — this is a service-swap, not a data
migration.

## Steps

1. **Stop and disable the v2 service.**

   ```bash
   systemctl --user disable --now arctis-manager
   ```

2. **Install v3** using whichever method matches your distro — see
   [README.md](../README.md#-install--setup).

3. **Enable the new units.**

   ```bash
   systemctl --user daemon-reload
   systemctl --user enable --now lam-hidraw-helper.service lam-daemon.service
   ```

   (The AUR and RPM packages run this for you as part of install/upgrade.)

4. **Remove the udev rule**, if you installed it manually or via a v2
   package. v3 doesn't need it: `lam-hidraw-helper` opens `/dev/hidraw*`
   directly via `CAP_DAC_OVERRIDE`, so device-node group ownership is no
   longer part of the permission model.

   ```bash
   sudo rm -f /etc/udev/rules.d/91-steelseries-arctis.rules
   sudo rm -f /usr/lib/udev/rules.d/91-steelseries-arctis.rules
   ```

5. **Remove old device YAML overrides**, if any. Device configuration files
   in `~/.config/arctis_manager/devices/` used the v2 DSL and are not
   compatible with v3's config format (see
   [DEVICE_DSL.md](DEVICE_DSL.md)). The bundled v3 device files supersede
   them; delete the folder unless you're maintaining a custom device
   definition you intend to rewrite:

   ```bash
   rm -rf ~/.config/arctis_manager/devices
   ```

Everything else under `~/.config/arctis_manager/` (general settings,
per-device settings, EQ presets) uses the same format in v2 and v3 and needs
no changes.

## Verifying the switch worked

```bash
systemctl --user status lam-hidraw-helper.service lam-daemon.service
```

Both should be `active (running)`. Then open `lam-gui` — it detects the
daemon over D-Bus the same way it did in v2, so no GUI-side changes are
needed.
