import QtQuick
import QtQuick.Controls as QQC2
import org.kde.kirigami as Kirigami
import org.kde.plasma.plasmoid
import org.kde.plasma.workspace.dbus as DBus

import "../code/dbus.js" as Dbus
import "../code/i18n.js" as I18n

PlasmoidItem {
    id: root

    property var status: ({})
    property var settingsPayload: ({})
    property var ncSettings: ({})

    readonly property var settingsConfig: root.settingsPayload.settings_config || ({})
    readonly property var currentValues: {
        var values = {}
        Object.keys(root.status).forEach(function (category) {
            Object.keys(root.status[category] || {}).forEach(function (k) {
                values[k] = root.status[category][k].value
            })
        })
        // GetSettings' "device" section covers settings that aren't part of
        // the status representation too (mirrors tray_quick_settings_tab.py's
        // update_settings()).
        Object.keys(root.settingsPayload.device || {}).forEach(function (k) {
            var v = root.settingsPayload.device[k]
            if (v !== null && v !== undefined) values[k] = v
        })
        return values
    }

    Plasmoid.icon: "arctis-manager"
    Plasmoid.title: I18n.translate("ui", "app_name")

    Component.onCompleted: {
        var lang = (Qt.locale().name || "en_US").split("_")[0]
        I18n.init(lang)
        refresh()
    }

    function refresh() {
        Dbus.getStatus(DBus.SessionBus, function (s) { root.status = s || {} })
        Dbus.getSettings(DBus.SessionBus, function (s) { root.settingsPayload = s || {} })
        Dbus.getNcSettings(DBus.SessionBus, function (s) { root.ncSettings = s || {} })
    }

    Timer {
        // Polling, not DBus.SignalWatcher — see the plan's note on why:
        // the daemon does emit StatusChanged/SettingsChanged/NCChanged, but
        // SignalWatcher's onReceivedSignal binding couldn't be verified in
        // the dev sandbox this was written in. Switch to signal-driven
        // updates once confirmed working live, keep this as a safe fallback.
        interval: Plasmoid.configuration.refreshIntervalMs
        running: root.expanded
        repeat: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }

    compactRepresentation: Kirigami.Icon {
        source: "arctis-manager"
        active: mouseArea.containsMouse

        MouseArea {
            id: mouseArea
            anchors.fill: parent
            hoverEnabled: true
            onClicked: root.expanded = !root.expanded
        }
    }

    fullRepresentation: Item {
        implicitWidth: Kirigami.Units.gridUnit * 22
        implicitHeight: Kirigami.Units.gridUnit * 28

        QQC2.SplitView {
            anchors.fill: parent
            orientation: Qt.Vertical

            QQC2.ScrollView {
                QQC2.SplitView.fillHeight: true
                QQC2.SplitView.minimumHeight: Kirigami.Units.gridUnit * 6

                StatusView {
                    width: parent.width
                    status: root.status
                    settingsConfig: root.settingsConfig
                    enabledFields: Plasmoid.configuration.enabledStatusFields
                    i18n: I18n
                }
            }

            QQC2.ScrollView {
                QQC2.SplitView.minimumHeight: Kirigami.Units.gridUnit * 6

                QuickSettingsView {
                    width: parent.width
                    pinnedSettings: Plasmoid.configuration.pinnedSettings
                    settingsConfig: root.settingsConfig
                    currentValues: root.currentValues
                    ncPreset: root.ncSettings.preset || "off"
                    dbusConnection: DBus.SessionBus
                    i18n: I18n
                }
            }
        }
    }
}
