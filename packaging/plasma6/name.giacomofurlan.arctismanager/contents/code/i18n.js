.pragma library
.import "lang/en.js" as LangEn

// Reuses the same translation *content* as the Qt Widgets GUI
// (src/linux_arctis_manager/lang/*.ini, read by I18n.translate() in
// src/linux_arctis_manager/gui/*.py) so both UIs share one source of truth.
// Only en.ini exists today, but falls back to English for any requested
// language that isn't bundled.
//
// This can't just read lang/*.ini at runtime the way the GNOME extension's
// i18n.js does (Gio-based file I/O, unaffected by any of this): Plasma's
// applet QML engine hard-blocks local file reads through XMLHttpRequest
// ("Set QML_XHR_ALLOW_FILE_READ to 1 to enable this feature" — a sandbox
// applied to third-party plasmoids, not something a packaged app gets to
// ask a user's desktop session for). So instead, `make generate-plasmoid-lang`
// (wired into `make install-plasmoid`) compiles each lang/*.ini into a QML
// JS module — lang/<code>.js, `.pragma library` + `var TEXT = "<ini text>"`
// — and this file statically `.import`s each one: module imports go through
// QML's own resolution, not XHR, so they aren't subject to that block.
//
// The tradeoff: `.import` targets must be literal/static, so a new language
// needs a line added here (an `.import` above, an entry in _BUNDLED below) —
// unlike the Python GUI and GNOME extension, which just pick up new
// lang/*.ini files automatically. Not a regression to work around now since
// only en.ini exists project-wide; revisit if/when a second language ships.
var _BUNDLED = {
    en: LangEn.TEXT,
}

var _sections = null

function _stripComment(value) {
    var idx = value.indexOf("#")
    if (idx >= 0) value = value.substring(0, idx)
    return value.trim()
}

function _parseIni(text) {
    var sections = {}
    var current = null
    var lines = text.split("\n")
    for (var i = 0; i < lines.length; i++) {
        var line = lines[i]
        var trimmed = line.trim()
        if (trimmed.length === 0 || trimmed.charAt(0) === "#" || trimmed.charAt(0) === ";") continue

        var sectionMatch = trimmed.match(/^\[(.+)\]$/)
        if (sectionMatch) {
            current = sectionMatch[1]
            sections[current] = {}
            continue
        }

        if (current === null) continue
        var eq = trimmed.indexOf("=")
        if (eq < 0) continue
        var key = trimmed.substring(0, eq).trim()
        var value = trimmed.substring(eq + 1)
        sections[current][key] = value
    }
    return sections
}

function _load(langCode) {
    var text = (langCode && _BUNDLED[langCode]) || _BUNDLED.en
    return _parseIni(text)
}

function init(langCode) {
    _sections = _load(langCode)
}

function translate(section, key) {
    if (_sections === null) init()
    var sec = _sections[section]
    if (!sec) return String(key)
    var raw = sec[String(key)]
    if (raw === undefined) return String(key)
    return _stripComment(raw).replace(/\\n/g, "\n")
}
