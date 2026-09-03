// "Configure..." page: pick which status fields are shown in the widget's
// status pane. Field list is queried live from the daemon (GetStatus), since
// it depends on the connected device. Note: unchecking every field reverts
// to "show all" (the empty-list default meaning), not "show none" — a
// deliberate simplification, not a bug.

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

    property var fieldKeys: []

    Component.onCompleted: {
        I18n.init()
        Dbus.getStatus(DBus.SessionBus, function (status) {
            var keys = []
            Object.keys(status || {}).forEach(function (category) {
                Object.keys(status[category] || {}).forEach(function (k) {
                    if (keys.indexOf(k) === -1) keys.push(k)
                })
            })
            var configured = Plasmoid.configuration.enabledStatusFields || []
            configured.forEach(function (k) {
                if (keys.indexOf(k) === -1) keys.push(k)
            })
            page.fieldKeys = keys
        })
    }

    function isEnabled(key) {
        var configured = Plasmoid.configuration.enabledStatusFields || []
        return configured.length === 0 || configured.indexOf(key) !== -1
    }

    function setEnabled(key, on) {
        var configured = (Plasmoid.configuration.enabledStatusFields || []).slice()
        if (configured.length === 0) configured = page.fieldKeys.slice()
        var idx = configured.indexOf(key)
        if (on && idx === -1) configured.push(key)
        if (!on && idx !== -1) configured.splice(idx, 1)
        Plasmoid.configuration.enabledStatusFields = configured
    }

    PC3.Label {
        Kirigami.FormData.isSection: true
        text: I18n.translate("ui", "status")
    }

    Repeater {
        model: page.fieldKeys

        delegate: PC3.CheckBox {
            required property string modelData
            Kirigami.FormData.label: ""
            text: {
                var label = I18n.translate("status", modelData)
                return label === modelData ? modelData : label
            }
            checked: page.isEnabled(modelData)
            onToggled: page.setEnabled(modelData, checked)
        }
    }
}
