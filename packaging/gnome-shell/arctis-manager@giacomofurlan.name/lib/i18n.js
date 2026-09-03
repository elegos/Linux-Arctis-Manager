// Reuses the same translation files as the Qt Widgets GUI and the Plasma
// widget (src/linux_arctis_manager/lang/*.ini, read by I18n.translate() in
// src/linux_arctis_manager/gui/*.py) so all three UI shells share one
// translation source. Only en.ini exists today, but this loads whichever
// file is installed for the requested language, falling back to en.ini.
//
// Reads from the same standalone, package-version-independent path the
// Plasma widget already established (Makefile's LANG_DIR):
// /usr/share/linux-arctis-manager/lang/ on all three packaged distros
// (Fedora/Debian/Arch all build with PREFIX=/usr).

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const LANG_DIR = '/usr/share/linux-arctis-manager/lang/';

let _sections = null;

function _stripComment(value) {
    const idx = value.indexOf('#');
    return (idx >= 0 ? value.slice(0, idx) : value).trim();
}

function _parseIni(text) {
    const sections = {};
    let current = null;
    for (const rawLine of text.split('\n')) {
        const line = rawLine.trim();
        if (line.length === 0 || line.startsWith('#') || line.startsWith(';'))
            continue;

        const sectionMatch = line.match(/^\[(.+)\]$/);
        if (sectionMatch) {
            current = sectionMatch[1];
            sections[current] = {};
            continue;
        }

        if (current === null)
            continue;
        const eq = line.indexOf('=');
        if (eq < 0)
            continue;
        const key = line.slice(0, eq).trim();
        const value = line.slice(eq + 1);
        sections[current][key] = value;
    }
    return sections;
}

function _readFile(path) {
    try {
        const [ok, contents] = GLib.file_get_contents(path);
        return ok ? new TextDecoder('utf-8').decode(contents) : null;
    } catch (e) {
        return null;
    }
}

function _load(langCode) {
    const candidates = langCode ? [langCode, 'en'] : ['en'];
    for (const code of candidates) {
        const text = _readFile(`${LANG_DIR}${code}.ini`);
        if (text !== null)
            return _parseIni(text);
    }
    console.warn(`arctis-manager: no translation file found under ${LANG_DIR} — is the package fully installed?`);
    return {};
}

export function init(langCode) {
    _sections = _load(langCode);
}

export function translate(section, key) {
    if (_sections === null)
        init();
    const sec = _sections[section];
    if (!sec)
        return String(key);
    const raw = sec[String(key)];
    if (raw === undefined)
        return String(key);
    return _stripComment(raw).replace(/\\n/g, '\n');
}
