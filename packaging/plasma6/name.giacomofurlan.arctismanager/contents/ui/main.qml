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
    // I18n.translate() reads a plain JS module variable, not a notifiable
    // QML property — a binding that calls it (Plasmoid.title below) gets
    // exactly one evaluation, at construction, which happens *before*
    // Component.onCompleted below ever runs I18n.init(). Reading this
    // property from that binding forces a re-evaluation once translations
    // have actually loaded; without it the title is permanently stuck on
    // whatever translate() returned pre-init (the raw "app_name" key).
    property bool i18nReady: false

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

    // "-symbolic" gets Kirigami/Plasma's automatic panel recoloring
    // (light icon on a dark panel, dark on light) — the plain
    // "arctis-manager" name is a fixed-color icon meant for the app
    // launcher/window, not the tray.
    Plasmoid.icon: "arctis-manager-symbolic"
    Plasmoid.title: root.i18nReady ? I18n.translate("ui", "app_name") : ""

    Component.onCompleted: {
        var lang = (Qt.locale().name || "en_US").split("_")[0]
        I18n.init(lang)
        root.i18nReady = true
        refresh()
    }

    function refresh() {
        Dbus.getStatus(DBus.SessionBus, function (s) { root.status = s || {} })
        Dbus.getSettings(DBus.SessionBus, function (s) { root.settingsPayload = s || {} })
        Dbus.getNcSettings(DBus.SessionBus, function (s) { root.ncSettings = s || {} })
    }

    Timer {
        // Signal-driven updates below make this a fallback, not the primary
        // path — but still needed: it's what refreshes on open
        // (triggeredOnStart) and covers anything a missed/dropped D-Bus
        // signal would otherwise leave stale.
        interval: Plasmoid.configuration.refreshIntervalMs
        running: root.expanded
        repeat: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }

    // DBus.SignalWatcher's onReceivedSignal is a plain invokable method, not
    // an actual Signal (confirmed with qmllint: "no matching signal found
    // for handler" on a direct `onReceivedSignal:` binding — that's what the
    // previous polling-only version's comment here meant by "couldn't be
    // verified"). Connections' function-based override works: Connections
    // can bind to an invokable method the same way it binds to a signal.
    DBus.SignalWatcher {
        id: statusWatcher
        service: Dbus.BUS_NAME
        path: Dbus.STATUS_PATH
        iface: Dbus.STATUS_IFACE
    }
    Connections {
        target: statusWatcher
        function onReceivedSignal(message) {
            if (message.member === "StatusChanged") {
                Dbus.getStatus(DBus.SessionBus, function (s) { root.status = s || {} })
            }
        }
    }

    DBus.SignalWatcher {
        id: settingsWatcher
        service: Dbus.BUS_NAME
        path: Dbus.SETTINGS_PATH
        iface: Dbus.SETTINGS_IFACE
    }
    Connections {
        target: settingsWatcher
        function onReceivedSignal(message) {
            if (message.member === "SettingsChanged") {
                Dbus.getSettings(DBus.SessionBus, function (s) { root.settingsPayload = s || {} })
            }
        }
    }

    DBus.SignalWatcher {
        id: ncWatcher
        service: Dbus.BUS_NAME
        path: Dbus.NC_PATH
        iface: Dbus.NC_IFACE
    }
    Connections {
        target: ncWatcher
        function onReceivedSignal(message) {
            if (message.member === "NCChanged") {
                Dbus.getNcSettings(DBus.SessionBus, function (s) { root.ncSettings = s || {} })
            }
        }
    }

    compactRepresentation: Kirigami.Icon {
        source: "arctis-manager-symbolic"
        // Explicit, not relying on the "-symbolic" name-suffix heuristic:
        // makes the icon render as a mask tinted with the theme's text
        // color, so it follows the panel's light/dark color scheme.
        isMask: true
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
