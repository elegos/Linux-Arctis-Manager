// D-Bus client for the Arctis Manager daemon.
//
// Bus name/paths mirror src/linux_arctis_manager/constants.py — keep in
// sync if the daemon's D-Bus surface ever changes (see docs/dbus.md).
//
// Method names were taken from the working Python client
// (src/linux_arctis_manager/gui/dbus_wrapper.py). The NC interface's
// *methods* are explicitly renamed server-side to an all-caps "NC"
// (`GetNCCapabilities`/`GetNCSettings`/`SetNCSettings`,
// daemon/engine/src/dbus.rs), but its *signal* isn't — verified live
// against the running daemon with `gdbus monitor`: the signal name on the
// wire is `NcChanged`, not `NCChanged` (zbus's default snake_case→PascalCase
// conversion has no special-case for the "NC" acronym; only the methods got
// an explicit `#[zbus(name = "...")]` override, the signal didn't).

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const BUS_NAME = 'name.giacomofurlan.ArctisManager.Next';
const BASE_PATH = '/name/giacomofurlan/ArctisManager/Next';

const STATUS_PATH = `${BASE_PATH}/Status`;
const STATUS_IFACE = `${BUS_NAME}.Status`;

const SETTINGS_PATH = `${BASE_PATH}/Settings`;
const SETTINGS_IFACE = `${BUS_NAME}.Settings`;

const NC_PATH = `${BASE_PATH}/NC`;
const NC_IFACE = `${BUS_NAME}.NC`;

function _signature(argCount) {
    return `(${'s'.repeat(argCount)})`;
}

// Synchronous on purpose: building the local proxy object is cheap (no
// D-Bus round-trip — that happens in proxy.call() below, which *is*
// async). Sidesteps new_for_bus()'s promise-vs-callback argument-count
// ambiguity entirely.
function _proxy(path, iface) {
    return Gio.DBusProxy.new_for_bus_sync(
        Gio.BusType.SESSION, Gio.DBusProxyFlags.NONE, null,
        BUS_NAME, path, iface, null);
}

function _call(path, iface, member, args = []) {
    const proxy = _proxy(path, iface);
    const variant = args.length > 0
        ? new GLib.Variant(_signature(args.length), args)
        : null;
    // Explicit callback, not GJS's auto-promisified form — that form's
    // exact required-argument count didn't match any combination tried
    // empirically against the real GJS on this machine (both 5 and 6
    // positional args either threw "N arguments required" or silently
    // resolved to undefined). This classic callback→Promise wrapping is
    // unambiguous and known to work.
    return new Promise((resolve, reject) => {
        proxy.call(member, variant, Gio.DBusCallFlags.NONE, -1, null, (proxySelf, result) => {
            try {
                resolve(proxySelf.call_finish(result).recursiveUnpack());
            } catch (e) {
                reject(e);
            }
        });
    });
}

async function _jsonCall(path, iface, member, args = []) {
    const [raw] = await _call(path, iface, member, args);
    return JSON.parse(raw);
}

export async function getStatus() {
    return _jsonCall(STATUS_PATH, STATUS_IFACE, 'GetStatus');
}

export async function getSettings() {
    return _jsonCall(SETTINGS_PATH, SETTINGS_IFACE, 'GetSettings');
}

export async function setSetting(setting, value) {
    const [ok] = await _call(SETTINGS_PATH, SETTINGS_IFACE, 'SetSetting',
        [setting, JSON.stringify(value)]);
    return !!ok;
}

export async function getListOptions(listName) {
    return _jsonCall(SETTINGS_PATH, SETTINGS_IFACE, 'GetListOptions', [listName]);
}

export async function getNcSettings() {
    return _jsonCall(NC_PATH, NC_IFACE, 'GetNCSettings');
}

export async function setNcSettings(settings) {
    await _call(NC_PATH, NC_IFACE, 'SetNCSettings', [JSON.stringify(settings)]);
}

function _subscribe(iface, signal, path, callback) {
    return Gio.DBus.session.signal_subscribe(
        BUS_NAME, iface, signal, path, null, Gio.DBusSignalFlags.NONE,
        (_connection, _sender, _objectPath, _ifaceName, _signalName, params) => {
            const [json] = params.recursiveUnpack();
            try {
                callback(JSON.parse(json));
            } catch (e) {
                logError(e, `dbus.js: failed to parse ${signal} payload`);
            }
        });
}

export function subscribeStatusChanged(callback) {
    return _subscribe(STATUS_IFACE, 'StatusChanged', STATUS_PATH, callback);
}

export function subscribeSettingsChanged(callback) {
    return _subscribe(SETTINGS_IFACE, 'SettingsChanged', SETTINGS_PATH, callback);
}

export function subscribeNcChanged(callback) {
    return _subscribe(NC_IFACE, 'NcChanged', NC_PATH, callback);
}

export function unsubscribe(id) {
    Gio.DBus.session.signal_unsubscribe(id);
}
