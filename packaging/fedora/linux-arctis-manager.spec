Name:           linux-arctis-manager
Version:        3.0.0~alpha1
Release:        4%{?dist}
Summary:        SteelSeries Arctis manager for Linux

License:        MIT
URL:            https://github.com/elegos/Linux-Arctis-Manager
Source0:        %{name}-%{version}.tar.gz

# The venv bundles third-party scripts we don't control the shebang of (e.g.
# PySide6's pyside_tool.py ships a bare `#!/usr/bin/env python`, which
# brp-mangle-shebangs treats as a hard build error, not a warning, under
# Fedora's explicit python2-vs-python3 policy). Nothing in this project
# executes a venv-bundled script via its own shebang — lam-gui always
# invokes $(VENVDIR)/bin/python3 explicitly — so the whole venv is exempt.
%global __brp_mangle_shebangs_exclude_from ^/usr/lib(64)?/linux-arctis-manager/venv/.*$

# Same reasoning, different check: rpmbuild's automatic dependency generator
# scans every ELF file for Requires:/Provides:, including the venv's bundled
# Qt6/PySide6 plugins — pulling in Requires: on things like Oracle/Mimer SQL
# client libraries and an embedded-KMS Qt platform plugin that a desktop
# install never has and this app never loads (PySide6 vendors the full Qt6
# plugin set; nothing here selects a SQL backend or the eglfs-kms platform).
# install-test's `dnf install` of the built .rpm is what actually caught
# this — build-pkg alone never installs the package, just builds it.
# Trade-off: this also drops genuinely-needed Requires: (libGL.so.1,
# libxkbcommon.so.0, ...) that Qt's *real* platform/widgets code needs, not
# just the unused plugins — acceptable since any real desktop session
# already has them (X11/Wayland pulls them in regardless), same call made
# for Debian's dh_shlibdeps -X venv exclude.
%global __requires_exclude_from ^/usr/lib(64)?/linux-arctis-manager/venv/.*$
%global __provides_exclude_from ^/usr/lib(64)?/linux-arctis-manager/venv/.*$

# The Python venv is built with stdlib venv + pip (pip resolves runtime
# deps straight from pyproject.toml) — no uv binary needed at build time.
# COPR builds have network access; Koji (official Fedora) does not — pip
# still needs to reach PyPI either way, same as any other Python package
# with unvendored dependencies.
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  python3
BuildRequires:  python3-pip
BuildRequires:  systemd-devel
BuildRequires:  openssl-devel
BuildRequires:  libcap
# glib-compile-schemas, for the GNOME Shell extension's bundled GSettings
# schema (see %install and %files gnome-extension below).
BuildRequires:  glib2

Requires:       python3
Requires:       libcap
Requires:       hicolor-icon-theme

# Only needed for the AI voice changer's one-shot .pth -> ONNX voice-model
# conversion tool (E10-S6a/S7) — a soft dependency since the rest of the
# daemon (NC, EQ, sidetone, LADSPA voice changer, ...) doesn't need torch at
# all. Falls back to a per-user pip venv, with explicit consent, when absent.
Suggests:       python3-torch

%description
Linux Arctis Manager replaces the SteelSeries GG software for managing
SteelSeries Arctis headsets on Linux. It provides a user-space daemon
(written in Rust) and a Qt6 GUI (written in Python) for controlling
equalizer settings, sidetone, ANC, LED profiles, and more.

%package lang
Summary:        Standalone translation files for %{name}
BuildArch:      noarch
# Not needed by the main package itself (see below), but useless without one
# of the UI shells, both of which already Require %{name} — depending on it
# here too ties this package's removal to the main package's, same as those
# shells, instead of being left behind as an untracked orphan.
Requires:       %{name} = %{version}-%{release}

%description lang
Translation files for %{name}'s non-Python UI shells (the KDE Plasma widget,
the GNOME Shell extension) — a standalone copy outside the main package's
Python venv, which those UI shells can read without needing Python at all.
Pulled in automatically by whichever of those shells you install; not
useful on its own.

%package plasma-widget
Summary:        KDE Plasma widget for %{name}
BuildArch:      noarch
Requires:       %{name} = %{version}-%{release}
Requires:       %{name}-lang = %{version}-%{release}
# Hard requirement: the widget does nothing without Plasma. Separate from
# the Supplements below, which is about *whether this subpackage installs
# itself automatically* (not whether it needs plasma-workspace once chosen).
Requires:       plasma-workspace
# Auto-suggested, not auto-installed on every main-package install: only
# offered when both the main package and plasma-workspace are already
# present, so a minimal/Wayland-only or non-Plasma install never gets it
# uninvited. Same "opt-in" reasoning as python3-torch above.
Supplements:    (plasma-workspace and %{name})

%description plasma-widget
A native KDE Plasma 6 widget (plasmoid) for %{name}: shows headset status
and a configurable set of quick-access controls in the Plasma panel,
positioned and sized by Plasma itself like the volume/network applets. Talks
to the already-running %{name} daemon directly over D-Bus — no Python
process involved.

%package gnome-extension
Summary:        GNOME Shell extension for %{name}
BuildArch:      noarch
Requires:       %{name} = %{version}-%{release}
Requires:       %{name}-lang = %{version}-%{release}
# Same hard-Requires + soft-Supplements combination as plasma-widget above,
# GNOME's equivalent of plasma-workspace.
Requires:       gnome-shell >= 45
Supplements:    (gnome-shell and %{name})

%description gnome-extension
A GNOME Shell extension for %{name}: shows headset status and a configurable
set of quick-access controls from the top panel. Talks to the already-running
%{name} daemon directly over D-Bus — no Python process involved. Targets
GNOME Shell 45+ (ES-module extensions) only.

%prep
%autosetup -n %{name}-%{version}

%build
make build PREFIX=/usr

%install
# LIBDIR: the Makefile defaults to $(PREFIX)/lib (correct for Arch/Debian,
# which don't split lib/lib64); Fedora's own convention is %{_libdir}
# (/usr/lib64 on x86_64), which %files below actually references.
make install DESTDIR=%{buildroot} PREFIX=/usr LIBDIR=%{_libdir}

%post
# setcap cannot be applied during %%install (buildroot is not the live fs).
chown root:root %{_libexecdir}/lam-hidraw-helper
setcap cap_dac_override+eip %{_libexecdir}/lam-hidraw-helper

%postun
if [ $1 -eq 0 ]; then
    # Final removal — stop and disable user services for all users.
    # Best-effort: loginctl may not be available in all environments.
    loginctl list-users --no-legend 2>/dev/null | awk '{print $1}' | while read uid; do
        systemd-run --uid="$uid" --user --machine="" \
            systemctl --user stop lam-daemon.service lam-hidraw-helper.service \
            2>/dev/null || true
        systemd-run --uid="$uid" --user --machine="" \
            systemctl --user disable lam-daemon.service lam-hidraw-helper.service \
            2>/dev/null || true
    done
fi

%files
%license LICENSE
%{_bindir}/lam-daemon
%{_bindir}/lam-gui
%{_libexecdir}/lam-hidraw-helper
%{_datadir}/linux-arctis-manager/devices/
%{_datadir}/applications/*.desktop
%{_datadir}/icons/hicolor/scalable/apps/arctis-manager.svg
%{_datadir}/icons/hicolor/scalable/apps/arctis-manager-symbolic.svg
%{_userunitdir}/lam-daemon.service
%{_userunitdir}/lam-hidraw-helper.service
%{_libdir}/linux-arctis-manager/

%files lang
# The GUI's own translations, bundled inside the main package's Python venv,
# are a separate copy — this is the standalone one install-plasmoid /
# install-gnome-extension's shared LANG_DIR creates specifically so those
# subpackages don't need the Python venv to have translated strings. See
# Makefile's LANG_DIR comment. Its own subpackage (not folded into
# plasma-widget or gnome-extension) because both of those need it, and RPM
# won't let two sibling subpackages both own the same file.
%{_datadir}/linux-arctis-manager/lang/

%files plasma-widget
%{_datadir}/plasma/plasmoids/name.giacomofurlan.arctismanager/

%files gnome-extension
%{_datadir}/gnome-shell/extensions/arctis-manager@giacomofurlan.name/

%changelog
* Sat Sep 05 2026 Giacomo 'Mr. Wolf' Furlan <git@giacomofurlan.name> - 3.0.0~alpha4-4
- Nova Pro Omni: stop reading audio_settings at startup. Confirmed on real
  hardware its HID_FEATURE report tops out at 63 bytes, nowhere near the
  ~171-byte struct requested (chunk_size 1036) - GET_FEATURE just echoed
  back our own SET_FEATURE write, never real device data. Needs a proper
  chunked read the engine doesn't have yet (tracked as E7-S10); all these
  settings still update live via sync_events, only the cold-boot snapshot
  is affected

* Sat Sep 05 2026 Giacomo 'Mr. Wolf' Furlan <git@giacomofurlan.name> - 3.0.0~alpha4-3
- Fix Nova Pro Omni audio_settings read: HID_FEATURE reads never armed the
  device with the wanted command (SET_FEATURE) before reading it back
  (GET_FEATURE), so it always returned its idle/default all-zero report

* Sat Sep 05 2026 Giacomo 'Mr. Wolf' Furlan <git@giacomofurlan.name> - 3.0.0~alpha4-2
- Fix Nova Pro Omni init hang: audio_settings.incoming was missing its
  leading report_id field, and the sync-read reply matcher couldn't tell
  a real reply from an unsolicited notification sharing the same report ID

* Tue Aug 25 2026 Giacomo Furlan <g.furlan@accenture.com> - 3.0.0~alpha1-1
- Initial v3 package: Rust daemon + Python Qt6 GUI, lam-cli removed
