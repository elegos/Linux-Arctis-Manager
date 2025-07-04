Name:           arctis-manager
Version:        %{_version}
Release:        %{?_release}%{!?_release:1}%{?dist}
Summary:        SteelSeries GG software replacement for Linux to manage Arctis devices

License:        GPL-3.0
URL:            https://github.com/elegos/Linux-Arctis-Manager
ExclusiveArch:  x86_64

# Dipendenze richieste per il pacchetto
BuildRequires:  bash, wget
Requires:       bash, pulseaudio-utils, libglvnd-glx, libglvnd-egl, fontconfig, libxkbcommon-x11, xcb-util-cursor, xcb-util-wm, xcb-util-keysyms

%description
SteelSeries GG software replacement to manage standard and advanced Arctis devices features, like ChatMix, ANC, wireless mode etc.

%prep
%if %{defined _localbuild}
    echo "Skipping src download (local build)"
    cp -r %{_sourcedir}/* .
%else
    wget https://github.com/elegos/Linux-Arctis-Manager/archive/refs/tags/v%{version}.zip -O %{_sourcedir}/v%{version}.zip
    unzip %{_sourcedir}/v%{version}.zip
    mv Linux-Arctis-Manager-%{version}/* .
    rm -rf Linux-Arctis-Manager-%{version}
%endif

%build
# Nothing to do here

%install
# build root cleanup
rm -rf %{buildroot}
mkdir -p %{buildroot}

# execute the install script (with chroot feature)
export CHROOT=%{buildroot}
export PREFIX="/usr"
bash install.sh

%files
/usr/lib/udev/rules.d/91-steelseries-arctis.rules
/usr/lib/systemd/user/arctis-manager.service
/usr/bin/arctis-manager
/usr/bin/arctis-manager-launcher
/usr/share/applications/ArctisManager.desktop
/usr/share/icons/hicolor/scalable/apps/arctis_manager.svg

%changelog
* Mon Jan 13 2025 Giacomo Furlan <giacomo@giacomofurlan.name> - 1.6.1-1
- First packaged version of the app
