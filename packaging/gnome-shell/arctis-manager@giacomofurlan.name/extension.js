// Panel indicator + popup for Arctis Manager. Talks directly to the daemon
// over D-Bus (lib/dbus.js) — no Python process involved.
//
// UX convention shared with the Plasma widget (packaging/plasma6/): no
// tabs, one popup with a status section above a quick-settings section.
// The Plasma widget uses a resizable SplitView; St (this toolkit) has no
// splitter widget, so this relies on the popup menu's own built-in
// overflow scrolling instead of a custom scroll area — simpler, and avoids
// a St.ScrollView-sizing risk area that couldn't be live-tested (this was
// written without a running GNOME Shell to test against — see the plan's
// Verification section).
//
// Could not be exercised against a real GNOME Shell (this machine runs
// Plasma). lib/dbus.js, lib/i18n.js, lib/format.js were all verified
// standalone with `gjs -m` against the real running daemon; this file's
// Shell-specific APIs (PanelMenu, PopupMenu, St, Main) were not.

import St from 'gi://St';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

import * as Dbus from './lib/dbus.js';
import * as I18n from './lib/i18n.js';
import * as Format from './lib/format.js';

// Mirrors the Plasma widget's QuickSettingsView.qml ncPresets list.
const NC_PRESETS = [
    ['off', 'nc_preset_off'],
    ['light', 'nc_preset_light'],
    ['standard', 'nc_preset_standard'],
    ['studio', 'nc_preset_studio'],
];

export default class ArctisManagerExtension extends Extension {
    enable() {
        const lang = (GLib.get_language_names()[0] || 'en').split(/[_.]/)[0];
        I18n.init(lang);

        this._settings = this.getSettings();
        this._status = {};
        this._settingsPayload = {};
        this._ncSettings = {};
        this._optionLists = {};

        this._indicator = new PanelMenu.Button(0.0, this.metadata.name, false);
        this._indicator.add_child(new St.Icon({
            icon_name: 'arctis-manager-symbolic',
            style_class: 'system-status-icon',
        }));

        this._statusSection = new PopupMenu.PopupMenuSection();
        this._settingsSection = new PopupMenu.PopupMenuSection();

        this._openAppItem = new PopupMenu.PopupMenuItem(I18n.translate('ui', 'open_app'));
        this._openAppItem.connect('activate', () => this._openMainApp());
        this._indicator.menu.addMenuItem(this._openAppItem);
        this._indicator.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        this._indicator.menu.addMenuItem(this._statusSection);
        this._indicator.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._indicator.menu.addMenuItem(this._settingsSection);

        // Rebuild the quick-settings section (option lists, config) only
        // when the popup is about to be shown — no point rebuilding combo
        // option lists while it's closed.
        this._openStateId = this._indicator.menu.connect('open-state-changed', (menu, open) => {
            if (open)
                this._refresh();
        });

        Main.panel.addToStatusArea(this.uuid, this._indicator);

        this._subscriptions = [
            Dbus.subscribeStatusChanged(status => {
                this._status = status || {};
                this._renderStatus();
            }),
            Dbus.subscribeSettingsChanged(settings => {
                this._settingsPayload = settings || {};
                this._renderSettings();
            }),
            Dbus.subscribeNcChanged(nc => {
                this._ncSettings = nc || {};
                this._renderSettings();
            }),
        ];

        this._refresh();
    }

    async _refresh() {
        try {
            const [status, settingsPayload, ncSettings] = await Promise.all([
                Dbus.getStatus(),
                Dbus.getSettings(),
                Dbus.getNcSettings(),
            ]);
            this._status = status || {};
            this._settingsPayload = settingsPayload || {};
            this._ncSettings = ncSettings || {};
        } catch (e) {
            logError(e, 'arctis-manager: refresh failed (daemon not running?)');
        }
        this._renderStatus();
        this._renderSettings();
    }

    _openMainApp() {
        try {
            Gio.Subprocess.new(['lam-gui'], Gio.SubprocessFlags.NONE);
        } catch (e) {
            logError(e, 'arctis-manager: failed to launch lam-gui');
        }
    }

    // ── Status section ──────────────────────────────────────────────────────

    _enabledStatusFields() {
        return this._settings.get_strv('enabled-status-fields');
    }

    _fieldEnabled(name, enabledFields) {
        return enabledFields.length === 0 || enabledFields.includes(name);
    }

    _renderStatus() {
        this._statusSection.removeAll();

        const categories = Object.keys(this._status || {});
        if (categories.length === 0) {
            this._statusSection.addMenuItem(this._infoRow(I18n.translate('ui', 'no_device_detected')));
            return;
        }

        const settingsConfig = (this._settingsPayload && this._settingsPayload.settings_config) || {};
        const enabledFields = this._enabledStatusFields();

        for (const category of categories) {
            const fields = this._status[category] || {};
            // Mirrors status_widget.py's skip_fields: hide the transparency
            // level slider unless transparency mode is actually "transparent".
            const hideTransparentLevel =
                (fields.transparency_mode && fields.transparency_mode.value) !== 'transparent';

            const visibleKeys = Object.keys(fields).filter(key => {
                if (key === 'transparent_level' && hideTransparentLevel)
                    return false;
                return this._fieldEnabled(key, enabledFields);
            });
            if (visibleKeys.length === 0)
                continue;

            this._statusSection.addMenuItem(this._infoRow(I18n.translate('status', category), true));
            for (const key of visibleKeys) {
                const display = Format.formatValue(key, fields[key], settingsConfig);
                this._statusSection.addMenuItem(
                    this._infoRow(`${I18n.translate('status', key)}: ${display}`));
            }
        }
    }

    _infoRow(text, bold = false) {
        const item = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        const label = new St.Label({text});
        if (bold)
            label.style = 'font-weight: bold;';
        item.add_child(label);
        return item;
    }

    // ── Quick settings section ──────────────────────────────────────────────

    _pinnedSettings() {
        return this._settings.get_strv('pinned-settings');
    }

    _renderSettings() {
        this._settingsSection.removeAll();

        const settingsConfig = (this._settingsPayload && this._settingsPayload.settings_config) || {};
        const currentValues = this._currentValues(settingsConfig);
        const pinned = this._pinnedSettings();

        const renderable = pinned.filter(sid => this._isRenderable(sid, settingsConfig));
        if (renderable.length === 0) {
            this._settingsSection.addMenuItem(this._infoRow(I18n.translate('ui', 'quick_settings_empty')));
            return;
        }

        for (const sid of renderable)
            this._buildControl(sid, settingsConfig, currentValues);
    }

    _currentValues(settingsConfig) {
        const values = {};
        for (const category of Object.keys(this._status || {})) {
            for (const [key, field] of Object.entries(this._status[category] || {}))
                values[key] = field.value;
        }
        const device = (this._settingsPayload && this._settingsPayload.device) || {};
        for (const [key, value] of Object.entries(device)) {
            if (value !== null && value !== undefined)
                values[key] = value;
        }
        return values;
    }

    _isRenderable(sid, settingsConfig) {
        if (sid === 'nc_preset')
            return true;
        const cfg = settingsConfig[sid];
        if (!cfg)
            return false;
        if (cfg.type === 'select') {
            const src = cfg.options_source;
            return !src || this._optionLists[src] !== undefined;
        }
        return true;
    }

    _labelFor(sid) {
        if (sid === 'nc_preset')
            return I18n.translate('ui', 'nc');
        let label = I18n.translate('settings', sid);
        if (label === sid)
            label = I18n.translate('status', sid);
        return label;
    }

    _buildControl(sid, settingsConfig, currentValues) {
        if (sid === 'nc_preset') {
            this._buildNcPresetControl();
            return;
        }

        const cfg = settingsConfig[sid];
        if (!cfg)
            return;

        if (cfg.type === 'toggle') {
            this._buildToggleControl(sid, cfg, currentValues[sid]);
        } else if (cfg.type === 'slider') {
            this._buildSliderControl(sid, cfg, currentValues[sid]);
        } else if (cfg.type === 'discrete_map') {
            this._buildDiscreteMapControl(sid, cfg, currentValues[sid]);
        } else if (cfg.type === 'select') {
            this._ensureOptions(cfg.options_source);
            this._buildSelectControl(sid, cfg, currentValues[sid]);
        }
    }

    async _ensureOptions(src) {
        if (!src || this._optionLists[src] !== undefined)
            return;
        try {
            this._optionLists[src] = await Dbus.getListOptions(src);
        } catch (e) {
            this._optionLists[src] = [];
            logError(e, `arctis-manager: GetListOptions(${src}) failed`);
        }
        this._renderSettings();
    }

    _buildNcPresetControl() {
        const item = new PopupMenu.PopupSubMenuMenuItem(this._labelFor('nc_preset'));
        for (const [key, labelKey] of NC_PRESETS) {
            const sub = new PopupMenu.PopupMenuItem(I18n.translate('ui', labelKey));
            if (key === (this._ncSettings.preset || 'off'))
                sub.setOrnament(PopupMenu.Ornament.CHECK);
            sub.connect('activate', () => {
                Dbus.setNcSettings({...this._ncSettings, preset: key}).catch(
                    e => logError(e, 'arctis-manager: SetNCSettings failed'));
            });
            item.menu.addMenuItem(sub);
        }
        this._settingsSection.addMenuItem(item);
    }

    _buildToggleControl(sid, cfg, rawValue) {
        const values = cfg.values || {};
        const onValue = values.on !== undefined ? values.on : true;
        const offValue = values.off !== undefined ? values.off : false;
        const checked = rawValue !== undefined ? rawValue === onValue : false;

        const item = new PopupMenu.PopupSwitchMenuItem(this._labelFor(sid), checked);
        item.connect('toggled', (_item, state) => {
            Dbus.setSetting(sid, state ? onValue : offValue).catch(
                e => logError(e, `arctis-manager: SetSetting(${sid}) failed`));
        });
        this._settingsSection.addMenuItem(item);
    }

    _buildSliderControl(sid, cfg, rawValue) {
        const min = cfg.min !== undefined ? cfg.min : 0;
        const max = cfg.max !== undefined ? cfg.max : 100;
        const value = rawValue !== undefined ? rawValue : min;

        const item = new PopupMenu.PopupBaseMenuItem({activate: false});
        item.add_child(new St.Label({text: this._labelFor(sid), x_expand: false}));

        const slider = new St.Slider({value: max > min ? (value - min) / (max - min) : 0});
        slider.x_expand = true;
        slider.connect('drag-end', () => {
            const newValue = Math.round(min + slider.value * (max - min));
            Dbus.setSetting(sid, newValue).catch(
                e => logError(e, `arctis-manager: SetSetting(${sid}) failed`));
        });
        item.add_child(slider);
        this._settingsSection.addMenuItem(item);
    }

    _buildDiscreteMapControl(sid, cfg, rawValue) {
        const mapping = cfg.values_mapping || {};
        const orderedKeys = Object.keys(mapping).sort((a, b) => parseInt(a) - parseInt(b));

        const item = new PopupMenu.PopupSubMenuMenuItem(this._labelFor(sid));
        for (const key of orderedKeys) {
            const sub = new PopupMenu.PopupMenuItem(I18n.translate('settings_values', mapping[key]));
            if (rawValue !== undefined && parseInt(key) === parseInt(rawValue))
                sub.setOrnament(PopupMenu.Ornament.CHECK);
            sub.connect('activate', () => {
                Dbus.setSetting(sid, parseInt(key)).catch(
                    e => logError(e, `arctis-manager: SetSetting(${sid}) failed`));
            });
            item.menu.addMenuItem(sub);
        }
        this._settingsSection.addMenuItem(item);
    }

    _buildSelectControl(sid, cfg, rawValue) {
        const options = this._optionLists[cfg.options_source] || [];
        const item = new PopupMenu.PopupSubMenuMenuItem(this._labelFor(sid));
        for (const option of options) {
            const label = option.name || option.description || String(option.id);
            const sub = new PopupMenu.PopupMenuItem(label);
            if (option.id === rawValue)
                sub.setOrnament(PopupMenu.Ornament.CHECK);
            sub.connect('activate', () => {
                Dbus.setSetting(sid, option.id).catch(
                    e => logError(e, `arctis-manager: SetSetting(${sid}) failed`));
            });
            item.menu.addMenuItem(sub);
        }
        this._settingsSection.addMenuItem(item);
    }

    disable() {
        if (this._openStateId && this._indicator)
            this._indicator.menu.disconnect(this._openStateId);
        this._openStateId = null;

        (this._subscriptions || []).forEach(id => Dbus.unsubscribe(id));
        this._subscriptions = null;

        this._indicator?.destroy();
        this._indicator = null;

        this._openAppItem = null;
        this._statusSection = null;
        this._settingsSection = null;
        this._settings = null;
        this._status = null;
        this._settingsPayload = null;
        this._ncSettings = null;
        this._optionLists = null;
    }
}
