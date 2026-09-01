Name:           linux-arctis-manager
Version:        3.0.0~alpha1
Release:        1%{?dist}
Summary:        SteelSeries Arctis manager for Linux

License:        MIT
URL:            https://github.com/elegos/Linux-Arctis-Manager
Source0:        %{name}-%{version}.tar.gz

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
BuildRequires:  libcap

Requires:       python3
Requires:       libcap

# Only needed for the AI voice changer's one-shot .pth -> ONNX voice-model
# conversion tool (E10-S6a/S7) — a soft dependency since the rest of the
# daemon (NC, EQ, sidetone, LADSPA voice changer, ...) doesn't need torch at
# all. Falls back to a per-user pip venv, with explicit consent, when absent.
Recommends:     python3-torch

%description
Linux Arctis Manager replaces the SteelSeries GG software for managing
SteelSeries Arctis headsets on Linux. It provides a user-space daemon
(written in Rust) and a Qt6 GUI (written in Python) for controlling
equalizer settings, sidetone, ANC, LED profiles, and more.

%prep
%autosetup -n %{name}-%{version}

%build
make build PREFIX=/usr

%install
make install DESTDIR=%{buildroot} PREFIX=/usr

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
%{_userunitdir}/lam-daemon.service
%{_userunitdir}/lam-hidraw-helper.service
%{_libdir}/linux-arctis-manager/

%changelog
* Mon Aug 25 2026 Giacomo Furlan <g.furlan@accenture.com> - 3.0.0~alpha1-1
- Initial v3 package: Rust daemon + Python Qt6 GUI, lam-cli removed
