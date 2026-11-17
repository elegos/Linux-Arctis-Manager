# Uninstalling

Use the method that matches how you installed.

## User-level installer

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/elegos/Linux-Arctis-Manager/releases/latest/download/install.sh | sh -s -- uninstall
```

`--purge` also deletes your settings. While every release is still a pre-release, `releases/latest`
doesn't resolve: download `install.sh` from a tag instead, as shown in
[Installing](install.md#user-level-installer-steamos-and-any-distro), or run the copy you already
have with `sh install.sh uninstall`.

## Arch Linux (AUR or release package)

The pre-removal hook stops and disables the services for you:

```bash
sudo pacman -Rns linux-arctis-manager
```

## Fedora (COPR)

Removing the main package also pulls in whichever UI-shell sub-packages you installed, since they
`Requires:` it:

```bash
sudo dnf remove linux-arctis-manager
```

## Debian, Ubuntu and derivatives

```bash
sudo apt remove linux-arctis-manager
```

(`apt remove` the `-lang`, `-plasma-widget` and `-gnome-extension` packages too if you installed
them.)

## Build from source

From the cloned repository:

```bash
make disable          # stop + disable the services (no sudo)
sudo make uninstall   # remove binaries, systemd units, desktop entries
```

Then remove your local settings if you don't intend to reinstall:

```bash
rm -rf ~/.config/arctis_manager
```

> [!NOTE]
> If you're coming from a v2 install, also remove the old udev rule, since v3 doesn't need one:
> `sudo rm -f /etc/udev/rules.d/91-steelseries-arctis.rules /usr/lib/udev/rules.d/91-steelseries-arctis.rules`
