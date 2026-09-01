#!/usr/bin/env bash
# Smoke-tests an install (source `make install`, or a native package built
# from packaging/{arch,fedora,debian}): verifies the daemon, helper, and GUI
# wrapper actually land and run, and that the privileged capability was
# applied. Run from the repository root, after the install; set PREFIX to
# match how it was installed (packages use /usr, `make install` defaults to
# /usr/local).
set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
BINDIR="${BINDIR:-$PREFIX/bin}"
LIBEXECDIR="${LIBEXECDIR:-$PREFIX/libexec}"

fail() {
  echo "✖ $1" >&2
  exit 1
}

echo "Checking lam-daemon..."
"$BINDIR/lam-daemon" --help >/dev/null 2>&1 || fail "lam-daemon --help failed"

echo "Checking lam-hidraw-helper capability..."
command -v getcap >/dev/null || fail "getcap not found (libcap not installed?)"
getcap "$LIBEXECDIR/lam-hidraw-helper" | grep -q cap_dac_override \
  || fail "lam-hidraw-helper is missing CAP_DAC_OVERRIDE"

echo "Checking lam-gui wrapper..."
"$BINDIR/lam-gui" --help >/dev/null 2>&1 || fail "lam-gui --help failed"

echo "✔ Install test passed ($BINDIR, $LIBEXECDIR)"
