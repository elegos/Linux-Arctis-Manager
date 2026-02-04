#!/usr/bin/env bash
set -euo pipefail

CUR_DIR=$(dirname "$0")
PROJECT_DIR=$(cd "$CUR_DIR" && cd .. && pwd)

cd "$PROJECT_DIR"

WHEEL="$(find dist -name "*.whl" | head -n1)"
CLI_NAME="lam-cli"

echo "Testing installation of: $WHEEL"
echo

success() {
  echo "✔ Installed with $1 via \`$2\`"
  exit 0
}

fail() {
  echo "✖ All installation methods failed"
  exit 1
}

# ensure PATH
export PATH="$HOME/.local/bin:$PATH"

# ---------- 1. pip --user ----------
echo "Trying pip install --user..."
if python3 -m pip install --user "$WHEEL" >/tmp/pip.log 2>&1; then
  command -v "$CLI_NAME" >/dev/null 2>&1 && success "pip" "pip install --user"
  echo "pip --user installed but CLI not found"
else
  cat /tmp/pip.log
fi

# ---------- 2. pipx ----------
if command -v pipx >/dev/null; then
  echo "Trying pipx install..."
  if pipx install "$WHEEL" >/tmp/pipx.log 2>&1; then
    command -v "$CLI_NAME" >/dev/null 2>&1 && success "pipx" "pipx install"
    echo "pipx installed but CLI not found"
  else
    cat /tmp/pipx.log
  fi
fi

fail
