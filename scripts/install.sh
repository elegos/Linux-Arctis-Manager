#!/bin/bash

# Linux Arctis Manager - Automated Install Script

set -e

echo "Installing Linux Arctis Manager..."

IS_ARCH=false

# Check for Arch-based systems first
if command -v yay &> /dev/null; then
    echo "Detected yay - Installing/upgrading from AUR..."
    yay -S --noconfirm linux-arctis-manager
    IS_ARCH=true
elif command -v paru &> /dev/null; then
    echo "Detected paru - Installing/upgrading from AUR..."
    paru -S --noconfirm linux-arctis-manager
    IS_ARCH=true
elif command -v pacman &> /dev/null; then
    echo "Error: Arch detected but no AUR helper (yay/paru) found. Please install one first."
    exit 1
else
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
fi

# Only run setup if not Arch
if [ "$IS_ARCH" = false ]; then
    echo "Setting up service and starting..."
    lam-cli setup --systray-autostart --start-now
fi

echo "✓ Linux Arctis Manager installed and started!"
