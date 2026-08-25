# Local-source PKGBUILD used exclusively by the Arch container build.
# The source tree is pre-copied into /home/builder/source by the Dockerfile;
# this PKGBUILD does not download anything.
#
# DO NOT submit this file to AUR — use packaging/arch/PKGBUILD instead.

pkgname=linux-arctis-manager
pkgver=0        # overridden by arch.build.dockerfile from VERSION file
pkgrel=1
pkgdesc="SteelSeries Arctis manager for Linux — native daemon and Qt6 GUI"
arch=('x86_64')
url="https://github.com/elegos/Linux-Arctis-Manager"
license=('MIT')
depends=('python' 'libcap')
makedepends=('rust' 'cargo' 'uv' 'python')
provides=('linux-arctis-manager')
conflicts=('linux-arctis-manager-git')
install=lam.install

# No source array — the Dockerfile has already placed the tree at /home/builder/source
source=()
sha256sums=()

build() {
    cd /home/builder/source
    make build PREFIX=/usr
}

package() {
    cd /home/builder/source
    make install PREFIX=/usr DESTDIR="$pkgdir"
}
