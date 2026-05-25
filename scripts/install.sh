#!/bin/bash

# Linux Arctis Manager - Automated Install Script

set -e

echo "Installing Linux Arctis Manager..."

# Check if pipx is installed
if ! command -v pipx &> /dev/null; then
    echo "Error: pipx not found. Please install pipx first."
    exit 1
fi

echo "Fetching latest release..."
DOWNLOAD_URL=$(curl -s https://api.github.com/repos/elegos/Linux-Arctis-Manager/releases/latest | grep -oP '"browser_download_url": "\K[^"]*\.whl' | head -1)

if [ -z "$DOWNLOAD_URL" ]; then
    echo "Error: Could not fetch latest release"
    exit 1
fi

curl -LO "$DOWNLOAD_URL"
WHEEL_FILE=$(basename "$DOWNLOAD_URL")

# Check if already installed
if pipx list | grep -q "linux-arctis-manager"; then
    echo "Upgrading existing installation..."
    pipx install --force "$WHEEL_FILE"
else
    echo "Installing linux-arctis-manager..."
    pipx install "$WHEEL_FILE"
fi

# Setup and start service
echo "Setting up service and starting..."
lam-cli setup --systray-autostart --start-now

echo "✓ Linux Arctis Manager installed and started!"
