#!/bin/sh
# Exercises scripts/install.sh against a locally built release tarball:
# signs it with a throwaway key (swapped into a copy of the installer),
# then runs the real verification + preflight (glibc floor, shared
# libraries, required commands) on THIS machine. Nothing is installed:
# --dry-run stops before the first change. Used by install-test.yaml across
# the distro matrix, where the preflight is the interesting part - it is
# what proves the "build old, run anywhere newer" tarball really resolves
# its libraries on each distro.
#
# Usage: tests/installer/dry-run.sh path/to/lam-<ver>-<arch>-<target>.tar.gz
set -eu

TARBALL=${1:?usage: dry-run.sh <tarball>}
HERE=$(cd "$(dirname "$0")" && pwd)
INSTALLER=$HERE/../../scripts/install.sh
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT

mkdir -p "$W/assets" "$W/shim" "$W/home"
cp "$TARBALL" "$W/assets/"
( cd "$W/assets" && sha256sum -- *.tar.gz > SHA256SUMS )
ssh-keygen -q -t ed25519 -N '' -f "$W/key" -C lam-release
ssh-keygen -Y sign -f "$W/key" -n lam-release "$W/assets/SHA256SUMS" >/dev/null 2>&1
PUB=$(cut -d' ' -f1,2 "$W/key.pub")
sed "s|^SIGNING_PUBKEY=.*|SIGNING_PUBKEY='$PUB'|" "$INSTALLER" > "$W/install.sh"

# A CI container has no systemd user session; the preflight only needs the
# probe to succeed.
printf '#!/bin/sh\nexit 0\n' > "$W/shim/systemctl"
chmod +x "$W/shim/systemctl"

# 1. A correctly signed release passes verification + preflight.
HOME=$W/home PATH=$W/shim:$PATH sh "$W/install.sh" --dry-run --yes --local-assets "$W/assets"

# 2. A tampered tarball must be rejected before anything is extracted.
mkdir -p "$W/bad"; cp "$W"/assets/* "$W/bad/"
for f in "$W"/bad/*.tar.gz; do echo tampered >> "$f"; done
if HOME=$W/home PATH=$W/shim:$PATH sh "$W/install.sh" --dry-run --yes --local-assets "$W/bad" >"$W/bad.log" 2>&1; then
    echo "tampered tarball was ACCEPTED" >&2; exit 1
fi
grep -q 'checksum mismatch' "$W/bad.log" || { cat "$W/bad.log" >&2; exit 1; }

# 3. A signature from a different key must be rejected.
mkdir -p "$W/wrongkey"; cp "$W"/assets/* "$W/wrongkey/"
ssh-keygen -q -t ed25519 -N '' -f "$W/other" -C other
rm -f "$W/wrongkey/SHA256SUMS.sig"
ssh-keygen -Y sign -f "$W/other" -n lam-release "$W/wrongkey/SHA256SUMS" >/dev/null 2>&1
if HOME=$W/home PATH=$W/shim:$PATH sh "$W/install.sh" --dry-run --yes --local-assets "$W/wrongkey" >"$W/wrong.log" 2>&1; then
    echo "foreign signature was ACCEPTED" >&2; exit 1
fi
grep -q 'signature verification FAILED' "$W/wrong.log" || { cat "$W/wrong.log" >&2; exit 1; }
echo "installer dry-run tests passed"
