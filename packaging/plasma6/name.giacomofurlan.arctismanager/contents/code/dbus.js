.pragma library

// Bus name/paths mirror src/linux_arctis_manager/constants.py — keep in
// sync if the daemon's D-Bus surface ever changes (see docs/dbus.md).
var BUS_NAME = "name.giacomofurlan.ArctisManager.Next"
var BASE_PATH = "/name/giacomofurlan/ArctisManager/Next"

var STATUS_PATH = BASE_PATH + "/Status"
var STATUS_IFACE = BUS_NAME + ".Status"

var SETTINGS_PATH = BASE_PATH + "/Settings"
var SETTINGS_IFACE = BUS_NAME + ".Settings"

var NC_PATH = BASE_PATH + "/NC"
var NC_IFACE = BUS_NAME + ".NC"

// Plasma::DBusConnection.asyncCall() wants `signature` as the full argument
// list wrapped in parens (e.g. "(ss)" for two string args, "()" for none) —
// verified against a real KWin call in Plasma's shipped taskmanager QML
// (/usr/lib64/qt6/qml/plasma/applet/.../taskmanager/main.qml). Not the raw
// D-Bus wire signature convention (which has no outer parens).
function _signature(argCount) {
    return "(" + "s".repeat(argCount) + ")"
}

function _call(dbus, path, iface, member, args) {
    return dbus.SessionBus.asyncCall({
        service: BUS_NAME,
        path: path,
        iface: iface,
        member: member,
        arguments: args || [],
        signature: _signature((args || []).length),
    })
}

function _jsonReply(reply, onOk, onError) {
    reply.finished.connect(function () {
        if (reply.isError) {
            if (onError) onError(reply.error.message)
            return
        }
        try {
            onOk(JSON.parse(reply.value))
        } catch (e) {
            if (onError) onError(String(e))
        }
    })
}

function getStatus(dbus, onOk, onError) {
    _jsonReply(_call(dbus, STATUS_PATH, STATUS_IFACE, "GetStatus"), onOk, onError)
}

function getSettings(dbus, onOk, onError) {
    _jsonReply(_call(dbus, SETTINGS_PATH, SETTINGS_IFACE, "GetSettings"), onOk, onError)
}

function setSetting(dbus, setting, value, onOk, onError) {
    var reply = _call(dbus, SETTINGS_PATH, SETTINGS_IFACE, "SetSetting", [setting, JSON.stringify(value)])
    reply.finished.connect(function () {
        if (reply.isError) {
            if (onError) onError(reply.error.message)
        } else if (onOk) {
            onOk(!!reply.value)
        }
    })
}

function getListOptions(dbus, listName, onOk, onError) {
    _jsonReply(_call(dbus, SETTINGS_PATH, SETTINGS_IFACE, "GetListOptions", [listName]), onOk, onError)
}

function getNcSettings(dbus, onOk, onError) {
    _jsonReply(_call(dbus, NC_PATH, NC_IFACE, "GetNCSettings"), onOk, onError)
}

function setNcSettings(dbus, settings, onOk, onError) {
    var reply = _call(dbus, NC_PATH, NC_IFACE, "SetNCSettings", [JSON.stringify(settings)])
    reply.finished.connect(function () {
        if (reply.isError) {
            if (onError) onError(reply.error.message)
        } else if (onOk) {
            onOk(true)
        }
    })
}
