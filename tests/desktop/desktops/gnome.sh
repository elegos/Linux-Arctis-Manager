# GNOME Shell (Wayland only) in headless mode. The legacy tray icon needs the
# AppIndicator extension; the app's own extension is the native widget.

EXTENSION=arctis-manager@giacomofurlan.name
APPINDICATOR=appindicatorsupport@rgcjonas.gmail.com

# The screenshots are wanted even when the extension does not declare support
# for this GNOME release yet, but that must not go unnoticed.
check_extension_version() {
    local version
    version=$(gnome-shell --version | sed 's/[^0-9]*\([0-9]*\).*/\1/')
    if ! grep -q "\"$version\"" "/repo/packaging/gnome-shell/$EXTENSION/metadata.json"; then
        log "WARNING: the extension does not declare GNOME $version in metadata.json: users get it as out of date. Version check disabled for this run."
        gsettings set org.gnome.shell disable-extension-version-validation true
    fi
}

# Runs JavaScript in the shell (allowed by --unsafe-mode).
shell_eval() {
    local reply
    reply=$(gdbus call --session -d org.gnome.Shell -o /org/gnome/Shell -m org.gnome.Shell.Eval "$1")
    echo "$1 -> $reply" >>/out/shell-eval.log
    [[ $reply == "(true,"* ]]
}

install_extension() {
    local dest=$HOME/.local/share/gnome-shell/extensions/$EXTENSION
    mkdir -p "$dest"
    cp -r /repo/packaging/gnome-shell/$EXTENSION/. "$dest/"
    glib-compile-schemas "$dest/schemas"
    GSETTINGS_SCHEMA_DIR=$dest/schemas gsettings set name.giacomofurlan.arctismanager pinned-settings \
        "['mic_volume', 'mic_side_tone']"
}

de_start() {
    [[ -e /dev/dri/renderD128 ]] || { log "GNOME needs a GPU render node (/dev/dri/renderD128)"; return 1; }
    export XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland
    install_extension
    check_extension_version
    gsettings set org.gnome.shell welcome-dialog-last-shown-version 999
    gsettings set org.gnome.shell enabled-extensions "['$APPINDICATOR']"
    gsettings set org.gnome.shell favorite-apps "['ArctisManager.desktop']"
    gsettings set org.gnome.desktop.notifications show-banners false
    gsettings set org.gnome.desktop.interface enable-animations false
    gnome-shell --headless --unsafe-mode --virtual-monitor "$SCREEN_SIZE" \
        --wayland-display wayland-test >/out/gnome-shell.log 2>&1 &
    wait_for "GNOME Shell" shell_eval 'Main.layoutManager._startingUp === false'
    wait_for "the tray watcher" bus_name_owned org.kde.StatusNotifierWatcher
    export WAYLAND_DISPLAY=wayland-test
    # Xwayland, for the legacy tray icon: the shell announces its display.
    DISPLAY=$(sed -n 's/.*Using public X11 display \(:[0-9]*\).*/\1/p' /out/gnome-shell.log | head -1)
    XAUTHORITY=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name '.mutter-Xwaylandauth.*' | head -1)
    export DISPLAY XAUTHORITY
    # Portals are D-Bus activated: they need to know the display.
    dbus-update-activation-environment --all
    sleep 3
    shell_eval 'Main.overview.hide()'
    sleep 1
}

capture_screen() {
    gdbus call --session -d org.gnome.Shell -o /org/gnome/Shell/Screenshot \
        -m org.gnome.Shell.Screenshot.Screenshot false false "$1" >/dev/null
}

# "mixed" is GNOME's default style: dark panel, light apps.
de_themes() { echo "light dark mixed"; }

de_apply_theme() {
    local scheme
    case $1 in
        light) scheme=prefer-light ;;
        dark) scheme=prefer-dark ;;
        mixed) scheme=default ;;
        *) log "unknown theme: $1"; return 1 ;;
    esac
    gsettings set org.gnome.desktop.interface color-scheme "$scheme"
    sleep 3
}

de_tray_region() { echo "320x32+$((SCREEN_WIDTH - 320))+0"; }

# The shortcut lives in the overview's dash, at the bottom centre.
de_show_shortcut() {
    shell_eval 'Main.overview.show()'
    sleep 2
}
de_hide_shortcut() {
    shell_eval 'Main.overview.hide()'
    sleep 1
}
de_launcher_region() { echo "240x112+$((SCREEN_WIDTH / 2 - 120))+$((SCREEN_HEIGHT - 112))"; }

de_show_launcher() {
    shell_eval 'Main.overview.show()'
    sleep 1
    shell_eval 'Main.overview.searchEntry.set_text("arctis")'
    sleep 3
}
de_hide_launcher() {
    shell_eval 'Main.overview.searchEntry.set_text("")'
    de_hide_shortcut
}

appindicator_menu() {
    shell_eval "Main.panel.statusArea[Object.keys(Main.panel.statusArea).find(k => k.startsWith('appindicator'))].menu.$1()"
}
de_open_tray_menu() { appindicator_menu open; sleep 2; }
de_close_tray_menu() { appindicator_menu close; sleep 1; }

de_enable_native() {
    gnome-extensions enable "$EXTENSION"
    sleep 4
    gnome-extensions info "$EXTENSION" >/out/extension-info.log 2>&1
    wait_for "the app's extension, see extension-info.log" shell_eval \
        "if (!Main.panel.statusArea['$EXTENSION']) throw new Error('not loaded')"
}
de_disable_native() {
    gnome-extensions disable "$EXTENSION"
    sleep 2
}
de_open_native() {
    shell_eval "Main.panel.statusArea['$EXTENSION'].menu.open()"
    sleep 3
}
de_close_native() {
    shell_eval "Main.panel.statusArea['$EXTENSION'].menu.close()"
    sleep 1
}
