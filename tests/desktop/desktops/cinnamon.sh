# Cinnamon (X11 session). Cinnamon has no widget of its own for this app.

de_start() {
    export XDG_CURRENT_DESKTOP=X-Cinnamon XDG_SESSION_TYPE=x11 DESKTOP_SESSION=cinnamon
    export LIBGL_ALWAYS_SOFTWARE=1
    start_x_server
    # Portals are D-Bus activated: they need to know the display.
    dbus-update-activation-environment --all
    gsettings set org.cinnamon enabled-applets \
        "['panel1:left:0:menu@cinnamon.org:0', 'panel1:left:1:panel-launchers@cinnamon.org:4', 'panel1:right:0:systray@cinnamon.org:1', 'panel1:right:1:xapp-status@cinnamon.org:2', 'panel1:right:2:calendar@cinnamon.org:3']"
    pin_launcher
    cinnamon --replace >/out/cinnamon.log 2>&1 &
    wait_for "Cinnamon" bus_name_owned org.Cinnamon
    # Normally autostarted by cinnamon-session: bridges StatusNotifierItem
    # icons (what Qt sends) to the panel.
    /usr/libexec/xapps/xapp-sn-watcher >/out/xapp-sn-watcher.log 2>&1 &
    wait_for "the tray watcher" bus_name_owned org.kde.StatusNotifierWatcher
    # Also autostarted by cinnamon-session: publishes the theme to applications.
    /usr/libexec/csd-xsettings >/out/csd-xsettings.log 2>&1 &
    sleep 5
}

# Pins ArctisManager.desktop to the panel launchers applet.
pin_launcher() {
    local dir=$HOME/.config/cinnamon/spices/panel-launchers@cinnamon.org
    local schema=/usr/share/cinnamon/applets/panel-launchers@cinnamon.org/settings-schema.json
    mkdir -p "$dir"
    python - "$schema" >"$dir/4.json" <<'PY'
import json, sys
settings = json.load(open(sys.argv[1]))
for entry in settings.values():
    if isinstance(entry, dict) and 'default' in entry:
        entry['value'] = entry['default']
settings['launcherList']['value'] = ['ArctisManager.desktop']
print(json.dumps(settings))
PY
}

# light: light panel and apps. dark: both dark. mixed: dark panel, light apps.
de_themes() { echo "light dark mixed"; }

de_apply_theme() {
    local panel apps scheme
    case $1 in
        light) panel=Mint-Y apps=Mint-Y scheme=prefer-light ;;
        dark) panel=Mint-Y-Dark apps=Mint-Y-Dark scheme=prefer-dark ;;
        mixed) panel=Mint-Y-Dark apps=Mint-Y scheme=prefer-light ;;
        *) log "unknown theme: $1"; return 1 ;;
    esac
    gsettings set org.cinnamon.theme name "$panel"
    gsettings set org.cinnamon.desktop.interface gtk-theme "$apps"
    gsettings set org.cinnamon.desktop.interface icon-theme Mint-Y
    gsettings set org.x.apps.portal color-scheme "$scheme"
    sleep 3
}

# Both ends of the bottom panel.
de_tray_region() { echo "240x40+$((SCREEN_WIDTH - 240))+$((SCREEN_HEIGHT - 40))"; }
de_launcher_region() { echo "240x40+0+$((SCREEN_HEIGHT - 40))"; }

de_show_launcher() {
    xdotool key super
    sleep 2
    xdotool type --delay 100 arctis
    sleep 2
}

de_hide_launcher() {
    xdotool key Escape
    sleep 1
}

# Right click on the tray icon, the first one left of the clock.
de_open_tray_menu() {
    xdotool mousemove $((SCREEN_WIDTH - 60)) $((SCREEN_HEIGHT - 20)) click 3
    sleep 2
}

de_close_tray_menu() {
    xdotool key Escape
    xdotool mousemove $((SCREEN_WIDTH / 2)) $((SCREEN_HEIGHT / 2))
    sleep 1
}
