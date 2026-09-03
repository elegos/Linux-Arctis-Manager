# Local-source PKGBUILD used exclusively by the Arch container build.
# The source tree is pre-copied into /home/builder/source by the Dockerfile;
# this PKGBUILD does not download anything.
#
# DO NOT submit this file to AUR — use packaging/arch/PKGBUILD instead.

pkgbase=linux-arctis-manager
pkgname=(linux-arctis-manager linux-arctis-manager-lang linux-arctis-manager-plasma-widget linux-arctis-manager-gnome-extension)
pkgver=0        # overridden by arch.build.dockerfile from VERSION file
pkgrel=1
arch=('x86_64')
url="https://github.com/elegos/Linux-Arctis-Manager"
license=('MIT')
makedepends=('rust' 'cargo' 'python' 'glib2')

# No source array — the Dockerfile has already placed the tree at /home/builder/source
source=()
sha256sums=()

build() {
    cd /home/builder/source
    make build PREFIX=/usr
}

package_linux-arctis-manager() {
    pkgdesc="SteelSeries Arctis manager for Linux — native daemon and Qt6 GUI"
    depends=('python' 'libcap' 'openssl' 'systemd-libs')
    provides=('linux-arctis-manager')
    conflicts=('linux-arctis-manager-git')
    install=lam.install

    cd /home/builder/source
    make install-core PREFIX=/usr DESTDIR="$pkgdir"
}

package_linux-arctis-manager-lang() {
    pkgdesc="Standalone translation files for linux-arctis-manager"
    # No compiled code in this one — same package for every arch.
    arch=('any')

    cd /home/builder/source
    make install-lang PREFIX=/usr DESTDIR="$pkgdir"
}

package_linux-arctis-manager-plasma-widget() {
    pkgdesc="KDE Plasma widget for linux-arctis-manager — status and configurable quick settings in the Plasma panel"
    # No compiled code in this one — same package for every arch.
    arch=('any')
    depends=('linux-arctis-manager' 'linux-arctis-manager-lang' 'plasma-workspace')

    cd /home/builder/source
    make install-plasmoid PREFIX=/usr DESTDIR="$pkgdir"
}

package_linux-arctis-manager-gnome-extension() {
    pkgdesc="GNOME Shell extension for linux-arctis-manager — status and configurable quick settings in the top panel"
    # No compiled code in this one — same package for every arch.
    arch=('any')
    depends=('linux-arctis-manager' 'linux-arctis-manager-lang' 'gnome-shell')

    cd /home/builder/source
    make install-gnome-extension PREFIX=/usr DESTDIR="$pkgdir"
}
