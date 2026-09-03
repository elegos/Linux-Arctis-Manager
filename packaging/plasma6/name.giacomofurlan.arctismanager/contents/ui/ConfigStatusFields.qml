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
import org.kde.kcmutils as KCM

import "../code/dbus.js" as Dbus
import "../code/i18n.js" as I18n

KCM.SimpleKCM {
    id: page

    // Flat union of every key across categories, kept alongside `categories`
    // below only for setEnabled()'s "unchecking the first item seeds the
    // full list" fallback (see there) — rendering uses `categories`.
    property var fieldKeys: []
    // [{name: categoryName, keys: [...]}, ...], plus one trailing
    // {name: null, keys: [...]} bucket for previously-enabled fields that
    // aren't in the live status (no device connected right now) — keeps
    // them toggleable without knowing which category they used to belong
    // to.
    property var categories: []
    // See main.qml's i18nReady for why this is needed: a binding that calls
    // I18n.translate() before I18n.init() below has run (as the section
    // label's does, since it's a direct child evaluated at construction)
    // gets stuck on the pre-init value forever otherwise.
    property bool i18nReady: false

    Component.onCompleted: {
        var lang = (Qt.locale().name || "en_US").split("_")[0]
        I18n.init(lang)
        page.i18nReady = true
        Dbus.getStatus(DBus.SessionBus, function (status) {
            var keys = []
            var cats = []
            Object.keys(status || {}).forEach(function (category) {
                var catKeys = Object.keys(status[category] || {})
                if (catKeys.length === 0) return
                cats.push({ name: category, keys: catKeys })
                catKeys.forEach(function (k) {
                    if (keys.indexOf(k) === -1) keys.push(k)
                })
            })
            var configured = Plasmoid.configuration.enabledStatusFields || []
            var leftover = configured.filter(function (k) { return keys.indexOf(k) === -1 })
            leftover.forEach(function (k) { keys.push(k) })
            if (leftover.length > 0) cats.push({ name: null, keys: leftover })
            page.fieldKeys = keys
            page.categories = cats
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

    ColumnLayout {
        width: page.width
        spacing: Kirigami.Units.smallSpacing

        // Kirigami.FormLayout is the wrong fit here and was tried first: it
        // hardcodes Layout.fillWidth: true on itself, then centers its own
        // content within that (anchors.horizontalCenter, see
        // org/kde/kirigami/layouts/FormLayout.qml) — deliberate for
        // label:field forms, but every row below has no label
        // (Kirigami.FormData.label would've been "") so there's nothing to
        // form-align in the first place, just a left-aligned checkbox list.
        PC3.Label {
            font.bold: true
            text: page.i18nReady ? I18n.translate("ui", "status") : ""
        }

        Repeater {
            model: page.categories

            delegate: ColumnLayout {
                id: categoryColumn
                required property var modelData
                Layout.fillWidth: true
                spacing: 2

                PC3.Label {
                    visible: categoryColumn.modelData.name !== null
                    font.bold: true
                    // Category names (bluetooth/headset/mic/...) live in the
                    // same [status] section as the field names inside them —
                    // mirrors StatusView.qml's category headers in the popup.
                    text: categoryColumn.modelData.name !== null
                        ? I18n.translate("status", categoryColumn.modelData.name) : ""
                }

                Repeater {
                    model: categoryColumn.modelData.keys

                    delegate: PC3.CheckBox {
                        required property string modelData
                        Layout.leftMargin: Kirigami.Units.gridUnit
                        text: {
                            var label = I18n.translate("status", modelData)
                            return label === modelData ? modelData : label
                        }
                        checked: page.isEnabled(modelData)
                        onToggled: page.setEnabled(modelData, checked)
                    }
                }
            }
        }
    }
}
