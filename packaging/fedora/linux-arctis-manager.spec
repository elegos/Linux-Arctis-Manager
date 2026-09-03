Name:           linux-arctis-manager
Version:        3.0.0~alpha1
Release:        1%{?dist}
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

Requires:       python3
Requires:       libcap

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

%package plasma-widget
Summary:        KDE Plasma widget for %{name}
Requires:       %{name} = %{version}-%{release}
# Suggested, not required: most Plasma installs already pull this in, but a
# minimal/Wayland-only Plasma spin might not. Same "opt-in" reasoning as
# python3-torch above — don't force a DE-specific dependency onto everyone
# who installs the main package.
Supplements:    (plasma-workspace and %{name})

%description plasma-widget
A native KDE Plasma 6 widget (plasmoid) for %{name}: shows headset status
and a configurable set of quick-access controls in the Plasma panel,
positioned and sized by Plasma itself like the volume/network applets. Talks
to the already-running %{name} daemon directly over D-Bus — no Python
process involved.

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
%{_datadir}/linux-arctis-manager/
%{_datadir}/applications/*.desktop
%{_userunitdir}/lam-daemon.service
%{_userunitdir}/lam-hidraw-helper.service
%{_libdir}/linux-arctis-manager/

%files plasma-widget
%{_datadir}/plasma/plasmoids/name.giacomofurlan.arctismanager/

%changelog
* Tue Aug 25 2026 Giacomo Furlan <g.furlan@accenture.com> - 3.0.0~alpha1-1
- Initial v3 package: Rust daemon + Python Qt6 GUI, lam-cli removed
