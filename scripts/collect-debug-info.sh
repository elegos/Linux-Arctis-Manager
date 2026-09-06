#!/usr/bin/env bash
# Collects a support bundle for Linux Arctis Manager bug reports: system/
# package info, the daemon's live D-Bus state, systemd unit status, recent
# logs, PipeWire audio routing (sinks/sink-inputs/loopback modules), connected
# SteelSeries HID device descriptors, and the daemon's own (non-secret) config
# files.
#
# Cross-distro: never installs anything, only checks whether each tool
# (lsusb, udevadm, busctl/gdbus, hid-recorder, pactl/wpctl, rpm/dpkg/pacman)
# is present and skips that section with a note if it's missing.
# Cross-device: doesn't hardcode a PID — enumerates every /dev/hidraw* node
# under SteelSeries' vendor ID (0x1038) it finds.
#
# Usage: ./scripts/collect-debug-info.sh [--usb-capture[=SECONDS]]
#
#   --usb-capture[=SECONDS]  Opt-in, needs root. Also records a raw USB
#                            bus-level trace (via usbmon) for SECONDS
#                            (default 40) into the bundle. Use this for a
#                            device_init/"headset not ready" hang report:
#                            hidraw-level tools (hid-recorder, below) only
#                            show reports arriving at YOUR process, not what
#                            lam-daemon itself writes out or whether the
#                            device replies at all — a usbmon capture shows
#                            both directions on the wire. Start the daemon
#                            (or make sure it's already stuck retrying) and
#                            have the headset powered on *before* running
#                            this, since the capture window is short and
#                            fixed. CAVEAT: usbmon captures per USB BUS, not
#                            per device — if another USB device shares the
#                            same bus/hub as the SteelSeries one, its traffic
#                            is in the capture too. Review usbmon-bus*.txt
#                            before attaching it anywhere.
#
# Output: a .tar.gz under /tmp, whose path is printed at the end. Attach
# that file to your GitHub issue by hand — this script does not send or
# upload anything on its own.

set -uo pipefail

VID="1038" # SteelSeries

USB_CAPTURE_SECONDS=""
for arg in "$@"; do
    case "$arg" in
        --usb-capture) USB_CAPTURE_SECONDS=40 ;;
        --usb-capture=*) USB_CAPTURE_SECONDS="${arg#*=}" ;;
        --help | -h)
            sed -n '2,36p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown argument: $arg (see --help)" >&2
            exit 1
            ;;
    esac
done

WORK_DIR="$(mktemp -d /tmp/lam-debug-info.XXXXXX)"
BUNDLE="${WORK_DIR}/bundle"
REPORT="${BUNDLE}/report.txt"
mkdir -p "${BUNDLE}/hid-descriptors" "${BUNDLE}/config"

have() { command -v "$1" >/dev/null 2>&1; }

section() {
    {
        echo
        echo "════════════════════════════════════════════════════════════════"
        echo "== $* =="
        echo "════════════════════════════════════════════════════════════════"
    } >>"$REPORT"
}

# Runs "$@", appending its output to the report under a small header.
# Never aborts the script on failure — just records that it failed.
run() {
    local label="$1"
    shift
    {
        echo
        echo "--- ${label} (\$ $*) ---"
        if "$@" 2>&1; then
            :
        else
            echo "[command failed or unavailable: $*]"
        fi
    } >>"$REPORT"
}

missing() {
    echo "[skipped: '$1' not found on this system — usually in the '$2' package]" >>"$REPORT"
}

echo "Collecting debug info into ${WORK_DIR} ..."

# ── System ───────────────────────────────────────────────────────────────────
section "System"
run "OS release" cat /etc/os-release
run "Kernel" uname -a
run "systemd version" bash -c 'systemctl --version | head -1'

# ── Package / install info ──────────────────────────────────────────────────
section "Package / install info"
{
    echo
    echo "--- resolved binaries on \$PATH ---"
    for bin in lam-daemon lam-gui lam-hidraw-helper; do
        if path="$(command -v "$bin" 2>/dev/null)"; then
            echo "$bin -> $path"
        else
            echo "$bin -> not found on \$PATH"
        fi
    done
    echo "(if a binary appears in more than one of /usr/bin, /usr/local/bin —"
    echo " e.g. a distro package plus a manual 'make install' — the one"
    echo " earlier in \$PATH wins, which may not be the one you just built)"
} >>"$REPORT"

if have rpm; then
    run "rpm package info" rpm -qa --qf '%{NAME} %{VERSION}-%{RELEASE}\n' linux-arctis-manager linux-arctis-manager-lang linux-arctis-manager-plasma-widget linux-arctis-manager-gnome-extension
elif have dpkg-query; then
    run "dpkg package info" dpkg-query -W -f='${Package} ${Version}\n' 'linux-arctis-manager*'
elif have pacman; then
    run "pacman package info" pacman -Qi linux-arctis-manager
else
    missing "rpm/dpkg-query/pacman" "your distro's package manager"
fi

# ── Live daemon state (D-Bus) ────────────────────────────────────────────────
BUS_NAME="name.giacomofurlan.ArctisManager.Next"
SETTINGS_PATH="/name/giacomofurlan/ArctisManager/Next/Settings"
SETTINGS_IFACE="name.giacomofurlan.ArctisManager.Next.Settings"

section "Live daemon state (D-Bus session bus)"
if have busctl; then
    run "daemon version (GetVersion)" busctl --user call "$BUS_NAME" "$SETTINGS_PATH" "$SETTINGS_IFACE" GetVersion
    run "daemon settings snapshot (GetSettings)" busctl --user call "$BUS_NAME" "$SETTINGS_PATH" "$SETTINGS_IFACE" GetSettings
elif have gdbus; then
    run "daemon version (GetVersion)" gdbus call --session --dest "$BUS_NAME" --object-path "$SETTINGS_PATH" --method "${SETTINGS_IFACE}.GetVersion"
    run "daemon settings snapshot (GetSettings)" gdbus call --session --dest "$BUS_NAME" --object-path "$SETTINGS_PATH" --method "${SETTINGS_IFACE}.GetSettings"
else
    missing "busctl/gdbus" "systemd or glib2 (gdbus)"
fi

# ── systemd units ────────────────────────────────────────────────────────────
section "systemd user units"
if have systemctl; then
    run "unit status" systemctl --user --no-pager status lam-daemon.service lam-hidraw-helper.service
    run "enabled state" systemctl --user is-enabled lam-daemon.service lam-hidraw-helper.service
    run "resolved unit files" systemctl --user --no-pager cat lam-daemon.service lam-hidraw-helper.service
else
    missing "systemctl" "systemd"
fi

section "Recent logs (last 1000 lines per unit)"
if have journalctl; then
    run "lam-daemon.service log" journalctl --user --no-pager -n 1000 -u lam-daemon.service
    run "lam-hidraw-helper.service log" journalctl --user --no-pager -n 1000 -u lam-hidraw-helper.service
else
    missing "journalctl" "systemd"
fi

# ── PipeWire / audio routing ─────────────────────────────────────────────────
# Needed for anything chatmix/redirect/EQ-audible related: the daemon only
# ever adjusts its own virtual sinks (Arctis_Media/Arctis_Chat) — whether that
# change is actually heard depends on whether those sinks exist, have live
# loopbacks to a real output, and are the system's default sink.
section "PipeWire / audio routing"
if have pactl; then
    run "server info (default sink/source)" pactl info
    run "sinks (Arctis_* virtual sinks + physical outputs)" pactl list sinks
    run "sink-inputs (which sink each app is actually playing through)" pactl list sink-inputs
    run "loopback/null-sink modules (module-loopback, module-null-sink)" bash -c \
        "pactl list modules | grep -A2 -E 'Name: module-(loopback|null-sink)'"
else
    missing "pactl" "pulseaudio-utils / pipewire-pulse"
fi
if have wpctl; then
    run "wpctl status (PipeWire graph overview)" wpctl status
else
    missing "wpctl" "wireplumber"
fi

# ── Connected SteelSeries devices ────────────────────────────────────────────
section "Connected SteelSeries USB devices"
if have lsusb; then
    run "lsusb" lsusb -d "${VID}:"
else
    missing "lsusb" "usbutils"
fi

MATCHED_DEVS=()
if have udevadm; then
    {
        echo
        echo "--- /dev/hidraw* nodes matching VID ${VID} ---"
        for dev in /dev/hidraw*; do
            [[ -e "$dev" ]] || continue
            info="$(udevadm info -a -n "$dev" 2>/dev/null)"
            dev_vid="$(grep -m1 'ATTRS{idVendor}'          <<<"$info" | grep -o '"[0-9a-fA-F]*"' | tr -d '"')"
            dev_pid="$(grep -m1 'ATTRS{idProduct}'         <<<"$info" | grep -o '"[0-9a-fA-F]*"' | tr -d '"')"
            dev_if="$(grep -m1  'ATTRS{bInterfaceNumber}'  <<<"$info" | grep -o '"[0-9a-fA-F]*"' | tr -d '"')"
            dev_name="$(grep -m1 'ATTRS{product}'          <<<"$info" | grep -o '"[^"]*"' | head -1)"
            if [[ "${dev_vid,,}" == "${VID,,}" ]]; then
                echo "$dev  PID=0x${dev_pid}  iface=${dev_if}  product=${dev_name}"
                MATCHED_DEVS+=("$dev")
            fi
        done
        if [[ ${#MATCHED_DEVS[@]} -eq 0 ]]; then
            echo "(none found — is the headset/dongle plugged in?)"
        fi
    } >>"$REPORT"
else
    missing "udevadm" "systemd/udev"
fi

# ── HID report descriptors ───────────────────────────────────────────────────
section "HID report descriptors"
if ((${#MATCHED_DEVS[@]} == 0)); then
    echo "[skipped: no matching hidraw devices found above]" >>"$REPORT"
elif ! have hid-recorder; then
    missing "hid-recorder" "hid-tools"
else
    echo "hidraw nodes are root-only — you may be asked for your sudo password." >&2
    if sudo -v; then
        for dev in "${MATCHED_DEVS[@]}"; do
            name="$(basename "$dev")"
            out="${BUNDLE}/hid-descriptors/${name}.txt"
            # hid-recorder prints the descriptor immediately, then blocks
            # streaming live events forever — cut it off after a couple
            # seconds, we only need the descriptor (a few real events are a
            # harmless bonus if the device happens to be chatty).
            sudo timeout 2 hid-recorder "$dev" >"$out" 2>&1
            echo "$dev -> hid-descriptors/${name}.txt" >>"$REPORT"
        done
    else
        echo "[skipped: sudo access unavailable]" >>"$REPORT"
    fi
fi

# ── USB bus-level capture (usbmon) ──────────────────────────────────────────
# Opt-in (--usb-capture): shows both directions of raw USB traffic — every
# command write lam-daemon sends and every reply (or lack of one) the device
# sends back — for whichever bus the SteelSeries device is on. This is the
# tool for "device_init/headset_status never gets a reply" reports: unlike
# the hid-recorder descriptor dump above (which only shows reports arriving
# at ITS OWN reader, not what another process writes), usbmon sees the whole
# URB traffic on the bus regardless of which process/interface it's on.
if [[ -n "$USB_CAPTURE_SECONDS" ]]; then
    section "USB bus-level capture (usbmon)"
    if ! have lsusb; then
        missing "lsusb" "usbutils (needed to find the bus number)"
    else
        BUS="$(lsusb -d "${VID}:" | head -1 | sed -n 's/^Bus \([0-9]\+\).*/\1/p')"
        if [[ -z "$BUS" ]]; then
            echo "[skipped: no SteelSeries device found by lsusb — is it plugged in?]" >>"$REPORT"
        else
            echo "SteelSeries device found on USB bus ${BUS}." >&2
            echo "This needs root — you may be asked for your sudo password." >&2
            echo "Capturing for ${USB_CAPTURE_SECONDS}s starting now — reproduce the" >&2
            echo "issue (headset powered on, daemon running/stuck retrying) during" >&2
            echo "this window." >&2
            if sudo -v; then
                sudo modprobe usbmon 2>/dev/null
                MON_NODE="/sys/kernel/debug/usb/usbmon/${BUS}u"
                if [[ ! -e "$MON_NODE" ]]; then
                    echo "[skipped: ${MON_NODE} not found — debugfs may not be mounted" \
                        "(try: sudo mount -t debugfs none /sys/kernel/debug) or the" \
                        "usbmon kernel module isn't available]" >>"$REPORT"
                else
                    CAP_FILE="${BUNDLE}/usbmon-bus${BUS}.txt"
                    echo "capturing... (this blocks for ${USB_CAPTURE_SECONDS}s)" >&2
                    sudo timeout "${USB_CAPTURE_SECONDS}" cat "$MON_NODE" >"$CAP_FILE" 2>/dev/null
                    sudo chown "$(id -u):$(id -g)" "$CAP_FILE" 2>/dev/null
                    lines="$(wc -l <"$CAP_FILE" 2>/dev/null || echo 0)"
                    {
                        echo
                        echo "--- capture summary ---"
                        echo "bus ${BUS}, ${USB_CAPTURE_SECONDS}s, ${lines} line(s) -> usbmon-bus${BUS}.txt"
                        echo "(one line per URB submit/complete; the SteelSeries device's own"
                        echo " address on this bus may change across replugs — cross-check"
                        echo " against the lsusb/hidraw output above for the run this capture"
                        echo " was taken in, not a prior one)"
                    } >>"$REPORT"
                fi
            else
                echo "[skipped: sudo access unavailable]" >>"$REPORT"
            fi
        fi
    fi
fi

# ── Config files (secrets deliberately excluded) ─────────────────────────────
section "Daemon config files"
CFG_BASE="${XDG_CONFIG_HOME:-$HOME/.config}/arctis_manager"
{
    echo
    echo "Config base: ${CFG_BASE}"
    echo "NOT collected on purpose: hf_token (HuggingFace credential),"
    echo "rvc_models/ (large binaries), calibration/ (audio cache)."
} >>"$REPORT"
if [[ -d "$CFG_BASE" ]]; then
    for f in general_settings.yaml nc_config.json vc_config.json; do
        [[ -f "$CFG_BASE/$f" ]] && cp "$CFG_BASE/$f" "${BUNDLE}/config/" 2>/dev/null
    done
    for sub in settings devices; do
        if [[ -d "$CFG_BASE/$sub" ]]; then
            mkdir -p "${BUNDLE}/config/$sub"
            cp -r "$CFG_BASE/$sub/." "${BUNDLE}/config/$sub/" 2>/dev/null
        fi
    done
    {
        echo
        echo "--- system/user device-config directories present ---"
        for d in "$CFG_BASE/devices" /usr/share/linux-arctis-manager/devices /usr/local/share/linux-arctis-manager/devices; do
            if [[ -d "$d" ]]; then
                echo "$d: $(find "$d" -maxdepth 1 -name '*.yaml' | wc -l) yaml file(s)"
            fi
        done
    } >>"$REPORT"
else
    echo "[skipped: ${CFG_BASE} does not exist]" >>"$REPORT"
fi

# ── Package up ────────────────────────────────────────────────────────────────
ARCHIVE="/tmp/lam-debug-info-$(date +%Y%m%d-%H%M%S).tar.gz"
tar -C "$WORK_DIR" -czf "$ARCHIVE" bundle
rm -rf "$WORK_DIR"

echo
echo "Done. Support bundle written to:"
echo "  ${ARCHIVE}"
echo
echo "Review it before sharing, then attach it to your GitHub issue by hand"
echo "(this script does not upload or send anything itself)."
