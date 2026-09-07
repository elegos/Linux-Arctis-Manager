// Port of QStatusWidget.format_value()
// (src/linux_arctis_manager/gui/status_widget.py) — keep in sync with that
// and the Plasma widget's contents/code/format.js, all three must agree on
// how a status value is displayed.

import * as I18n from './i18n.js';

export function formatValue(statusKey, statusO, settingsConfig) {
    const val = statusO.value;
    const dtype = statusO.type;

    if (dtype === 'percentage')
        return `${val}%`;

    if (dtype === 'on_off')
        return I18n.translate('status_values', val ? 'on' : 'off');

    if (typeof val === 'number' && ['uint8', 'uint16', 'uint32'].includes(dtype)) {
        const cfg = (settingsConfig && settingsConfig[statusKey]) || {};
        const vm = cfg.values_mapping || {};
        const intKey = String(Math.trunc(val));
        const labelKey = vm[intKey] !== undefined ? vm[intKey] : intKey;
        return I18n.translate('status_values', labelKey);
    }

    return I18n.translate('status_values', val);
}

// Port of ascii_bar.py's battery_bar/chatmix_bar
// (src/linux_arctis_manager/gui/ascii_bar.py) — keep the two in sync.

export function batteryBar(value, width = 10) {
    value = Math.max(0, Math.min(100, value));
    const filled = Math.round(value / 100 * width);
    return '█'.repeat(filled) + '░'.repeat(width - filled);
}

export function chatMixBar(value, width = 11) {
    value = Math.max(0, Math.min(100, value));
    const pos = Math.round(value / 100 * (width - 1));
    return Array.from({length: width}, (_, i) => i === pos ? '●' : '·').join('');
}

// Reduces independent game/chat mix levels to one 0 (game) - 100 (chat)
// position, for chatMixBar(). Devices don't expose a single balance value —
// only two independently-reported levels — but SteelSeries' ChatMix dial is
// a crossfader in practice, so game+chat is expected to add up to ~100;
// this holds even when it doesn't (falls back to 50/50 on 0+0).
export function chatMixBalance(game, chat) {
    const total = game + chat;
    return total > 0 ? (chat / total * 100) : 50;
}
