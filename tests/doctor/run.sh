#!/usr/bin/env bash
# Runs scripts/doctor.sh against a fake sysfs tree and stub commands
# (systemctl, getcap, busctl, pactl, journalctl, pgrep) so its verdicts can
# be checked without real hardware or a real install. Each scenario asserts
# on the output lines and the exit code.
#
# Usage: tests/doctor/run.sh
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
DOCTOR=$HERE/../../scripts/doctor.sh
FAILED=0

# new_world -> sets $W to a fresh fake machine: one healthy Nova 7 Gen 2
# dongle, v3 installed and running, helper with its capability.
new_world() {
    W=$(mktemp -d)
    local dev=$W/sys/devices/usb3/3-4
    mkdir -p "$dev/3-4:1.0" "$dev/3-4:1.5/hid/hidraw/hidraw3" "$W/sys/bus/usb/devices" "$W/sys/class/hidraw/hidraw3" \
        "$W/bin" "$W/home/.config" "$W/data/devices" "$W/libexec" "$W/udev"
    echo 1038 >"$dev/idVendor"
    echo 227e >"$dev/idProduct"
    echo "Arctis Nova 7 Gen 2" >"$dev/product"
    echo 01 >"$dev/3-4:1.0/bInterfaceClass"
    echo 03 >"$dev/3-4:1.5/bInterfaceClass"
    # Real sysfs lists interfaces as siblings of the device in bus/usb/devices.
    ln -s "$dev" "$W/sys/bus/usb/devices/3-4"
    ln -s "$dev/3-4:1.0" "$W/sys/bus/usb/devices/3-4:1.0"
    ln -s "$dev/3-4:1.5" "$W/sys/bus/usb/devices/3-4:1.5"
    ln -s "$dev/3-4:1.5" "$W/sys/class/hidraw/hidraw3/device"

    printf 'device:\n  variants:\n    - product_id: 0x227E\n      bootloader_pid: 0x227F\n' >"$W/data/devices/nova_7_gen2.yaml"
    touch "$W/libexec/lam-hidraw-helper" "$W/bin/lam-daemon"
    chmod +x "$W/libexec/lam-hidraw-helper" "$W/bin/lam-daemon"

    # Stub state, edited per scenario.
    echo active >"$W/daemon_state"
    echo cap_dac_override=ep >"$W/getcap_out"
    printf 'Arctis_Media\nArctis_Chat\nalsa_output.usb-SteelSeries\n' >"$W/sinks"

    cat >"$W/bin/systemctl" <<EOF
#!/bin/sh
case "\$*" in
  "--user show-environment") exit 0 ;;
  *"-p LoadState --value arctis-manager.service") echo not-found ;;
  *"-p LoadState --value "*) echo loaded ;;
  *"-p UnitFileState --value "*) echo enabled ;;
  *"-p ActiveState --value arctis-manager.service") echo inactive ;;
  *"-p ActiveState --value "*) cat "$W/daemon_state" ;;
  *"-p ExecStart --value lam-hidraw-helper.service") echo "{ path=$W/libexec/lam-hidraw-helper ; argv[]=x }" ;;
  *"-p ExecStart --value "*) echo "{ path=$W/bin/lam-daemon ; argv[]=x }" ;;
esac
EOF
    cat >"$W/bin/getcap" <<EOF
#!/bin/sh
printf '%s %s\n' "\$1" "\$(cat "$W/getcap_out")"
EOF
    cat >"$W/bin/busctl" <<EOF
#!/bin/sh
[ "\$(cat "$W/daemon_state")" = active ]
EOF
    cat >"$W/bin/pactl" <<EOF
#!/bin/sh
case "\$*" in
  info) exit 0 ;;
  "list short sinks") awk '{print NR "\t" \$0}' "$W/sinks" ;;
  get-default-sink) echo Arctis_Media ;;
esac
EOF
    printf '#!/bin/sh\necho "daemon log line"\n' >"$W/bin/journalctl"
    printf '#!/bin/sh\nexit 1\n' >"$W/bin/pgrep"
    chmod +x "$W"/bin/*
}

run_doctor() { # -> $OUT, $CODE
    OUT=$(env -i PATH="$W/bin:/usr/bin:/bin" HOME="$W/home" XDG_CONFIG_HOME="$W/home/.config" \
        LAM_DATADIR="$W/data" LAM_DOCTOR_SYSFS="$W/sys" LAM_DOCTOR_UDEV_DIRS="$W/udev" \
        bash "$DOCTOR" --no-color 2>&1)
    CODE=$?
}

expect() { # expect SCENARIO PATTERN (grep -E against output)
    if ! grep -qE "$2" <<<"$OUT"; then
        echo "FAIL [$1]: expected output matching: $2"
        echo "$OUT" | sed 's/^/    /'
        FAILED=1
    fi
}
expect_code() {
    if [[ "$CODE" != "$2" ]]; then
        echo "FAIL [$1]: expected exit $2, got $CODE"
        FAILED=1
    fi
}

# 1. Healthy install.
new_world
run_doctor
expect healthy '^  OK Arctis Nova 7 Gen 2 \(1038:227e\) is supported'
expect healthy '^  OK Arctis Nova 7 Gen 2 \(1038:227e\) has 1 hidraw node'
expect healthy '^  OK lam-hidraw-helper has cap_dac_override'
expect healthy '^  OK ChatMix sinks Arctis_Media and Arctis_Chat exist'
expect healthy '^Everything looks good\.'
expect_code healthy 0
rm -rf "$W"

# 2. Helper lost its capability (e.g. after a manual rebuild/copy), daemon down.
new_world
: >"$W/getcap_out"
echo failed >"$W/daemon_state"
run_doctor
expect no-cap '^FAIL lam-hidraw-helper is missing the cap_dac_override capability'
expect no-cap 'fix: sudo setcap cap_dac_override\+eip .*/lam-hidraw-helper'
expect no-cap '^FAIL lam-daemon.service is not running \(failed\)'
expect no-cap '\| daemon log line'
expect_code no-cap 1
rm -rf "$W"

# 3. A SteelSeries audio device no config knows about.
new_world
echo ffff >"$W/sys/devices/usb3/3-4/idProduct"
run_doctor
expect unsupported '^FAIL Arctis Nova 7 Gen 2 \(1038:ffff\) is not supported yet'
expect_code unsupported 1
rm -rf "$W"

# 4. Dongle stuck in firmware-update mode; v2 leftovers present.
new_world
echo 227f >"$W/sys/devices/usb3/3-4/idProduct"
touch "$W/udev/91-steelseries-arctis.rules"
mkdir -p "$W/home/.config/arctis_manager/devices"
printf 'device:\n  command_padding:\n    length: 64\n' >"$W/home/.config/arctis_manager/devices/old.yaml"
echo Arctis_Game >>"$W/sinks"
run_doctor
expect bootloader '^WARN .*\(1038:227f\) is in firmware-update \(bootloader\) mode'
expect bootloader '^WARN old udev rule .*/91-steelseries-arctis.rules is not needed by v3'
expect bootloader '^WARN .*/old.yaml is a v2-format device config'
expect bootloader '^WARN an Arctis_Game sink exists'
expect_code bootloader 0
rm -rf "$W"

if ((FAILED)); then
    echo "doctor tests FAILED"
    exit 1
fi
echo "doctor tests passed"
