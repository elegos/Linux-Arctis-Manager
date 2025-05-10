#!/usr/bin/env bash

if [ -z "${PREFIX}" ]; then
    install_prefix=/usr
else
    install_prefix=${PREFIX}
fi

if [ -x "${CHROOT}" ]; then
    chroot_path="${CHROOT%/}"
else
    chroot_path=""
fi

# Files to install
bin_files=("dist/arctis-manager" "dist/arctis-manager-launcher")
systemd_service_file="systemd/arctis-manager.service"
udev_rules_file="udev/91-steelseries-arctis.rules"
desktop_file="ArctisManager.desktop"
icon_file="arctis_manager/images/steelseries_logo.svg"

# Install directories
applications_dir="${chroot_path}${install_prefix}/share/applications/"
icons_dir="${chroot_path}/usr/share/icons/hicolor/scalable/apps/"
bin_dir="${chroot_path}${install_prefix}/bin"
udev_dir="${chroot_path}/usr/lib/udev/rules.d/"
systemd_dir="${chroot_path}/usr/lib/systemd/user/"

function superuserdo() {
    if [ "${chroot_path}" != "" ]; then
        $@
    else
        sudo $@
    fi
}

function install() {
    echo "Installing Arctis Manager..."

    superuserdo mkdir -p "${bin_dir}"
    superuserdo mkdir -p "${applications_dir}"
    superuserdo mkdir -p "${icons_dir}"

    echo "Running pyinstaller to generate binary files"
    python3 -m pip install --upgrade pipenv
    python -m pipenv install -d
    python -m pipenv run pyinstaller arctis-manager.spec
    python -m pipenv run pyinstaller arctis-manager-launcher.spec
    python -m pipenv --rm

    echo "Installing binaries in ${bin_dir}"
    for file in "${bin_files[@]}"; do
        dest_file="${bin_dir}/$(basename "${file}")"
        superuserdo cp "${file}" "${dest_file}"
    done

    echo "Installing desktop file in ${applications_dir}"
    dest_file="${applications_dir}/$(basename "${desktop_file}")"
    superuserdo cp "${desktop_file}" "${dest_file}"

    echo "Installing icon file in ${icons_dir}"
    dest_file="${icons_dir}/arctis_manager.svg"
    superuserdo cp "${icon_file}" "${dest_file}"

    # Udev rules
    echo "Installing udev rules."
    superuserdo mkdir -p "${udev_dir}"
    superuserdo cp "${udev_rules_file}" "${udev_dir}"
    if [ "${chroot_path}" == "" ]; then
        superuserdo udevadm control --reload
        superuserdo udevadm trigger
    fi

    # SystemD service
    echo "Installing and enabling systemd user service."
    if [ "${chroot_path}" == "" ]; then
        systemctl --user disable --now "$(basename ${systemd_service_file})" 2>/dev/null
    fi
    superuserdo mkdir -p "${systemd_dir}"
    superuserdo cp "${systemd_service_file}" "${systemd_dir}"
    if [ "${chroot_path}" == "" ]; then
        systemctl --user enable --now "$(basename ${systemd_service_file})"
    fi
}

function uninstall() {
    echo "Uninstalling Arctis Manager..."
    echo

    echo "Removing udev rules."
    sudo rm -rf "${udev_dir}/$(basename ${udev_rules_file})" 2>/dev/null

    echo "Removing user systemd service."
    # systemd service
    systemctl --user disable --now "$(basename ${systemd_service_file})" 2>/dev/null
    sudo rm -rf "${systemd_dir}/$(basename ${systemd_service_file})" 2>/dev/null

    echo "Removing desktop file."
    sudo rm -rf "${applications_dir}/$(basename ${desktop_file})" 2>/dev/null

    echo "Removing icon file."
    sudo rm -rf "${icons_dir}/arctis_manager.svg" 2>/dev/null

    # Remove the custom lib dir
    echo "Removing application data."
    sudo rm -rf "${lib_dir}" 2>/dev/null
    for file in "${bin_files[@]}"; do
        sudo rm -rf "${bin_dir}/$(basename "${file}")" 2>/dev/null
    done
}

# Uninstall previous version
./uninstall_old_arctis_chatmix.sh

if [[ -v UNINSTALL ]]; then
    uninstall
else
    install
fi
