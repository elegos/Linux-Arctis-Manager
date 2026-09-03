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
import org.kde.kcmutils as KCM

import "../code/dbus.js" as Dbus
import "../code/i18n.js" as I18n

KCM.SimpleKCM {
    id: page

    property var availableIds: []
    // See main.qml's i18nReady for why this is needed.
    property bool i18nReady: false

    Component.onCompleted: {
        var lang = (Qt.locale().name || "en_US").split("_")[0]
        I18n.init(lang)
        page.i18nReady = true
        Dbus.getSettings(DBus.SessionBus, function (settings) {
            var ids = ["nc_preset"]
            Object.keys((settings || {}).settings_config || {}).forEach(function (k) {
                // Excludes internal/protocol fields with no [settings] or
                // [status] translation entry — e.g. the EQ curve editor's
                // per-band gain1..gainN (settings_config has no way to mark
                // a field as "not meant to be pinned individually", but the
                // lang files never gave these an entry either, since nobody
                // ever intended them to be shown outside that editor).
                if (ids.indexOf(k) === -1 && page.labelFor(k) !== k) ids.push(k)
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

    ColumnLayout {
        width: page.width
        spacing: Kirigami.Units.smallSpacing

        // See ConfigStatusFields.qml for why this is a ColumnLayout and not
        // a Kirigami.FormLayout.
        PC3.Label {
            font.bold: true
            text: page.i18nReady ? I18n.translate("ui", "quick_settings") : ""
        }

        Repeater {
            model: page.availableIds

            delegate: PC3.CheckBox {
                required property string modelData
                text: page.labelFor(modelData)
                checked: page.isPinned(modelData)
                onToggled: page.setPinned(modelData, checked)
            }
        }
    }
}
