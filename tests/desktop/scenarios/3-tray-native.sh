# The desktop's own widget (Plasma widget, GNOME extension), where one exists.
if declare -F de_enable_native >/dev/null; then
    de_enable_native
    screenshot "tray-native--$theme--panel" "$(de_tray_region)"
    de_open_native
    screenshot "tray-native--$theme--popup"
    de_close_native
    de_disable_native
fi
