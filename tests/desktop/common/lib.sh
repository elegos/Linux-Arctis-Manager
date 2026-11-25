# Helpers shared by desktop scripts and scenarios, plus the default of every
# optional desktop hook. A desktop script overrides what it needs.
#
# Every desktop script defines:
#   de_start               bring the desktop up, return once it is usable
#   de_themes              print the names of the theme variants to test
#   de_apply_theme NAME    switch to one of them
#   de_tray_region         print the tray area as WxH+X+Y
#   de_launcher_region     print the area of the panel/dock shortcut as WxH+X+Y
#   de_show_launcher       show the menu or search with the app in it
#   de_hide_launcher       close it again
#
# Optional (defaults below):
#   capture_screen FILE    how to grab the whole screen (default: X11)
#   de_show_shortcut / de_hide_shortcut   if the shortcut is not always visible
#   de_open_tray_menu / de_close_tray_menu  menu of the legacy tray icon
#   de_enable_native / de_disable_native    the desktop's own widget
#   de_open_native / de_close_native        its popup

SCREEN_SIZE=${SCREEN_SIZE:-1280x720}
SCREEN_WIDTH=${SCREEN_SIZE%x*}
SCREEN_HEIGHT=${SCREEN_SIZE#*x}
APP_PID=

log() { echo "[desktop-test] $*"; }

# wait_for <description> <command...>: retries for up to WAIT_TIMEOUT seconds.
wait_for() {
    local what=$1 deadline=$((SECONDS + ${WAIT_TIMEOUT:-60}))
    shift
    until "$@" >/dev/null 2>&1; do
        if ((SECONDS >= deadline)); then
            log "timed out waiting for $what"
            return 1
        fi
        sleep 0.5
    done
}

bus_name_owned() {
    dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.GetNameOwner "string:$1"
}

# A headless X server, or the Xephyr window of the host with --xephyr.
start_x_server() {
    if [[ -n ${XEPHYR_DISPLAY:-} ]]; then
        export DISPLAY=$XEPHYR_DISPLAY
    else
        export DISPLAY=:99
        Xvfb "$DISPLAY" -screen 0 "${SCREEN_SIZE}x24" -nolisten tcp >/out/xvfb.log 2>&1 &
    fi
    wait_for "the X server" xdpyinfo
}

# What `make install` would put in the user's data directories.
install_app_files() {
    local share=$HOME/.local/share
    local icons=$share/icons/hicolor/scalable/apps
    local images=/repo/src/linux_arctis_manager/gui/images
    install -Dm644 "$images/launcher_icon.svg" "$icons/arctis-manager.svg"
    install -Dm644 "$images/app_icon.svg" "$icons/arctis-manager-symbolic.svg"
    install -Dm644 -t "$share/applications" /repo/src/linux_arctis_manager/desktop/*.desktop
    # The .desktop files and the tray shells run `lam-gui`.
    mkdir -p "$HOME/.local/bin"
    printf '#!/bin/sh\nexec python -m linux_arctis_manager.scripts.gui "$@"\n' >"$HOME/.local/bin/lam-gui"
    chmod +x "$HOME/.local/bin/lam-gui"
    export PATH=$HOME/.local/bin:$PATH
}

capture_screen() { import -window root "$1"; }

# screenshot <name> [WxH+X+Y]: whole screen, or the given region enlarged.
screenshot() {
    if [[ -n ${2:-} ]]; then
        capture_screen /tmp/screen.png
        magick /tmp/screen.png -crop "$2" +repage -filter point -resize 400% "/out/$1.png"
    else
        capture_screen "/out/$1.png"
    fi
    log "screenshot: $1"
}

# What applications are told about light/dark: 0 no preference, 1 dark, 2 light.
log_color_scheme() {
    log "portal color-scheme: $(gdbus call --session -d org.freedesktop.portal.Desktop \
        -o /org/freedesktop/portal/desktop -m org.freedesktop.portal.Settings.Read \
        org.freedesktop.appearance color-scheme 2>&1)"
}

# start_app <log name> [lam-gui arguments...]
start_app() {
    local name=$1
    shift
    # The log records which Qt platform theme and colours the app picked up.
    QT_LOGGING_RULES='qt.qpa.theme*=true' lam-gui --no-enforce-systemd -vvvv "$@" >"/out/$name.log" 2>&1 &
    APP_PID=$!
    sleep "${APP_SETTLE:-6}"
    if ! kill -0 "$APP_PID" 2>/dev/null; then
        log "the app exited early, see $name.log"
        return 1
    fi
}

stop_app() {
    [[ -n $APP_PID ]] || return 0
    kill "$APP_PID" 2>/dev/null || true
    wait "$APP_PID" 2>/dev/null || true
    APP_PID=
    sleep 1
}

de_show_shortcut() { :; }
de_hide_shortcut() { :; }
