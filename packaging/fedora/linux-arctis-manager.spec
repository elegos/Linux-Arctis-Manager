Name:           linux-arctis-manager
Version:        3.0.0~alpha1
Release:        1%{?dist}
Summary:        SteelSeries Arctis manager for Linux

License:        MIT
URL:            https://github.com/elegos/Linux-Arctis-Manager
Source0:        %{name}-%{version}.tar.gz

# uv is used to build the Python venv from the lockfile.
# COPR builds have network access; Koji (official Fedora) does not — for Koji,
# pre-generate a vendor tarball with: uv export --frozen --no-dev -o requirements.txt
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  uv
BuildRequires:  python3
BuildRequires:  systemd-devel
BuildRequires:  libcap

Requires:       python3
Requires:       libcap

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
