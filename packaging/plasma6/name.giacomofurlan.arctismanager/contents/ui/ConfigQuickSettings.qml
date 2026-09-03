// "Configure..." page: pick which settings are pinned as quick-access
// controls. Mirrors the removed tray_quick_settings_editor.py's
// _available_ids(): "nc_preset" (synthetic) plus every key GetSettings
// reports in settings_config.

import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC3
import org.kde.plasma.plasmoid
import org.kde.plasma.workspace.dbus as DBus

import "../code/dbus.js" as Dbus
import "../code/i18n.js" as I18n

Kirigami.FormLayout {
    id: page

    property var availableIds: []

    Component.onCompleted: {
        I18n.init()
        Dbus.getSettings(DBus.SessionBus, function (settings) {
            var ids = ["nc_preset"]
            Object.keys((settings || {}).settings_config || {}).forEach(function (k) {
                if (ids.indexOf(k) === -1) ids.push(k)
            })
            page.availableIds = ids
        })
    }

    function isPinned(id) {
        return (Plasmoid.configuration.pinnedSettings || []).indexOf(id) !== -1
    }

    function setPinned(id, on) {
        var pinned = (Plasmoid.configuration.pinnedSettings || []).slice()
        var idx = pinned.indexOf(id)
        if (on && idx === -1) pinned.push(id)
        if (!on && idx !== -1) pinned.splice(idx, 1)
        Plasmoid.configuration.pinnedSettings = pinned
    }

    function labelFor(id) {
        if (id === "nc_preset") return I18n.translate("ui", "nc")
        var label = I18n.translate("settings", id)
        if (label === id) label = I18n.translate("status", id)
        return label === id ? id : label
    }

    PC3.Label {
        Kirigami.FormData.isSection: true
        text: I18n.translate("ui", "quick_settings")
    }

    Repeater {
        model: page.availableIds

        delegate: PC3.CheckBox {
            required property string modelData
            Kirigami.FormData.label: ""
            text: page.labelFor(modelData)
            checked: page.isPinned(modelData)
            onToggled: page.setPinned(modelData, checked)
        }
    }
}
