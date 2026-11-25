# The icon of the .desktop shortcut: on the panel or dock, and in the menu.
de_show_shortcut
screenshot "launcher--$theme--panel" "$(de_launcher_region)"
de_hide_shortcut
de_show_launcher
screenshot "launcher--$theme--menu"
de_hide_launcher
