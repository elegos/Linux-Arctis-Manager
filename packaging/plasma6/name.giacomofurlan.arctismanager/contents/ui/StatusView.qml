// Read-only status list — mirrors QStatusWidget
// (src/linux_arctis_manager/gui/status_widget.py). Renders whatever
// GetStatus reports, filtered by `enabledFields` when non-empty.

import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC3

import "../code/format.js" as Format

ColumnLayout {
    id: root

    // { categoryName: { fieldName: {value, type} } }
    property var status: ({})
    property var settingsConfig: ({})
    // Empty/undefined = show every field (default behaviour).
    property var enabledFields: []
    property var i18n

    spacing: Kirigami.Units.smallSpacing

    function fieldEnabled(name) {
        return !root.enabledFields || root.enabledFields.length === 0
            || root.enabledFields.indexOf(name) !== -1
    }

    PC3.Label {
        visible: root.status === null || Object.keys(root.status).length === 0
        text: root.i18n ? root.i18n.translate("ui", "no_device_detected") : ""
        font.bold: true
    }

    Repeater {
        model: root.status ? Object.keys(root.status) : []

        delegate: ColumnLayout {
            id: categoryColumn
            required property string modelData
            readonly property var fields: root.status[modelData] || {}
            // Mirrors status_widget.py's skip_fields: hide the transparency
            // level slider unless transparency mode is actually "transparent".
            readonly property bool hideTransparentLevel:
                (fields.transparency_mode && fields.transparency_mode.value) !== "transparent"
            readonly property var visibleKeys: Object.keys(fields).filter(function (k) {
                if (k === "transparent_level" && categoryColumn.hideTransparentLevel) return false
                return root.fieldEnabled(k)
            })

            Layout.fillWidth: true
            visible: visibleKeys.length > 0
            spacing: 2

            PC3.Label {
                text: root.i18n ? root.i18n.translate("status", categoryColumn.modelData) : categoryColumn.modelData
                font.bold: true
                font.pointSize: Kirigami.Theme.defaultFont.pointSize * 1.15
            }

            Repeater {
                model: categoryColumn.visibleKeys

                delegate: PC3.Label {
                    required property string modelData
                    Layout.fillWidth: true
                    text: (root.i18n ? root.i18n.translate("status", modelData) : modelData)
                        + ": " + Format.formatValue(root.i18n, modelData, categoryColumn.fields[modelData], root.settingsConfig)
                }
            }
        }
    }
}
