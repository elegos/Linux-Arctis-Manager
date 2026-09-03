.pragma library

// Reuses the same translation files as the Qt Widgets GUI
// (src/linux_arctis_manager/lang/*.ini, read by I18n.translate() in
// src/linux_arctis_manager/gui/*.py) so both UIs share one translation
// source. Only en.ini exists today, but this loads whichever file is
// installed for the requested language, falling back to en.ini.
//
// The venv's own copy of lang/*.ini lives under a Python-version-dependent
// site-packages path we can't reliably guess from QML, so the Makefile
// installs a second, stable copy at $(DATADIR)/linux-arctis-manager/lang/
// (see LANG_DIR in the top-level Makefile) — /usr/share/linux-arctis-manager
// /lang/ on all three packaged distros (Fedora/Debian/Arch all build with
// PREFIX=/usr).
var LANG_DIR = "/usr/share/linux-arctis-manager/lang/"

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
    var candidates = []
    if (langCode) candidates.push(langCode)
    candidates.push("en")

    for (var i = 0; i < candidates.length; i++) {
        var url = "file://" + LANG_DIR + candidates[i] + ".ini"
        var xhr = new XMLHttpRequest()
        try {
            xhr.open("GET", url, false)
            xhr.send()
        } catch (e) {
            continue
        }
        // file:// requests report success as status 0 with a body, not 200.
        if (xhr.responseText) {
            return _parseIni(xhr.responseText)
        }
    }
    return {}
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
