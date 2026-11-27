#!/usr/bin/env bash
# Checks whether Linux Arctis Manager can work on this machine and, for
# every problem it finds, prints the command that fixes it. Meant as the
# first thing to run when "my headset doesn't work" - by a person or by a
# coding agent helping them.
#
# Read-only: never uses sudo, never installs or changes anything, never
# writes to the headset. Run it as your normal desktop user, with the
# headset dongle plugged in.
#
# Usage: ./scripts/doctor.sh [--no-color]
#   (or, without a checkout:
#    curl -fsSLO https://raw.githubusercontent.com/elegos/Linux-Arctis-Manager/develop/scripts/doctor.sh
#    bash doctor.sh)
#
# Output: one line per check, prefixed OK / WARN / FAIL / SKIP / INFO,
# followed by an indented "fix:" line for WARN/FAIL. Exit code is 1 if any
# check FAILed, 0 otherwise.
#
# If doctor.sh can't explain the problem, scripts/collect-debug-info.sh
# builds the full support bundle to attach to a GitHub issue.

set -uo pipefail

VID="1038" # SteelSeries
BUS_NAME="name.giacomofurlan.ArctisManager.Next"
DAEMON_UNIT="lam-daemon.service"
HELPER_UNIT="lam-hidraw-helper.service"
LEGACY_UNIT="arctis-manager.service" # v1/v2 daemon
REPO_URL="https://github.com/elegos/Linux-Arctis-Manager"

# Overridable for tests (tests/doctor/run.sh).
SYSFS="${LAM_DOCTOR_SYSFS:-/sys}"
UDEV_RULES_DIRS="${LAM_DOCTOR_UDEV_DIRS:-/etc/udev/rules.d /usr/lib/udev/rules.d /lib/udev/rules.d}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"

COLOR=1
[[ -t 1 ]] || COLOR=0
for arg in "$@"; do
    case "$arg" in
        --no-color) COLOR=0 ;;
        --help | -h)
            sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown argument: $arg (see --help)" >&2
            exit 2
            ;;
    esac
done

FAILS=0
WARNS=0

paint() { # $1 = ANSI color code, $2 = text
    if ((COLOR)); then printf '\033[%sm%s\033[0m' "$1" "$2"; else printf '%s' "$2"; fi
}
ok()   { printf '%s %s\n' "$(paint 32 '  OK')" "$1"; }
info() { printf '%s %s\n' "$(paint 36 'INFO')" "$1"; }
skip() { printf '%s %s\n' "$(paint 90 'SKIP')" "$1"; }
warn() { WARNS=$((WARNS + 1)); printf '%s %s\n' "$(paint 33 'WARN')" "$1"; shift; fixes "$@"; }
fail() { FAILS=$((FAILS + 1)); printf '%s %s\n' "$(paint 31 'FAIL')" "$1"; shift; fixes "$@"; }
fixes() { local f; for f in "$@"; do printf '       fix: %s\n' "$f"; done; }
detail() { sed 's/^/       | /'; } # indent captured command output
section() { printf '\n%s\n' "$(paint 1 "== $* ==")"; }
have() { command -v "$1" >/dev/null 2>&1; }

# unit_prop UNIT PROPERTY -> value from `systemctl --user show`
unit_prop() { systemctl --user show -p "$2" --value "$1" 2>/dev/null; }

echo "Linux Arctis Manager doctor (read-only: changes nothing)"

# ── Session ─────────────────────────────────────────────────────────────────
section "Session"
SYSTEMD_OK=0
if [[ ${EUID:-$(id -u)} -eq 0 ]]; then
    fail "running as root" \
        "run doctor.sh as your normal desktop user: the daemon and its helper are per-user services"
elif ! have systemctl; then
    fail "systemctl not found: Linux Arctis Manager runs as systemd user services"
elif ! systemctl --user show-environment >/dev/null 2>&1; then
    fail "no systemd user session reachable (ssh, container or distrobox without --systemd?)" \
        "run doctor.sh from a terminal inside your desktop session"
else
    ok "systemd user session reachable"
    SYSTEMD_OK=1
fi

# ── Devices ─────────────────────────────────────────────────────────────────
section "SteelSeries devices"

# Device config dirs, in daemon priority order (user overrides first), then
# the source checkout this script lives in, if any.
CONFIG_DIRS=()
[[ -n "${LAM_DATADIR:-}" ]] && CONFIG_DIRS+=("$LAM_DATADIR/devices")
CONFIG_DIRS+=("$HOME/.local/share/linux-arctis-manager/devices"
    /usr/local/share/linux-arctis-manager/devices
    /usr/share/linux-arctis-manager/devices)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)"
CHECKOUT_CONFIGS="$SCRIPT_DIR/../daemon/device-configs"

# ids_in DIR KEY -> lowercase hex ids (no 0x) for KEY (product_id or
# bootloader_pid) across DIR's device files (base_*.yaml hold no ids).
ids_in() {
    [[ -d "$1" ]] || return 0
    find "$1" -maxdepth 1 -name '*.yaml' ! -name 'base_*' -print0 2>/dev/null |
        xargs -0 -r grep -h -o -i -E "$2:[[:space:]]*0x[0-9a-f]+" 2>/dev/null |
        sed -E 's/.*0[xX]//' | tr 'A-F' 'a-f' | sort -u
}

INSTALLED_IDS=""
INSTALLED_BOOT=""
INSTALLED_DIR=""
for d in "${CONFIG_DIRS[@]}"; do
    ids="$(ids_in "$d" product_id)"
    if [[ -n "$ids" ]]; then
        INSTALLED_IDS+="$ids"$'\n'
        INSTALLED_BOOT+="$(ids_in "$d" bootloader_pid)"$'\n'
        [[ -z "$INSTALLED_DIR" ]] && INSTALLED_DIR="$d"
    fi
done
CHECKOUT_IDS="$(ids_in "$CHECKOUT_CONFIGS" product_id)"

has_id() { grep -qx "$2" <<<"$1"; } # has_id LIST ID

HEADSETS=() # sysfs dirs of SteelSeries audio devices
IGNORED=0
for dev in "$SYSFS"/bus/usb/devices/*; do
    [[ -f "$dev/idVendor" ]] || continue
    [[ "$(cat "$dev/idVendor")" == "$VID" ]] || continue
    pid="$(tr 'A-F' 'a-f' <"$dev/idProduct")"
    # Headsets/dongles expose a USB Audio interface (class 01); keyboards
    # and mice don't. Configured pure-HID accessories (charging cases) count.
    if grep -qx 01 "$dev":*/bInterfaceClass 2>/dev/null || has_id "$INSTALLED_IDS$CHECKOUT_IDS" "$pid"; then
        HEADSETS+=("$dev")
    elif has_id "$INSTALLED_BOOT" "$pid"; then
        HEADSETS+=("$dev")
    else
        IGNORED=$((IGNORED + 1))
    fi
done

((IGNORED)) && info "ignoring $IGNORED non-audio SteelSeries device(s) (keyboard, mouse, ...)"

if ((${#HEADSETS[@]} == 0)); then
    fail "no SteelSeries headset or dongle detected on USB" \
        "plug the dongle/base station straight into the PC (not a hub) and check: lsusb -d ${VID}:"
fi

for dev in "${HEADSETS[@]}"; do
    pid="$(tr 'A-F' 'a-f' <"$dev/idProduct")"
    name="$(cat "$dev/product" 2>/dev/null || echo 'unknown product')"
    label="$name (${VID}:${pid})"

    if has_id "$INSTALLED_BOOT" "$pid"; then
        warn "$label is in firmware-update (bootloader) mode" \
            "unplug and replug it; if it stays this way, finish or retry the firmware update"
        continue
    fi

    if has_id "$INSTALLED_IDS" "$pid"; then
        ok "$label is supported (config in $INSTALLED_DIR)"
    elif [[ -z "$INSTALLED_IDS" ]] && has_id "$CHECKOUT_IDS" "$pid"; then
        info "$label is supported by this checkout (no installed device configs found yet)"
    elif has_id "$CHECKOUT_IDS" "$pid"; then
        warn "$label is supported in this checkout but not by the installed version" \
            "update Linux Arctis Manager, then: systemctl --user restart $DAEMON_UNIT"
    elif [[ -z "$INSTALLED_IDS" && -z "$CHECKOUT_IDS" ]]; then
        skip "$label: can't tell if it's supported (no device configs found; see the Install section)"
    else
        fail "$label is not supported yet" \
            "check docs/device_compatibility.md, then open a 'New device' issue at $REPO_URL/issues with the bundle from scripts/collect-debug-info.sh"
    fi

    # Is the kernel exposing hidraw nodes for it? (the helper opens these)
    nodes=0
    for h in "$SYSFS"/class/hidraw/*; do
        [[ -e "$h/device" ]] || continue
        [[ "$(readlink -f "$h/device")" == "$(readlink -f "$dev")"/* ]] && nodes=$((nodes + 1))
    done
    if ((nodes)); then
        ok "$label has $nodes hidraw node(s)"
    else
        fail "$label has no /dev/hidraw* nodes (the kernel's HID driver isn't bound to it)" \
            "replug the device; check 'sudo dmesg | tail' for USB errors"
    fi
done

# ── Install ─────────────────────────────────────────────────────────────────
section "Install"
INSTALL_HINT="install it: $REPO_URL#-install--setup"
exec_path() { unit_prop "$1" ExecStart | sed -n 's/.*path=\([^ ;]*\).*/\1/p'; }

DAEMON_BIN="$(command -v lam-daemon 2>/dev/null)"
if [[ -z "$DAEMON_BIN" ]] && ((SYSTEMD_OK)); then
    DAEMON_BIN="$(exec_path "$DAEMON_UNIT")"
    [[ -x "$DAEMON_BIN" ]] || DAEMON_BIN=""
fi

# The helper's real path is whatever its unit runs; fall back to the
# locations the Makefile, distro packages and the user-level installer use.
HELPER=""
((SYSTEMD_OK)) && HELPER="$(exec_path "$HELPER_UNIT")"
if [[ -z "$HELPER" || ! -e "$HELPER" ]]; then
    HELPER=""
    for c in "$HOME/.local/libexec/lam-hidraw-helper" /usr/local/libexec/lam-hidraw-helper \
        /usr/libexec/lam-hidraw-helper /usr/lib/linux-arctis-manager/lam-hidraw-helper; do
        [[ -e "$c" ]] && HELPER="$c" && break
    done
fi

UNITS_LOADED=0
((SYSTEMD_OK)) && [[ "$(unit_prop "$DAEMON_UNIT" LoadState)" == "loaded" ]] && UNITS_LOADED=1

# Nothing of v3 at all: say so once instead of failing every later check.
INSTALLED=1
if [[ -z "$DAEMON_BIN" && -z "$HELPER" && -z "$INSTALLED_IDS" ]] && ! ((UNITS_LOADED)); then
    INSTALLED=0
    fail "Linux Arctis Manager v3 is not installed (no lam-daemon, lam-hidraw-helper, services or device configs)" \
        "$INSTALL_HINT  - then re-run this script"
fi

if ((INSTALLED)); then
    if [[ -n "$DAEMON_BIN" ]]; then
        ok "lam-daemon found: $DAEMON_BIN"
    else
        fail "lam-daemon not found" "$INSTALL_HINT"
    fi
fi

if ! ((INSTALLED)); then
    :
elif [[ -z "$HELPER" ]]; then
    fail "lam-hidraw-helper not found" "$INSTALL_HINT"
else
    ok "lam-hidraw-helper found: $HELPER"
    # The helper is the only thing allowed to open /dev/hidraw*; it needs
    # this file capability (v3 uses no udev rules). Copying or rebuilding the
    # binary drops it.
    if ! have getcap; then
        skip "can't check the helper's capability: 'getcap' not found (libcap / libcap2-bin package)"
    elif getcap "$HELPER" 2>/dev/null | grep -q cap_dac_override; then
        ok "lam-hidraw-helper has cap_dac_override"
    else
        fail "lam-hidraw-helper is missing the cap_dac_override capability, so it can't open the headset" \
            "sudo setcap cap_dac_override+eip $HELPER && systemctl --user restart $HELPER_UNIT $DAEMON_UNIT"
    fi
fi

if ((INSTALLED)) && [[ -z "$INSTALLED_IDS" ]]; then
    fail "no installed device configs found (looked in ${CONFIG_DIRS[*]})" "$INSTALL_HINT"
fi

# ── Services ────────────────────────────────────────────────────────────────
section "Services"
if ! ((INSTALLED)); then
    skip "service checks (not installed yet)"
elif ((SYSTEMD_OK)); then
    for unit in "$HELPER_UNIT" "$DAEMON_UNIT"; do
        if [[ "$(unit_prop "$unit" LoadState)" != "loaded" ]]; then
            fail "$unit is not installed" "$INSTALL_HINT"
            continue
        fi
        enabled="$(unit_prop "$unit" UnitFileState)"
        active="$(unit_prop "$unit" ActiveState)"
        if [[ "$enabled" != "enabled" ]]; then
            warn "$unit is not enabled ($enabled), so it won't start at login" \
                "systemctl --user enable --now $HELPER_UNIT $DAEMON_UNIT"
        fi
        if [[ "$active" == "active" ]]; then
            ok "$unit is running"
        else
            fail "$unit is not running ($active); last log lines:" \
                "systemctl --user restart $HELPER_UNIT $DAEMON_UNIT"
            if have journalctl; then
                journalctl --user -u "$unit" -n 6 --no-pager -o cat 2>/dev/null | detail
            fi
        fi
    done
else
    skip "service checks (no systemd user session, see above)"
fi

# ── D-Bus ───────────────────────────────────────────────────────────────────
section "D-Bus"
if have busctl; then
    busctl --user status "$BUS_NAME" >/dev/null 2>&1 && on_bus=1 || on_bus=0
elif have gdbus; then
    gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner "$BUS_NAME" 2>/dev/null | grep -q true && on_bus=1 || on_bus=0
else
    on_bus=""
fi
if ! ((INSTALLED)); then
    skip "D-Bus check (not installed yet)"
elif [[ -z "$on_bus" ]]; then
    skip "can't check D-Bus: neither busctl nor gdbus found"
elif ((on_bus)); then
    ok "daemon is answering on the session bus ($BUS_NAME), so the GUI and widgets can reach it"
else
    fail "daemon is not on the session bus, so the GUI and widgets can't reach it" \
        "see the Services section above; then: journalctl --user -u $DAEMON_UNIT -n 50"
fi

# ── Audio ───────────────────────────────────────────────────────────────────
section "Audio (PipeWire)"
SINKS=""
have pactl && SINKS="$(pactl list short sinks 2>/dev/null | cut -f2)"
if ! have pactl; then
    fail "pactl not found: the daemon needs it to create the ChatMix sinks" \
        "install your distro's pipewire-pulse (or pulseaudio-utils) package"
elif ! pactl info >/dev/null 2>&1; then
    fail "pactl can't reach the sound server" "make sure PipeWire (pipewire-pulse) is running: systemctl --user status pipewire-pulse"
elif ! ((INSTALLED)); then
    ok "sound server reachable (ChatMix sink check skipped: not installed yet)"
else
    missing_sinks=()
    for s in Arctis_Media Arctis_Chat; do
        grep -qx "$s" <<<"$SINKS" || missing_sinks+=("$s")
    done
    if ((${#missing_sinks[@]} == 0)); then
        ok "ChatMix sinks Arctis_Media and Arctis_Chat exist"
        default_sink="$(pactl get-default-sink 2>/dev/null)"
        [[ -n "$default_sink" ]] && info "default output is $default_sink (apps without their own choice play here)"
    else
        warn "missing sink(s): ${missing_sinks[*]} - ChatMix won't do anything until they exist" \
            "power the headset on: the daemon creates these once the headset itself (not just the dongle) is connected" \
            "if it is on: journalctl --user -u $DAEMON_UNIT -n 50"
    fi
fi

# ── Leftovers from v1/v2 ────────────────────────────────────────────────────
section "Leftovers from older versions (v1/v2)"
leftovers=0
if ((SYSTEMD_OK)) && [[ "$(unit_prop "$LEGACY_UNIT" LoadState)" == "loaded" ]]; then
    leftovers=1
    if [[ "$(unit_prop "$LEGACY_UNIT" ActiveState)" == "active" ]]; then
        fail "the old $LEGACY_UNIT (v1/v2) is running and will fight lam-daemon over the headset" \
            "systemctl --user disable --now $LEGACY_UNIT  (then see docs/migration-v2-to-v3.md)"
    else
        warn "the old $LEGACY_UNIT (v1/v2) is still installed" \
            "systemctl --user disable $LEGACY_UNIT  and uninstall v2 (docs/migration-v2-to-v3.md)"
    fi
fi
# An old daemon started by hand, outside systemd.
if have pgrep && pgrep -x arctis-manager >/dev/null 2>&1 &&
    ! { ((SYSTEMD_OK)) && [[ "$(unit_prop "$LEGACY_UNIT" ActiveState)" == "active" ]]; }; then
    leftovers=1
    fail "an old arctis-manager (v1/v2) process is running and will fight lam-daemon over the headset" \
        "pkill -x arctis-manager  (then uninstall v2: docs/migration-v2-to-v3.md)"
fi
# v2 named its virtual sinks Arctis_Game/Arctis_Chat; v3 uses Arctis_Media.
if grep -qx Arctis_Game <<<"$SINKS"; then
    leftovers=1
    warn "an Arctis_Game sink exists: that's the v1/v2 daemon's, v3 creates Arctis_Media instead" \
        "stop the old daemon (see above); the sink disappears with it"
fi
# Each rules file once, even if it matches both patterns or /lib is /usr/lib.
seen_rules=" "
for d in $UDEV_RULES_DIRS; do
    for f in "$d"/*steelseries*.rules "$d"/*arctis*.rules; do
        [[ -e "$f" ]] || continue
        real="$(readlink -f "$f")"
        [[ "$seen_rules" == *" $real "* ]] && continue
        seen_rules+="$real "
        leftovers=1
        warn "old udev rule $real is not needed by v3 (harmless, but stale)" "sudo rm -f $real"
    done
done
if [[ -d "$CONFIG_HOME/arctis_manager/devices" ]]; then
    while IFS= read -r f; do
        leftovers=1
        warn "$f is a v2-format device config; v3 reads this folder and can't use it" \
            "move it out of $CONFIG_HOME/arctis_manager/devices/ (see docs/migration-v2-to-v3.md)"
    done < <(grep -l -E 'command_padding|listen_interface_indexes' "$CONFIG_HOME/arctis_manager/devices"/*.yaml 2>/dev/null)
fi
((leftovers)) || ok "no v1/v2 leftovers found"

# ── Summary ─────────────────────────────────────────────────────────────────
echo
if ((FAILS)); then
    echo "$(paint 31 "$FAILS problem(s)"), $WARNS warning(s). Fix the FAIL lines top to bottom, then re-run this script."
    echo "Still stuck? Run scripts/collect-debug-info.sh and attach its bundle to a 'Headset not working' issue: $REPO_URL/issues"
    exit 1
fi
if ((WARNS)); then
    echo "No blocking problems, $(paint 33 "$WARNS warning(s)") worth a look."
else
    paint 32 'Everything looks good.'
    echo
fi
exit 0
