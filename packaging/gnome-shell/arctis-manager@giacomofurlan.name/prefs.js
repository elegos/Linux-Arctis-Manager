// "Preferences" window: pick which status fields are shown and which
// settings are pinned as quick-access controls. Mirrors the Plasma widget's
// ConfigStatusFields.qml / ConfigQuickSettings.qml — same two GSettings
// array-of-string keys, same "query the daemon live for the current
// field/setting list" approach.
//
// Runs in a separate GTK4 + libadwaita process, not inside GNOME Shell —
// Gio (hence D-Bus) works fine here, only Shell-internal APIs (Meta,
// Clutter, Shell, St) don't. Could not be exercised against a real GNOME
// Shell/Adwaita — see extension.js's header and the plan's Verification
// section.

import Adw from 'gi://Adw';

import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

import * as Dbus from './lib/dbus.js';
import * as I18n from './lib/i18n.js';

export default class ArctisManagerPreferences extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        I18n.init();
        const settings = this.getSettings();
        window._settings = settings;

        const statusPage = new Adw.PreferencesPage({
            title: I18n.translate('ui', 'status'),
            icon_name: 'view-list-symbolic',
        });
        window.add(statusPage);

        const settingsPage = new Adw.PreferencesPage({
            title: I18n.translate('ui', 'quick_settings'),
            icon_name: 'preferences-system-symbolic',
        });
        window.add(settingsPage);

        this._buildStatusPage(statusPage, settings);
        this._buildSettingsPage(settingsPage, settings);
    }

    async _buildStatusPage(page, settings) {
        const group = new Adw.PreferencesGroup({title: I18n.translate('ui', 'status')});
        page.add(group);

        let status = {};
        try {
            status = await Dbus.getStatus();
        } catch (e) {
            logError(e, 'arctis-manager prefs: GetStatus failed');
        }

        const keys = [];
        for (const category of Object.keys(status || {})) {
            for (const key of Object.keys(status[category] || {})) {
                if (!keys.includes(key))
                    keys.push(key);
            }
        }
        // Keep previously-enabled-but-now-unreported fields selectable too
        // (e.g. no device connected right now), same as the Plasma widget's
        // ConfigStatusFields.qml.
        for (const key of settings.get_strv('enabled-status-fields')) {
            if (!keys.includes(key))
                keys.push(key);
        }

        for (const key of keys) {
            let label = I18n.translate('status', key);
            if (label === key)
                label = key;
            const row = new Adw.SwitchRow({title: label});
            group.add(row);

            const configured = settings.get_strv('enabled-status-fields');
            row.active = configured.length === 0 || configured.includes(key);
            row.connect('notify::active', () => {
                let current = settings.get_strv('enabled-status-fields').slice();
                if (current.length === 0)
                    current = keys.slice();
                const idx = current.indexOf(key);
                if (row.active && idx === -1)
                    current.push(key);
                if (!row.active && idx !== -1)
                    current.splice(idx, 1);
                settings.set_strv('enabled-status-fields', current);
            });
        }
    }

    async _buildSettingsPage(page, settings) {
        const group = new Adw.PreferencesGroup({title: I18n.translate('ui', 'quick_settings')});
        page.add(group);

        let settingsPayload = {};
        try {
            settingsPayload = await Dbus.getSettings();
        } catch (e) {
            logError(e, 'arctis-manager prefs: GetSettings failed');
        }

        const ids = ['nc_preset'];
        for (const id of Object.keys((settingsPayload && settingsPayload.settings_config) || {})) {
            if (!ids.includes(id))
                ids.push(id);
        }

        for (const id of ids) {
            let label = id === 'nc_preset' ? I18n.translate('ui', 'nc') : I18n.translate('settings', id);
            if (label === id)
                label = I18n.translate('status', id);
            if (label === id)
                label = id;

            const row = new Adw.SwitchRow({title: label});
            group.add(row);

            row.active = settings.get_strv('pinned-settings').includes(id);
            row.connect('notify::active', () => {
                const current = settings.get_strv('pinned-settings').slice();
                const idx = current.indexOf(id);
                if (row.active && idx === -1)
                    current.push(id);
                if (!row.active && idx !== -1)
                    current.splice(idx, 1);
                settings.set_strv('pinned-settings', current);
            });
        }
    }
}
