.pragma library

// Port of QStatusWidget.format_value()
// (src/linux_arctis_manager/gui/status_widget.py) — keep the two in sync,
// they must never disagree on how a status value is displayed.

function formatValue(I18n, statusKey, statusO, settingsConfig) {
    var val = statusO.value
    var dtype = statusO.type

    if (dtype === "percentage") return val + "%"

    if (dtype === "on_off") return I18n.translate("status_values", val ? "on" : "off")

    if (typeof val === "number" && (dtype === "uint8" || dtype === "uint16" || dtype === "uint32")) {
        var cfg = (settingsConfig && settingsConfig[statusKey]) || {}
        var vm = cfg.values_mapping || {}
        var intKey = String(Math.trunc(val))
        var labelKey = (vm && vm[intKey] !== undefined) ? vm[intKey] : intKey
        return I18n.translate("status_values", labelKey)
    }

    return I18n.translate("status_values", val)
}
