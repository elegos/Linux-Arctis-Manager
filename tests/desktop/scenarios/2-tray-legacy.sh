# The legacy tray icon (`lam-gui --systray`) and, where it can be opened, its
# menu. The app starts anew in each theme, as it picks its colours at startup.
start_app "tray-legacy-$theme" --systray
screenshot "tray-legacy--$theme--panel" "$(de_tray_region)"
if declare -F de_open_tray_menu >/dev/null; then
    de_open_tray_menu
    screenshot "tray-legacy--$theme--menu"
    de_close_tray_menu
fi
stop_app
