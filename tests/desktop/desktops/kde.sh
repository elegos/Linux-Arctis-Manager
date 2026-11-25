# KDE Plasma (Wayland session) on KWin's headless backend.

PLASMOID=name.giacomofurlan.arctismanager

# Runs a script in the running Plasma shell.
plasma_script() {
    dbus-send --session --print-reply --dest=org.kde.plasmashell /PlasmaShell \
        org.kde.PlasmaShell.evaluateScript "string:$1" >>/out/plasma-script.log
}

install_plasmoid() {
    local dest=$HOME/.local/share/plasma/plasmoids/$PLASMOID
    mkdir -p "$dest"
    cp -r /repo/packaging/plasma6/$PLASMOID/. "$dest/"
    # What `make generate-plasmoid-lang` does.
    mkdir -p "$dest/contents/code/lang"
    python - /repo/src/linux_arctis_manager/lang "$dest/contents/code/lang" <<'PY'
import json, sys
from pathlib import Path
for ini in Path(sys.argv[1]).glob('*.ini'):
    text = ini.read_text(encoding='utf-8')
    (Path(sys.argv[2]) / f'{ini.stem}.js').write_text('.pragma library\nvar TEXT = ' + json.dumps(text) + '\n')
PY
    # Pin some quick settings: the popup would otherwise be half empty.
    sed -i '/name="pinnedSettings"/a <default>mic_volume,mic_side_tone</default>' "$dest/contents/config/main.xml"
}

de_start() {
    [[ -e /dev/dri/renderD128 ]] || { log "KDE needs a GPU render node (/dev/dri/renderD128)"; return 1; }
    export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland XDG_MENU_PREFIX=plasma-
    export KDE_FULL_SESSION=true KDE_SESSION_VERSION=6
    export KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1
    install_plasmoid
    # KWin runs this once it is up: it is the only way to learn the X display
    # and cookie of its Xwayland, which the legacy tray icon runs on.
    printf '#!/bin/sh\nprintf "export DISPLAY=%%s XAUTHORITY=%%s\\n" "$DISPLAY" "$XAUTHORITY" >/tmp/xwayland.env\n' >/tmp/dump-env.sh
    chmod +x /tmp/dump-env.sh
    kwin_wayland --virtual --width "$SCREEN_WIDTH" --height "$SCREEN_HEIGHT" --xwayland \
        --socket wayland-test /tmp/dump-env.sh >/out/kwin.log 2>&1 &
    wait_for "KWin" test -s /tmp/xwayland.env
    # shellcheck source=/dev/null
    source /tmp/xwayland.env
    export WAYLAND_DISPLAY=wayland-test
    dbus-update-activation-environment --all
    kbuildsycoca6 >/out/kbuildsycoca.log 2>&1
    plasmashell >/out/plasmashell.log 2>&1 &
    wait_for "Plasma" bus_name_owned org.kde.plasmashell
    wait_for "the tray watcher" bus_name_owned org.kde.StatusNotifierWatcher
    sleep 20
    pkill plasma-welcome || true
    plasma_script 'panels().forEach(p => p.widgets().forEach(w => {
        if (w.type == "org.kde.plasma.icontasks") {
            w.currentConfigGroup = ["General"];
            w.writeConfig("launchers", ["applications:ArctisManager.desktop"]);
            w.reloadConfig();
        }
    }))'
    sleep 3
}

capture_screen() { /usr/bin/python3 /repo/tests/desktop/common/kwin_screenshot.py "$1"; }

de_themes() { echo "light dark mixed"; }

# The global theme, then its panel style and colours explicitly: applying a
# global theme alone does not always repaint the panel.
de_apply_theme() {
    local theme panel colors
    case $1 in
        light) theme=org.kde.breeze.desktop panel=breeze-light colors=BreezeLight ;;
        dark) theme=org.kde.breezedark.desktop panel=breeze-dark colors=BreezeDark ;;
        mixed) theme=org.kde.breezetwilight.desktop panel=breeze-dark colors=BreezeLight ;;
        *) log "unknown theme: $1"; return 1 ;;
    esac
    {
        plasma-apply-lookandfeel -a "$theme"
        plasma-apply-desktoptheme "$panel"
        plasma-apply-colorscheme "$colors"
    } >>/out/theme.log 2>&1
    sleep 6
}

de_tray_region() { echo "320x56+$((SCREEN_WIDTH - 320))+$((SCREEN_HEIGHT - 56))"; }
de_launcher_region() { echo "320x56+0+$((SCREEN_HEIGHT - 56))"; }

# KRunner searching for the app.
de_show_launcher() {
    dbus-send --session --print-reply --dest=org.kde.krunner /App org.kde.krunner.App.query string:arctis >/dev/null
    sleep 4
}

de_hide_launcher() {
    pkill krunner || true
    sleep 1
}

de_enable_native() {
    plasma_script "panels()[0].addWidget('$PLASMOID')"
    sleep 5
}

de_disable_native() {
    plasma_script "panels().forEach(p => p.widgets('$PLASMOID').forEach(w => w.remove()))"
    sleep 2
}

# The popup's content in a window of its own: nothing can click on the panel.
de_open_native() {
    plasmawindowed "$PLASMOID" >/out/plasmawindowed.log 2>&1 &
    NATIVE_PID=$!
    sleep 6
}

de_close_native() {
    kill "$NATIVE_PID" 2>/dev/null || true
    sleep 1
}
