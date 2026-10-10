#!/usr/bin/env bash
# Builds the prebuilt, user-level release tarball that scripts/install.sh
# installs into ~/.local (no root except the one setcap step).
#
#   scripts/build-release-tarball.sh [--target generic|<os-id>] [--out DIR]
#
# Output: DIR/lam-<version>-<arch>-<target>.tar.gz   (default DIR: ./dist)
#
# Run it inside the *oldest* glibc environment you want to support (see
# packaging/containers/release.dockerfile) -- the resulting glibc floor is
# measured from the built binaries and written to manifest.txt, so the
# installer can refuse a too-old host before touching anything. This is the
# same idea as a manylinux wheel: build old, run anywhere newer.
#
# What's inside (everything relative to the tarball's single top directory):
#   manifest.txt              key=value prerequisites/pins read by install.sh
#   bin/ libexec/ share/      `make install SKIP_PYTHON=1` layout (PREFIX=/usr)
#   systemd/user/*.service.in unit templates (@BINDIR@/@LIBEXECDIR@ filled in
#                             by the installer for the real prefix)
#   python/                   lam wheel + hash-pinned requirements.lock.txt
#                             (the installer builds the venv on the target: a
#                             venv bakes absolute paths, so it can't be moved)
#
# Needs: cargo, make, uv, python3, glib-compile-schemas, objdump, curl, tar.
set -euo pipefail

TARGET=generic
OUT_DIR=dist
# Static (musl) uv, downloaded by install.sh to create the venv and fetch a
# redistributable CPython (python-build-standalone). Pinned here, verified
# against the sha256 published next to the release asset, and recorded in the
# (signed) manifest so the installer trusts the manifest, not the network.
UV_VERSION=${UV_VERSION:-0.10.9}
PYTHON_VERSION=${PYTHON_VERSION:-3.12}

while [ $# -gt 0 ]; do
    case $1 in
        --target) TARGET=$2; shift 2 ;;
        --out)    OUT_DIR=$2; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

cd "$(dirname "$0")/.."
ROOT=$PWD
VERSION=$(tr -d '[:space:]' < VERSION)
ARCH=$(uname -m)
case $ARCH in
    x86_64|aarch64) ;;
    *) echo "unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

NAME="lam-${VERSION}-${ARCH}-${TARGET}"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
STAGE=$WORK/stage
PKG=$WORK/$NAME
mkdir -p "$STAGE" "$PKG"

echo "==> Building and staging (make install SKIP_PYTHON=1 PREFIX=/usr)"
make install SKIP_PYTHON=1 DESTDIR="$STAGE" PREFIX=/usr
cp -a "$STAGE/usr/." "$PKG/"

# Generated units hardcode /usr paths; ship the templates instead and let the
# installer fill in the real prefix.
rm -rf "${PKG:?}/lib"
mkdir -p "$PKG/systemd/user"
cp packaging/systemd/user/*.service.in "$PKG/systemd/user/"

# The WirePlumber .conf drop-in belongs under the *config* dir, not the data
# dir, for a user-level install; keep it next to the script in the tarball
# and let the installer place each where WirePlumber looks.
echo "==> Building the Python wheel and hash-pinned requirements"
make sync-version
mkdir -p "$PKG/python"
uv build --wheel --out-dir "$PKG/python"
rm -f "$PKG/python/.gitignore"
uv export --frozen --no-dev --no-emit-project --format requirements-txt \
    --output-file "$PKG/python/requirements.lock.txt" >/dev/null

echo "==> Measuring the glibc floor"
GLIBC_MIN=$(
    for f in "$PKG/bin/lam-daemon" "$PKG/libexec/lam-hidraw-helper"; do
        objdump -T "$f" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//'
    done | sort -uV | tail -n1
)
[ -n "$GLIBC_MIN" ] || { echo "could not determine glibc floor" >&2; exit 1; }

case $ARCH in x86_64) UV_ARCH=x86_64 ;; aarch64) UV_ARCH=aarch64 ;; esac
UV_ASSET="uv-${UV_ARCH}-unknown-linux-musl.tar.gz"
UV_SHA256=$(curl --proto '=https' --tlsv1.2 -fsSL \
    "https://github.com/astral-sh/uv/releases/download/${UV_VERSION}/${UV_ASSET}.sha256" | awk '{print $1}')
[ ${#UV_SHA256} -eq 64 ] || { echo "bad uv sha256: '$UV_SHA256'" >&2; exit 1; }

cat > "$PKG/manifest.txt" <<EOF
format=1
version=${VERSION}
arch=${ARCH}
target=${TARGET}
glibc_min=${GLIBC_MIN}
python=${PYTHON_VERSION}
uv_version=${UV_VERSION}
uv_asset=${UV_ASSET}
uv_sha256=${UV_SHA256}
cmd=systemctl
cmd=tar
cmd_any=pkexec|sudo
EOF

case $OUT_DIR in /*) ;; *) OUT_DIR=$ROOT/$OUT_DIR ;; esac
mkdir -p "$OUT_DIR"
tar -C "$WORK" --owner=0 --group=0 --numeric-owner -czf "$OUT_DIR/$NAME.tar.gz" "$NAME"
echo "==> $OUT_DIR/$NAME.tar.gz (glibc >= $GLIBC_MIN)"
