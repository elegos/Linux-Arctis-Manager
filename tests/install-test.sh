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
# lam-daemon has no --help/early-exit flag (only --log-level) — it's a
# long-running service by design, so the meaningful check is "does it start
# and keep running" (catches e.g. a missing shared library at load time),
# not "does it exit promptly".
log="$(mktemp)"
"$BINDIR/lam-daemon" --log-level=error >"$log" 2>&1 &
pid=$!
sleep 1
if ! kill -0 "$pid" 2>/dev/null; then
  cat "$log" >&2
  fail "lam-daemon exited immediately instead of staying up"
fi
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
rm -f "$log"

echo "Checking lam-hidraw-helper capability..."
command -v getcap >/dev/null || fail "getcap not found (libcap not installed?)"
getcap "$LIBEXECDIR/lam-hidraw-helper" | grep -q cap_dac_override \
  || fail "lam-hidraw-helper is missing CAP_DAC_OVERRIDE"

echo "Checking lam-gui wrapper..."
# Not executed: gui.py imports PySide6.QtWidgets at module load, before
# argparse even runs, which dlopen()s libGL.so.1 — a real desktop always
# has it (any X11/Wayland session pulls it in), but a headless package-
# manager smoke-test container neither has nor should need a GL stack just
# to prove the install landed correctly.
[ -x "$BINDIR/lam-gui" ] || fail "lam-gui is missing or not executable"

echo "✔ Install test passed ($BINDIR, $LIBEXECDIR)"
