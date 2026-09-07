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

// Port of ascii_bar.py's battery_bar/chatmix_bar
// (src/linux_arctis_manager/gui/ascii_bar.py) — keep the two in sync.

function batteryBar(value, width) {
    width = width || 10
    value = Math.max(0, Math.min(100, value))
    var filled = Math.round(value / 100 * width)
    return "█".repeat(filled) + "░".repeat(width - filled)
}

function chatMixBar(value, width) {
    width = width || 11
    value = Math.max(0, Math.min(100, value))
    var pos = Math.round(value / 100 * (width - 1))
    var chars = []
    for (var i = 0; i < width; i++) chars.push(i === pos ? "●" : "·")
    return chars.join("")
}

// Reduces independent game/chat mix levels to one 0 (game) - 100 (chat)
// position, for chatMixBar(). Devices don't expose a single balance value —
// only two independently-reported levels — but SteelSeries' ChatMix dial is
// a crossfader in practice, so game+chat is expected to add up to ~100;
// this holds even when it doesn't (falls back to 50/50 on 0+0).
function chatMixBalance(game, chat) {
    var total = game + chat
    return total > 0 ? (chat / total * 100) : 50
}
