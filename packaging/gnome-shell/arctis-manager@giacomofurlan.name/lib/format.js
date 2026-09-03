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
