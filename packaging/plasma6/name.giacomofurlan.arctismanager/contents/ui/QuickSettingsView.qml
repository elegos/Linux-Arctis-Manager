// Pinned quick-access controls — mirrors the removed
// tray_quick_settings_tab.py's control building (recovered via
// `git show HEAD~1:src/linux_arctis_manager/gui/tray_quick_settings_tab.py`
// before it was deleted). One row per id in `pinnedSettings`; "nc_preset" is
// a synthetic entry (not a real daemon setting) that maps to the NC
// interface's preset instead of Settings.SetSetting.

import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC3

import "../code/dbus.js" as Dbus

ColumnLayout {
    id: root

    property var pinnedSettings: []
    // { settingId: {type, values, min, max, step, values_mapping, options_source, ...} }
    property var settingsConfig: ({})
    property var currentValues: ({})
    property string ncPreset: "off"
    property var dbusConnection
    property var i18n
    // { listName: [{id, name/description}, ...] } — filled in lazily via GetListOptions.
    property var optionLists: ({})

    readonly property var ncPresets: [
        ["off", "nc_preset_off"],
        ["light", "nc_preset_light"],
        ["standard", "nc_preset_standard"],
        ["studio", "nc_preset_studio"],
    ]

    spacing: Kirigami.Units.smallSpacing

    function _t(section, key) {
        return root.i18n ? root.i18n.translate(section, key) : key
    }

    function labelFor(sid) {
        if (sid === "nc_preset") return _t("ui", "nc")
        var label = _t("settings", sid)
        if (label === sid) label = _t("status", sid)
        return label
    }

    function isRenderable(sid) {
        if (sid === "nc_preset") return true
        var cfg = root.settingsConfig[sid]
        if (!cfg) return false
        if (cfg.type === "select") {
            var src = cfg.options_source
            return !src || root.optionLists[src] !== undefined
        }
        return true
    }

    function ensureOptions(sid) {
        var cfg = root.settingsConfig[sid]
        if (!cfg || cfg.type !== "select") return
        var src = cfg.options_source
        if (!src || root.optionLists[src] !== undefined) return
        Dbus.getListOptions(root.dbusConnection, src, function (list) {
            var next = Object.assign({}, root.optionLists)
            next[src] = list
            root.optionLists = next
        }, function () {
            var next = Object.assign({}, root.optionLists)
            next[src] = []
            root.optionLists = next
        })
    }

    onPinnedSettingsChanged: pinnedSettings.forEach(ensureOptions)
    onSettingsConfigChanged: pinnedSettings.forEach(ensureOptions)

    PC3.Label {
        visible: root.pinnedSettings.length === 0
        Layout.fillWidth: true
        wrapMode: Text.WordWrap
        text: root._t("ui", "quick_settings_empty")
        opacity: 0.7
    }

    Repeater {
        model: root.pinnedSettings.filter(root.isRenderable)

        delegate: ColumnLayout {
            id: row
            required property string modelData
            readonly property string sid: modelData
            readonly property var cfg: root.settingsConfig[sid] || ({})

            Layout.fillWidth: true
            spacing: 2

            PC3.Label {
                text: root.labelFor(row.sid)
                opacity: 0.7
                font.pointSize: Kirigami.Theme.smallFont.pointSize
            }

            // ── nc_preset ────────────────────────────────────────────────
            PC3.ComboBox {
                Layout.fillWidth: true
                visible: row.sid === "nc_preset"
                model: root.ncPresets.map(function (p) { return root._t("ui", p[1]) })
                currentIndex: {
                    for (var i = 0; i < root.ncPresets.length; i++)
                        if (root.ncPresets[i][0] === root.ncPreset) return i
                    return 0
                }
                onActivated: function (index) {
                    var preset = root.ncPresets[index][0]
                    Dbus.setNcSettings(root.dbusConnection, { preset: preset })
                }
            }

            // ── toggle ───────────────────────────────────────────────────
            PC3.Switch {
                visible: row.sid !== "nc_preset" && row.cfg.type === "toggle"
                checked: root.currentValues[row.sid] === ((row.cfg.values && row.cfg.values.on) !== undefined ? row.cfg.values.on : true)
                onToggled: {
                    var onVal = (row.cfg.values && row.cfg.values.on !== undefined) ? row.cfg.values.on : true
                    var offVal = (row.cfg.values && row.cfg.values.off !== undefined) ? row.cfg.values.off : false
                    Dbus.setSetting(root.dbusConnection, row.sid, checked ? onVal : offVal)
                }
            }

            // ── slider ───────────────────────────────────────────────────
            RowLayout {
                visible: row.sid !== "nc_preset" && row.cfg.type === "slider"
                Layout.fillWidth: true

                PC3.Slider {
                    id: slider
                    Layout.fillWidth: true
                    from: row.cfg.min !== undefined ? row.cfg.min : 0
                    to: row.cfg.max !== undefined ? row.cfg.max : 100
                    stepSize: row.cfg.step !== undefined ? row.cfg.step : 1

                    // Not a continuous `value:` binding on purpose — that
                    // would fight the user mid-drag every time a poll
                    // refreshes currentValues. Only resync while not
                    // pressed, same as the old Qt widget's
                    // blockSignals()-during-external-update pattern.
                    function syncFromModel() {
                        if (slider.pressed) return
                        var v = root.currentValues[row.sid]
                        slider.value = v !== undefined ? v : slider.from
                    }

                    Component.onCompleted: syncFromModel()
                    Connections {
                        target: root
                        function onCurrentValuesChanged() { slider.syncFromModel() }
                    }

                    onPressedChanged: if (!pressed) Dbus.setSetting(root.dbusConnection, row.sid, Math.round(value))
                }
                PC3.Label {
                    text: Math.round(slider.value)
                    Layout.minimumWidth: Kirigami.Units.gridUnit * 1.5
                }
            }

            // ── discrete_map ─────────────────────────────────────────────
            PC3.ComboBox {
                id: discreteCombo
                visible: row.sid !== "nc_preset" && row.cfg.type === "discrete_map"
                Layout.fillWidth: true
                readonly property var orderedKeys: Object.keys(row.cfg.values_mapping || {}).sort(function (a, b) { return parseInt(a) - parseInt(b) })
                model: orderedKeys.map(function (k) { return root._t("settings_values", row.cfg.values_mapping[k]) })
                currentIndex: {
                    var raw = root.currentValues[row.sid]
                    for (var i = 0; i < orderedKeys.length; i++)
                        if (parseInt(orderedKeys[i]) === parseInt(raw)) return i
                    return 0
                }
                onActivated: function (index) {
                    Dbus.setSetting(root.dbusConnection, row.sid, parseInt(orderedKeys[index]))
                }
            }

            // ── select (options fetched via GetListOptions) ─────────────
            PC3.ComboBox {
                id: selectCombo
                visible: row.sid !== "nc_preset" && row.cfg.type === "select"
                Layout.fillWidth: true
                readonly property var options: root.optionLists[row.cfg.options_source] || []
                model: options.map(function (o) { return o.name || o.description || String(o.id) })
                currentIndex: {
                    var raw = root.currentValues[row.sid]
                    for (var i = 0; i < options.length; i++)
                        if (options[i].id === raw) return i
                    return -1
                }
                onActivated: function (index) {
                    Dbus.setSetting(root.dbusConnection, row.sid, options[index].id)
                }
            }
        }
    }
}
