#!/usr/bin/env bash

distrobox create -n arctis-manager --additional-packages "pipx python3-pip python3-devel libusb1-devel pulseaudio-libs git"

distrobox enter arctis-manager -- bash -c "
curl -LsSf https://astral.sh/uv/install.sh | sh
mkdir -p ~/src
cd ~/src
rm -rf Linux-Arctis-Manager 2>/dev/null || true
git clone https://github.com/elegos/Linux-Arctis-Manager.git -b develop
cd Linux-Arctis-Manager
rm -rf dist 2>/dev/null || true
uv build
pipx install --force dist/*.whl
mkdir -p ~/.local/share/applications
lam-cli desktop write
lam-cli udev write-rules --force --rules-path ~/91-steelseries-arctis.rules
"

# Move udev rules to the correct location and reload udev
sudo mv ~/91-steelseries-arctis.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=usb

# Systemd user service
mkdir -p ~/.config/systemd/user
cat <<'EOF' > ~/.config/systemd/user/arctis-manager.service
[Unit]
Description=Arctis Manager
StartLimitInterval=1min
StartLimitBurst=5

[Service]
Type=simple
ExecStart=__HOME__/.local/share/pipx/venvs/linux-arctis-manager/bin/lam-daemon
Restart=on-failure
RestartSec=1
StartLimitIntervalSec=60
StartLimitBurst=60

[Install]
WantedBy=graphical-session.target
EOF
sed -i "s|__HOME__|$HOME|g" ~/.config/systemd/user/arctis-manager.service

# Enable and start the systemd user service
systemctl --user enable --now arctis-manager.service
