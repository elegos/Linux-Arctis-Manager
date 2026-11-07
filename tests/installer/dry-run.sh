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
# Usage: tests/installer/dry-run.sh path/to/lam-<ver>-<arch>-<target>.tar.gz...
# With several tarballs (the OpenSSL 3 "generic" and OpenSSL 4 "generic-ossl4"
# builds) the installer must pick the one matching the host's libssl.
set -eu

[ $# -ge 1 ] || { echo "usage: dry-run.sh <tarball>..." >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
INSTALLER=$HERE/../../scripts/install.sh
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT

mkdir -p "$W/assets" "$W/shim" "$W/home"
cp "$@" "$W/assets/"
( cd "$W/assets" && sha256sum -- *.tar.gz > SHA256SUMS )
ssh-keygen -q -t ed25519 -N '' -f "$W/key" -C lam-release
ssh-keygen -Y sign -f "$W/key" -n lam-release "$W/assets/SHA256SUMS" >/dev/null 2>&1
PUB=$(cut -d' ' -f1,2 "$W/key.pub")
sed "s|^SIGNING_PUBKEY=.*|SIGNING_PUBKEY='$PUB'|" "$INSTALLER" > "$W/install.sh"

# A CI container has no systemd user session; the preflight only needs the
# probe to succeed.
printf '#!/bin/sh\nexit 0\n' > "$W/shim/systemctl"
chmod +x "$W/shim/systemctl"

# 1. A correctly signed release passes verification + preflight, and the
# preflight reports which OpenSSL is installed / installable (the data used
# to pick the right tarball variant per distro).
HOME=$W/home PATH=$W/shim:$PATH sh "$W/install.sh" --dry-run --yes --local-assets "$W/assets" | tee "$W/ok.log"
grep -q '^OpenSSL (installed): ' "$W/ok.log" || { echo "preflight did not report the installed OpenSSL" >&2; exit 1; }
grep -q '^OpenSSL (installable): ' "$W/ok.log" || { echo "preflight did not report the installable OpenSSL" >&2; exit 1; }

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
# 4. Build selection follows the host's OpenSSL: libssl.so.4 -> generic-ossl4,
# otherwise generic. A fake ldconfig stands in for the host; only the
# "Selected build" line matters here (the preflight may fail on the real
# libraries when the picked variant does not match this machine).
FIRST=$(ls "$W"/assets/*.tar.gz | head -n1)
BASE=$(basename "$FIRST" | sed 's/-[^-]*\.tar\.gz$//')
mkdir -p "$W/sel" "$W/ssl3" "$W/ssl4"
cp "$W/assets/SHA256SUMS" "$W/assets/SHA256SUMS.sig" "$W/sel/" 2>/dev/null || true
cp "$FIRST" "$W/sel/$BASE-generic.tar.gz"; cp "$FIRST" "$W/sel/$BASE-generic-ossl4.tar.gz"
printf '#!/bin/sh\necho "\tlibssl.so.4 (libc6,x86-64) => /usr/lib64/libssl.so.4"\n' > "$W/ssl4/ldconfig"
printf '#!/bin/sh\necho "\tlibssl.so.3 (libc6,x86-64) => /usr/lib64/libssl.so.3"\n' > "$W/ssl3/ldconfig"
chmod +x "$W/ssl4/ldconfig" "$W/ssl3/ldconfig"
for want in ssl4:generic-ossl4 ssl3:generic; do
    dir=${want%%:*}; target=${want#*:}
    HOME=$W/home PATH=$W/$dir:$W/shim:$PATH sh "$W/install.sh" --dry-run --yes --local-assets "$W/sel" >"$W/sel.log" 2>&1 || true
    grep -q "Selected build: .*-$target\.tar\.gz" "$W/sel.log" \
        || { echo "host with $dir did not select $target" >&2; cat "$W/sel.log" >&2; exit 1; }
done
echo "installer dry-run tests passed"
