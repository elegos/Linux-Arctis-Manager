#!/bin/sh
# Linux Arctis Manager - user-level installer (install / upgrade / uninstall).
#
#   curl --proto '=https' --tlsv1.2 -fsSL \
#     https://github.com/elegos/Linux-Arctis-Manager/releases/latest/download/install.sh | sh
#
# Everything lands under ~/.local (and ~/.config for the systemd units and
# the WirePlumber drop-in); nothing is installed system-wide. The only step
# that needs root is giving the HID helper its capability (setcap) - it is
# announced first and you must confirm it.
#
# Usage: install.sh [install|upgrade|uninstall] [options]
#   (no command)        install, or upgrade an existing install
#   uninstall           remove everything this script installed
#   --pre-release       consider pre-releases too (default: stable only)
#   --version X.Y.Z     install that exact version
#   --reinstall         reinstall even if that version is already installed
#   --purge             with uninstall: also delete ~/.config/arctis_manager
#   --yes               don't wait for confirmations (non-interactive)
#   --dry-run           download, verify and check prerequisites, change nothing
#   --diagnose          print an environment report for bug reports, then exit
#   --prefix DIR        install under DIR instead of ~/.local
#   --local-assets DIR  use already-downloaded release assets (offline/testing)
#   -h, --help
#
# Trust model: the release's SHA256SUMS is signed with the maintainer's SSH
# key (public key embedded below); the signature is verified with the
# `ssh-keygen -Y verify` already present on virtually every system, and the
# tarball must match the signed checksum before anything is extracted or run.
# This script itself is only protected by TLS - it is the root of that chain.
set -eu

REPO='elegos/Linux-Arctis-Manager'
ISSUES_URL="https://github.com/$REPO/issues/new/choose"
SIGNING_NAMESPACE='lam-release'
# The release signing public key ("ssh-keygen -Y sign" key, type + base64).
# Fingerprint is also published in the README; compare out of band if paranoid.
SIGNING_PUBKEY='ssh-ed25519 PLACEHOLDER-REPLACE-WITH-RELEASE-PUBLIC-KEY'

CMD=install
CHANNEL=stable
WANT_VERSION=
REINSTALL=0
PURGE=0
ASSUME_YES=0
DRY_RUN=0
DIAGNOSE=0
PREFIX=${HOME:-}/.local
LOCAL_ASSETS=

TMP=
MANIFEST=
TAG=
VER=
TARGET=
TARBALL=
OS_ID=unknown
OS_NAME=unknown
OS_FAMILY=unknown
ARCH=
USE_COLOR=0

if [ -t 1 ]; then USE_COLOR=1; fi

# ── output helpers ─────────────────────────────────────────────────────────────
c() { # c <ansi-code> <text>
    if [ "$USE_COLOR" = 1 ]; then printf '\033[%sm%s\033[0m' "$1" "$2"; else printf '%s' "$2"; fi
}
say()  { printf '%s\n' "$*"; }
step() { printf '%s %s\n' "$(c '1;34' '==>')" "$*"; }
warn() { printf '%s %s\n' "$(c '1;33' 'warning:')" "$*" >&2; }
sec()  { printf '%s %s\n' "$(c '1;35' '[security]')" "$*"; }
die()  { printf '%s %s\n' "$(c '1;31' 'error:')" "$*" >&2; exit 1; }

cleanup() {
    if [ -n "$TMP" ] && [ -d "$TMP" ]; then rm -rf "$TMP"; fi
}
trap cleanup EXIT
trap 'printf "\nAborted.\n" >&2; exit 130' INT TERM HUP

# confirm <what is about to happen>: every step that touches security-relevant
# state waits for Enter (or Ctrl+C to abort) unless --yes was given.
confirm() {
    printf '\n%s\n' "$1"
    if [ "$ASSUME_YES" = 1 ]; then
        say "(--yes given: continuing)"
        return 0
    fi
    # Read from the terminal, not stdin: under `curl | sh` stdin is the script.
    if ! ( : </dev/tty ) 2>/dev/null; then
        die "no terminal available to ask for confirmation; re-run with --yes to accept the step above"
    fi
    printf '%s ' "If you do not agree press Ctrl+C, otherwise press Enter to continue..."
    IFS= read -r _ </dev/tty
}

usage() {
    cat <<'EOF'
Usage: install.sh [install|upgrade|uninstall] [options]
  (no command)        install, or upgrade an existing install
  uninstall           remove everything this script installed
  --pre-release       consider pre-releases too (default: stable only)
  --version X.Y.Z     install that exact version
  --reinstall         reinstall even if that version is already installed
  --purge             with uninstall: also delete ~/.config/arctis_manager
  --yes               don't wait for confirmations (non-interactive)
  --dry-run           download, verify and check prerequisites, change nothing
  --diagnose          print an environment report for bug reports, then exit
  --prefix DIR        install under DIR instead of ~/.local
  --local-assets DIR  use already-downloaded release assets (offline/testing)
  -h, --help
EOF
}

# ── argument parsing ───────────────────────────────────────────────────────────
parse_args() {
    while [ $# -gt 0 ]; do
        case $1 in
            install|upgrade) CMD=install ;;
            uninstall)       CMD=uninstall ;;
            --pre-release)   CHANNEL=pre-release ;;
            --version)       [ $# -ge 2 ] || die "--version needs a value"; WANT_VERSION=${2#v}; shift ;;
            --reinstall)     REINSTALL=1 ;;
            --purge)         PURGE=1 ;;
            --yes|-y)        ASSUME_YES=1 ;;
            --dry-run)       DRY_RUN=1 ;;
            --diagnose)      DIAGNOSE=1 ;;
            --prefix)        [ $# -ge 2 ] || die "--prefix needs a value"; PREFIX=$2; shift ;;
            --local-assets)  [ $# -ge 2 ] || die "--local-assets needs a value"; LOCAL_ASSETS=$2; shift ;;
            -h|--help)       usage; exit 0 ;;
            *)               die "unknown argument: $1 (try --help)" ;;
        esac
        shift
    done
}

# ── environment detection ──────────────────────────────────────────────────────
os_field() { ( . /etc/os-release 2>/dev/null || exit 0; eval "printf '%s' \"\${$1:-}\"" ); }

detect_env() {
    [ -n "${HOME:-}" ] || die "\$HOME is not set"
    [ "$(id -u)" != 0 ] || die "do not run this as root: it installs into your own home directory (it asks for privileges only for the one step that needs them)"
    [ "$(uname -s)" = Linux ] || die "Linux only (found $(uname -s))"
    ARCH=$(uname -m)
    case $ARCH in
        x86_64|aarch64) ;;
        *) die "unsupported architecture: $ARCH (builds exist for x86_64 and aarch64)" ;;
    esac
    OS_ID=$(os_field ID | tr 'A-Z' 'a-z' | tr -c 'a-z0-9_\n-' '_')
    [ -n "$OS_ID" ] || OS_ID=unknown
    OS_NAME=$(os_field PRETTY_NAME)
    [ -n "$OS_NAME" ] || OS_NAME=$OS_ID
    ID_LIKE=$(os_field ID_LIKE | tr 'A-Z' 'a-z')
    case " $OS_ID $ID_LIKE " in
        *" arch "*|*" steamos "*)                       OS_FAMILY=arch ;;
        *" debian "*|*" ubuntu "*)                      OS_FAMILY=debian ;;
        *" fedora "*|*" rhel "*|*" centos "*|*" suse "*) OS_FAMILY=rpm ;;
    esac

    XDG_CONFIG=${XDG_CONFIG_HOME:-$HOME/.config}
    BINDIR=$PREFIX/bin
    LIBEXECDIR=$PREFIX/libexec
    DATADIR=$PREFIX/share
    LAMDATA=$DATADIR/linux-arctis-manager
    LIBDIR=$PREFIX/lib/linux-arctis-manager
    UNITDIR=$XDG_CONFIG/systemd/user
    RECEIPT=$LIBDIR/install-receipt
    HELPER=$LIBEXECDIR/lam-hidraw-helper
    UNIT_MARK='# Installed by Linux Arctis Manager install.sh'
}

have() { command -v "$1" >/dev/null 2>&1; }

# ver_ge A B: is version A >= B (dotted numbers)?
ver_ge() { [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -n1)" = "$2" ]; }

fetch() { # fetch <url> <dest>
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --connect-timeout 15 -o "$2" "$1"
}

# ── receipt ────────────────────────────────────────────────────────────────────
receipt_get() { [ -f "$RECEIPT" ] && sed -n "s/^$1=//p" "$RECEIPT" | head -n1 || true; }

# ── conflict detection (distro packages / v2 / foreign units) ──────────────────
pkg_owner() { # pkg_owner <path> -> package name or empty
    if have dpkg;   then dpkg -S "$1" 2>/dev/null | head -n1 | sed 's/:.*//' && return 0; fi
    if have rpm;    then rpm -qf "$1" 2>/dev/null | grep -v 'not owned' | head -n1 && return 0; fi
    if have pacman; then pacman -Qoq "$1" 2>/dev/null | head -n1 && return 0; fi
    return 0
}

check_conflicts() {
    found=
    for f in /usr/bin/lam-daemon /usr/local/bin/lam-daemon /usr/bin/lam-gui /usr/local/bin/lam-gui \
             /usr/libexec/lam-hidraw-helper /usr/local/libexec/lam-hidraw-helper \
             /usr/lib/systemd/user/lam-daemon.service /usr/local/lib/systemd/user/lam-daemon.service; do
        [ -e "$f" ] && found="$found $f"
    done
    if [ -e "$UNITDIR/lam-daemon.service" ] && ! grep -qF "$UNIT_MARK" "$UNITDIR/lam-daemon.service" 2>/dev/null; then
        found="$found $UNITDIR/lam-daemon.service"
    fi
    [ -n "$found" ] || return 0

    warn "an existing Linux Arctis Manager installation not managed by this script was found:"
    pkgs=
    manual=
    for f in $found; do
        p=$(pkg_owner "$f" || true)
        if [ -n "$p" ]; then
            case " $pkgs " in *" $p "*) ;; *) pkgs="$pkgs $p" ;; esac
        else
            manual="$manual $f"
        fi
    done
    for p in $pkgs; do
        case $OS_FAMILY in
            debian) say "  - installed as package '$p'  ->  sudo apt remove $p" ;;
            rpm)    say "  - installed as package '$p'  ->  sudo dnf remove $p" ;;
            arch)   say "  - installed as package '$p'  ->  sudo pacman -R $p" ;;
            *)      say "  - installed as package '$p' (remove it with your package manager)" ;;
        esac
    done
    for f in $manual; do
        say "  - not owned by any package (remove it by hand, e.g. an old v2 install): $f"
    done
    die "remove the above first, then run this installer again (two copies would fight over the same services)"
}

# ── release resolution ─────────────────────────────────────────────────────────
# Prints "tag|prerelease|asset,asset,..." per release, newest first. Relies on
# GitHub's pretty-printed JSON layout (release keys at 4 spaces, asset keys at
# 8), which avoids needing jq/python on the target.
list_releases() {
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --connect-timeout 15 \
        -H 'Accept: application/vnd.github+json' \
        "https://api.github.com/repos/$REPO/releases?per_page=100" | awk '
        function val(s) { sub(/^[^:]*: *"/, "", s); sub(/".*$/, "", s); return s }
        /^  \{/                 { tag = ""; pre = "false"; assets = "" }
        /^    "tag_name":/      { tag = val($0) }
        /^    "prerelease":/    { pre = ($0 ~ /true/) ? "true" : "false" }
        /^        "name":/      { assets = assets (assets == "" ? "" : ",") val($0) }
        /^  \},?$/              { if (tag != "") print tag "|" pre "|" assets }'
}

# Sets TAG, VER, TARBALL (asset name), BASE_URL. Prefers an OS-specific build,
# falls back to "generic"; skips releases lacking the tarball or its signature.
resolve_release() {
    TARGETS="$OS_ID generic"
    [ "$OS_ID" = generic ] && TARGETS=generic
    listing=$(list_releases) || die "could not query GitHub releases (network down or API rate limit hit?)"
    TAG=
    set -f
    for line in $listing; do
        rtag=${line%%|*}; rest=${line#*|}; rpre=${rest%%|*}; assets=${rest#*|}
        [ "$rpre" = true ] && [ "$CHANNEL" = stable ] && continue
        rver=${rtag#v}
        if [ -n "$WANT_VERSION" ] && [ "$rver" != "$WANT_VERSION" ]; then continue; fi
        case ",$assets," in *,SHA256SUMS,*) ;; *) continue ;; esac
        case ",$assets," in *,SHA256SUMS.sig,*) ;; *) continue ;; esac
        for t in $TARGETS; do
            name="lam-${rver}-${ARCH}-${t}.tar.gz"
            case ",$assets," in
                *",$name,"*) TAG=$rtag; VER=$rver; TARBALL=$name; TARGET=$t; break ;;
            esac
        done
        [ -n "$TAG" ] && break
    done
    set +f
    if [ -z "$TAG" ]; then
        if [ -n "$WANT_VERSION" ]; then
            die "no release $WANT_VERSION with a user-level $ARCH build was found"
        fi
        die "no $CHANNEL release with a user-level $ARCH build was found (try --pre-release, or check $ISSUES_URL)"
    fi
    BASE_URL="https://github.com/$REPO/releases/download/$TAG"
    if [ "$TARGET" = generic ] && [ "$OS_ID" != generic ] && [ "$OS_ID" != unknown ]; then
        say "Using the generic build (there is none specific to '$OS_ID')."
        say "If something turns out not to work, please report it: $ISSUES_URL"
    fi
}

# ── download + verify ──────────────────────────────────────────────────────────
download_and_verify() {
    case $SIGNING_PUBKEY in *PLACEHOLDER*) die "this copy of the installer has no release signing key embedded; refusing to install unverified binaries" ;; esac
    have ssh-keygen || die "ssh-keygen (OpenSSH client) is required to verify the release signature"
    have sha256sum  || die "sha256sum is required"
    have tar        || die "tar is required"

    DL=$TMP/dl; mkdir -p "$DL"
    if [ -n "$LOCAL_ASSETS" ]; then
        step "Using local release assets from $LOCAL_ASSETS"
        for f in "$TARBALL" SHA256SUMS SHA256SUMS.sig; do
            [ -f "$LOCAL_ASSETS/$f" ] || die "missing $LOCAL_ASSETS/$f"
            cp "$LOCAL_ASSETS/$f" "$DL/$f"
        done
    else
        step "Downloading $TARBALL ($TAG)"
        for f in "$TARBALL" SHA256SUMS SHA256SUMS.sig; do
            fetch "$BASE_URL/$f" "$DL/$f" || die "download failed: $BASE_URL/$f"
        done
    fi

    step "Verifying the release signature"
    printf '%s namespaces="%s" %s\n' "$SIGNING_NAMESPACE" "$SIGNING_NAMESPACE" "$SIGNING_PUBKEY" > "$DL/allowed_signers"
    if ! sigout=$(ssh-keygen -Y verify -f "$DL/allowed_signers" -I "$SIGNING_NAMESPACE" \
            -n "$SIGNING_NAMESPACE" -s "$DL/SHA256SUMS.sig" < "$DL/SHA256SUMS" 2>&1); then
        die "signature verification FAILED - nothing was installed or run. ($sigout)"
    fi
    sec "SHA256SUMS signature OK: $(echo "$sigout" | sed -n 's/.*key \(.*\)$/\1/p')"
    ( cd "$DL" && grep -F " $TARBALL" SHA256SUMS | sed 's/ \*/  /' | sha256sum -c - >/dev/null 2>&1 ) \
        || die "checksum mismatch for $TARBALL - nothing was installed or run"
    sec "$TARBALL matches the signed checksum"

    step "Extracting"
    mkdir -p "$TMP/x"
    tar -xzf "$DL/$TARBALL" -C "$TMP/x"
    SRC=$TMP/x/$(find "$TMP/x" -mindepth 1 -maxdepth 1 -printf '%f\n' | head -n1)
    MANIFEST=$SRC/manifest.txt
    [ -f "$MANIFEST" ] || die "manifest.txt missing from the tarball"
}

mf() { [ -f "$MANIFEST" ] && sed -n "s/^$1=//p" "$MANIFEST"; }

# ── preflight (read-only; runs before anything is modified) ────────────────────
pkg_hint() { # pkg_hint <lib-or-command> -> package name for this distro family
    case "$OS_FAMILY:$1" in
        debian:libpulse.so.0)  echo libpulse0 ;;        rpm:libpulse.so.0) echo pulseaudio-libs ;;   arch:libpulse.so.0) echo libpulse ;;
        debian:libudev.so.1)   echo libudev1 ;;         rpm:libudev.so.1)  echo systemd-libs ;;      arch:libudev.so.1)  echo systemd-libs ;;
        debian:libssl.so.*|debian:libcrypto.so.*) echo libssl3 ;;
        rpm:libssl.so.*|rpm:libcrypto.so.*)       echo openssl-libs ;;
        arch:libssl.so.*|arch:libcrypto.so.*)     echo openssl ;;
        debian:libgcc_s.so.1)  echo libgcc-s1 ;;        rpm:libgcc_s.so.1) echo libgcc ;;            arch:libgcc_s.so.1) echo gcc-libs ;;
        debian:setcap|debian:getcap) echo libcap2-bin ;; rpm:setcap|rpm:getcap) echo libcap ;;       arch:setcap|arch:getcap) echo libcap ;;
        debian:pkexec) echo policykit-1 ;;              rpm:pkexec) echo polkit ;;                   arch:pkexec) echo polkit ;;
        *) echo "" ;;
    esac
}

PRE_PROBLEMS=
add_problem() { PRE_PROBLEMS="$PRE_PROBLEMS
  - $1"; }

preflight() {
    step "Checking prerequisites"
    m_arch=$(mf arch)
    [ "$m_arch" = "$ARCH" ] || add_problem "build is for $m_arch but this machine is $ARCH"

    glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
    gmin=$(mf glibc_min)
    if [ -z "$glibc" ]; then
        add_problem "glibc not detected (musl-based systems such as Alpine are not supported)"
    elif ! ver_ge "$glibc" "$gmin"; then
        add_problem "glibc $glibc is older than the required $gmin (this OS is too old for the prebuilt binaries)"
    fi

    for cmd in $(mf cmd); do
        have "$cmd" || add_problem "missing command: $cmd$(h=$(pkg_hint "$cmd"); [ -n "$h" ] && echo " (package: $h)")"
    done
    for alt in $(mf cmd_any); do
        ok=0; for cmd in $(echo "$alt" | tr '|' ' '); do have "$cmd" && ok=1; done
        [ "$ok" = 1 ] || add_problem "need one of: $(echo "$alt" | sed 's/|/, /g')"
    done
    have setcap || add_problem "missing command: setcap$(h=$(pkg_hint setcap); [ -n "$h" ] && echo " (package: $h)")"

    if have systemctl; then
        systemctl --user show-environment >/dev/null 2>&1 \
            || add_problem "no systemd user session reachable (systemctl --user failed); log in graphically or enable lingering"
    fi

    if have ldd; then
        for bin in "$SRC/bin/lam-daemon" "$SRC/libexec/lam-hidraw-helper"; do
            for lib in $(ldd "$bin" 2>&1 | sed -n 's/^[[:space:]]*\(.*\) => not found.*/\1/p'); do
                h=$(pkg_hint "$lib")
                add_problem "missing shared library for $(basename "$bin"): $lib${h:+ (package: $h)}"
            done
        done
    else
        warn "ldd not found: skipping the shared-library check"
    fi

    if have pactl; then
        pactl info >/dev/null 2>&1 || warn "no PipeWire/PulseAudio server answered (pactl info); the daemon needs one at run time"
    else
        warn "pactl not found: cannot verify that a PipeWire/PulseAudio server is running"
    fi
}

report() { # environment block for bug reports
    cat <<EOF
\`\`\`
Installer:     install.sh ($CMD, channel=$CHANNEL${WANT_VERSION:+, version=$WANT_VERSION})
OS:            $OS_NAME (ID=$OS_ID, family=$OS_FAMILY, VARIANT_ID=$(os_field VARIANT_ID))
Kernel/arch:   $(uname -sr) / $ARCH
glibc:         $(getconf GNU_LIBC_VERSION 2>/dev/null || echo none)
Release:       ${TAG:-unresolved} / ${TARBALL:-n/a}
Build floor:   glibc >= $(mf glibc_min 2>/dev/null || echo '?')
systemd:       $(systemctl --version 2>/dev/null | head -n1 || echo none)
Audio server:  $(pactl info 2>/dev/null | sed -n 's/^Server Name: //p' || echo unknown)
Problems:${PRE_PROBLEMS:- none}
\`\`\`
EOF
}

preflight_or_exit() {
    if [ -n "$PRE_PROBLEMS" ]; then
        printf '\n%s Prerequisites are not met - nothing was changed:%s\n' "$(c '1;31' 'error:')" "$PRE_PROBLEMS" >&2
        if [ "$OS_ID" = steamos ]; then
            say "" >&2
            say "On SteamOS the root filesystem is read-only and system updates erase manual package installs;" >&2
            say "if a library is missing here, that is a bug in our build - please report it." >&2
        else
            say "" >&2
            say "Install the missing pieces with your package manager and run the installer again." >&2
        fi
        say "If you think this is a bug, open an issue and paste this report: $ISSUES_URL" >&2
        report >&2
        exit 1
    fi
    sec "all prerequisites satisfied"
}

# ── installation ───────────────────────────────────────────────────────────────
NEWRECEIPT=
track() { printf '%s=%s\n' "$1" "$2" >> "$NEWRECEIPT"; }

install_file() { # install_file <mode> <src> <dst>   (rename over: no ETXTBSY)
    mkdir -p "$(dirname "$3")"
    cp "$2" "$3.new.$$"; chmod "$1" "$3.new.$$"; mv -f "$3.new.$$" "$3"
    track file "$3"
}

replace_dir() { # replace_dir <src> <dst>
    rm -rf "$2"; mkdir -p "$(dirname "$2")"; cp -a "$1" "$2"
    track dir "$2"
}

ROLLBACK=
rollback_stash() { # move an existing path aside (rename keeps xattrs/owner)
    [ -e "$1" ] || return 0
    mkdir -p "$ROLLBACK"
    mv "$1" "$ROLLBACK/$(echo "$1" | tr '/' '%')"
}
rollback_copy() { # hardlinked backup copy of a directory tree
    [ -e "$1" ] || return 0
    mkdir -p "$ROLLBACK"
    cp -al "$1" "$ROLLBACK/$(echo "$1" | tr '/' '%')"
}
rollback_restore() {
    [ -d "$ROLLBACK" ] || return 0
    for f in "$ROLLBACK"/*; do
        [ -e "$f" ] || continue
        orig=$(basename "$f" | tr '%' '/')
        rm -rf "$orig"; mv "$f" "$orig"
    done
}

setup_python() {
    uvdir=$LIBDIR/uv
    mkdir -p "$uvdir"
    uv_asset=$(mf uv_asset); uv_sha=$(mf uv_sha256); uv_ver=$(mf uv_version)
    if [ ! -x "$uvdir/uv" ] || [ "$("$uvdir/uv" --version 2>/dev/null | awk '{print $2}')" != "$uv_ver" ]; then
        step "Downloading uv $uv_ver"
        fetch "https://github.com/astral-sh/uv/releases/download/$uv_ver/$uv_asset" "$TMP/$uv_asset" \
            || die "uv download failed"
        echo "$uv_sha  $TMP/$uv_asset" | sha256sum -c - >/dev/null 2>&1 \
            || die "uv checksum mismatch (expected $uv_sha, pinned in the signed manifest)"
        sec "uv archive matches the checksum pinned in the signed manifest"
        mkdir -p "$TMP/uvx"; tar -xzf "$TMP/$uv_asset" -C "$TMP/uvx"
        cp "$(find "$TMP/uvx" -name uv -type f | head -n1)" "$uvdir/uv.new"; chmod 755 "$uvdir/uv.new"; mv -f "$uvdir/uv.new" "$uvdir/uv"
    fi
    export UV_NO_CONFIG=1 UV_PYTHON_INSTALL_DIR="$LIBDIR/python" UV_PYTHON_PREFERENCE=only-managed \
           UV_LINK_MODE=copy
    py=$(mf python)
    step "Preparing Python $py (standalone build, verified by uv)"
    "$uvdir/uv" python install --no-bin "$py" >/dev/null 2>&1 || die "could not install the standalone Python $py"
    venv=$LIBDIR/venv
    lock=$SRC/python/requirements.lock.txt
    uvpy="$venv/bin/python"
    cur=
    if [ -x "$uvpy" ]; then cur=$("$uvpy" -c 'import sys; print("%d.%d" % sys.version_info[:2])' 2>/dev/null || true); fi
    if [ -n "$cur" ] && [ "$cur" = "$py" ]; then
        # Upgrade in place: only the packages that differ are fetched. The
        # rollback copy is hardlinked (same filesystem), so it costs ~nothing.
        rollback_copy "$venv"
        if cmp -s "$lock" "$LIBDIR/python.lock" 2>/dev/null; then
            step "Python dependencies unchanged, updating the application only"
        else
            step "Updating the Python dependencies (only what changed is downloaded)"
            "$uvdir/uv" pip sync --quiet --python "$uvpy" --require-hashes "$lock"
        fi
        "$uvdir/uv" pip install --quiet --python "$uvpy" --reinstall --no-deps "$SRC"/python/*.whl
    else
        rollback_stash "$venv"
        step "Creating the virtual environment and installing locked dependencies"
        "$uvdir/uv" venv --quiet --python "$py" "$venv"
        "$uvdir/uv" pip install --quiet --python "$uvpy" --require-hashes --no-deps -r "$lock"
        "$uvdir/uv" pip install --quiet --python "$uvpy" --no-deps "$SRC"/python/*.whl
    fi
    rollback_stash "$LIBDIR/python.lock"
    cp "$lock" "$LIBDIR/python.lock"
    track dir "$LIBDIR/venv"
}

write_gui_wrapper() {
    mkdir -p "$BINDIR"
    cat > "$BINDIR/lam-gui.new.$$" <<EOF
#!/bin/sh
exec "$LIBDIR/venv/bin/python3" \\
     -m linux_arctis_manager.scripts.gui "\$@"
EOF
    chmod 755 "$BINDIR/lam-gui.new.$$"; mv -f "$BINDIR/lam-gui.new.$$" "$BINDIR/lam-gui"
    track file "$BINDIR/lam-gui"
}

install_units() {
    mkdir -p "$UNITDIR/lam-daemon.service.d"
    for u in lam-hidraw-helper lam-daemon; do
        { echo "$UNIT_MARK"
          sed -e "s|@BINDIR@|$BINDIR|g" -e "s|@LIBEXECDIR@|$LIBEXECDIR|g" "$SRC/systemd/user/$u.service.in"; } \
            > "$UNITDIR/$u.service"
        track file "$UNITDIR/$u.service"
    done
    # The binaries bake in the packaged data dir; point them at this prefix.
    { echo "$UNIT_MARK"; echo '[Service]'; echo "Environment=LAM_DATADIR=$LAMDATA"; } \
        > "$UNITDIR/lam-daemon.service.d/10-lam-datadir.conf"
    track file "$UNITDIR/lam-daemon.service.d/10-lam-datadir.conf"
    track dir_if_empty "$UNITDIR/lam-daemon.service.d"
}

install_files() {
    install_file 755 "$SRC/bin/lam-daemon" "$BINDIR/lam-daemon"
    install_file 755 "$SRC/libexec/lam-hidraw-helper" "$HELPER"
    replace_dir "$SRC/share/linux-arctis-manager" "$LAMDATA"
    replace_dir "$SRC/share/plasma/plasmoids/name.giacomofurlan.arctismanager" "$DATADIR/plasma/plasmoids/name.giacomofurlan.arctismanager"
    replace_dir "$SRC/share/gnome-shell/extensions/arctis-manager@giacomofurlan.name" "$DATADIR/gnome-shell/extensions/arctis-manager@giacomofurlan.name"
    replace_dir "$SRC/share/wireplumber/scripts/lam" "$DATADIR/wireplumber/scripts/lam"
    install_file 644 "$SRC/share/wireplumber/wireplumber.conf.d/51-lam-sink-visibility.conf" \
        "$XDG_CONFIG/wireplumber/wireplumber.conf.d/51-lam-sink-visibility.conf"
    for ic in arctis-manager arctis-manager-symbolic; do
        install_file 644 "$SRC/share/icons/hicolor/scalable/apps/$ic.svg" "$DATADIR/icons/hicolor/scalable/apps/$ic.svg"
    done
    for d in "$SRC"/share/applications/*.desktop; do
        out=$DATADIR/applications/$(basename "$d")
        mkdir -p "$DATADIR/applications"
        sed "s|^Exec=lam-gui|Exec=$BINDIR/lam-gui|" "$d" > "$out"; chmod 644 "$out"
        track file "$out"
    done
    write_gui_wrapper
    install_units
}

apply_capability() {
    # Idempotent: skip when the helper already carries the capability and is
    # unchanged since the last install (a replaced file loses its xattrs).
    if have getcap && getcap "$HELPER" 2>/dev/null | grep -q cap_dac_override \
            && [ "$(stat -c %U "$HELPER")" = root ]; then
        return 0
    fi
    confirm "$(sec 'Privileged step (the only one)')
  What:  run  chown root:root  and  setcap cap_dac_override+eip  on
           $HELPER
  Why:   the helper hands the daemon an open handle to your SteelSeries HID
         device nodes (/dev/hidraw*) without any udev rules. It checks that
         the device is a SteelSeries one and that the caller is your own user.
         Owned by root, nothing running as you can modify the binary.
  How:   via $(have pkexec && echo pkexec || echo sudo) (you will be asked for your password)."
    sh_path=$(command -v sh)
    if have pkexec && { [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]; }; then
        pkexec "$sh_path" -c 'chown root:root "$1" && setcap cap_dac_override+eip "$1"' sh "$HELPER" \
            || die "the privileged step failed or was cancelled"
    elif have sudo; then
        sudo "$sh_path" -c 'chown root:root "$1" && setcap cap_dac_override+eip "$1"' sh "$HELPER" \
            || die "the privileged step failed or was cancelled"
    else
        die "need pkexec (with a graphical session) or sudo; run manually: sudo sh -c 'chown root:root \"$HELPER\" && setcap cap_dac_override+eip \"$HELPER\"'"
    fi
    if have getcap && ! getcap "$HELPER" | grep -q cap_dac_override; then
        die "the capability is not set on $HELPER (filesystem without xattr support?)"
    fi
    if have findmnt && findmnt -no OPTIONS --target "$LIBEXECDIR" 2>/dev/null | grep -qw nosuid; then
        warn "$LIBEXECDIR is on a 'nosuid' mount: the kernel ignores file capabilities there, the helper will not work"
    fi
    sec "capability set on $HELPER"
}

refresh_desktop_caches() {
    if have update-desktop-database; then update-desktop-database "$DATADIR/applications" >/dev/null 2>&1 || true; fi
    if have kbuildsycoca6; then kbuildsycoca6 >/dev/null 2>&1 || true; fi
}

gui_libs_check() {
    have ldd || return 0
    missing=$(for so in $(find "$LIBDIR/venv" -name 'QtWidgets*.so' -o -name 'libQt6Widgets.so*' 2>/dev/null | head -n2); do
        ldd "$so" 2>/dev/null | sed -n 's/^[[:space:]]*\(.*\) => not found.*/\1/p'; done | sort -u)
    if [ -n "$missing" ]; then
        warn "the GUI may not start: missing system libraries: $(echo $missing)"
        warn "the daemon is unaffected. If installing them does not help, report it: $ISSUES_URL"
    fi
}

start_services() {
    systemctl --user daemon-reload
    systemctl --user enable lam-hidraw-helper.service lam-daemon.service >/dev/null 2>&1
    if systemctl --user is-active --quiet lam-daemon.service; then
        systemctl --user restart lam-hidraw-helper.service lam-daemon.service
    else
        systemctl --user start lam-hidraw-helper.service lam-daemon.service
    fi
    i=0
    while [ $i -lt 15 ]; do
        if systemctl --user is-active --quiet lam-daemon.service \
                && systemctl --user is-active --quiet lam-hidraw-helper.service; then return 0; fi
        i=$((i + 1)); sleep 1
    done
    return 1
}

# remove_entries <receipt-like file>: delete the files/dirs it lists.
remove_entries() {
    sed -n 's/^file=//p' "$1" | while IFS= read -r f; do rm -f "$f"; done
    sed -n 's/^dir=//p' "$1" | while IFS= read -r d; do rm -rf "$d"; done
    sed -n 's/^dir_if_empty=//p' "$1" | while IFS= read -r d; do rmdir "$d" 2>/dev/null || true; done
}

do_install() {
    check_conflicts

    old_ver=$(receipt_get version || true)
    resolve_release_or_local
    if [ -n "$old_ver" ] && [ "$old_ver" = "$VER" ] && [ "$REINSTALL" = 0 ] && [ "$DIAGNOSE" = 0 ]; then
        say "Version $VER is already installed (use --reinstall to redo it)."
        exit 0
    fi
    download_and_verify
    preflight
    if [ "$DIAGNOSE" = 1 ]; then report; exit 0; fi
    preflight_or_exit

    action=Installing; [ -n "$old_ver" ] && action="Upgrading $old_ver ->"
    if [ "$DRY_RUN" = 1 ]; then
        say ""; say "$action $VER ($TARGET build) - dry run, nothing was changed."
        exit 0
    fi

    confirm "$(sec 'Installation plan')
  $action $VER ($TARGET build, glibc >= $(mf glibc_min))
  Files:    $BINDIR, $LIBEXECDIR, $DATADIR (apps, icons, widgets), $LIBDIR (Python)
  Config:   $UNITDIR (systemd user units), $XDG_CONFIG/wireplumber (audio policy drop-in)
  Network:  downloads uv $(mf uv_version) and a standalone Python $(mf python) (checksums pinned
            in the signed manifest), then the locked Python dependencies from PyPI (hash-pinned).
  Nothing outside your home directory is touched, except the single privileged step announced later."

    ROLLBACK=$LIBDIR/.rollback
    rm -rf "$ROLLBACK"; mkdir -p "$LIBDIR"
    NEWRECEIPT=$TMP/receipt; : > "$NEWRECEIPT"
    if [ -f "$RECEIPT" ]; then cp "$RECEIPT" "$TMP/receipt.old"; fi

    if systemctl --user is-active --quiet lam-daemon.service 2>/dev/null; then
        step "Stopping the running services"
        systemctl --user stop lam-daemon.service lam-hidraw-helper.service || true
    fi
    rollback_stash "$BINDIR/lam-daemon"; rollback_stash "$HELPER"; rollback_stash "$BINDIR/lam-gui"

    # errexit is ignored inside a subshell used as an `if` condition, so
    # capture the status explicitly.
    set +e
    ( set -e; setup_python; install_files )
    rc=$?
    set -e
    if [ "$rc" != 0 ]; then
        warn "installation failed; restoring the previous state"
        rm -rf "$LIBDIR/venv"
        remove_entries "$NEWRECEIPT"
        rollback_restore
        rm -rf "$ROLLBACK"
        if [ -n "$old_ver" ]; then systemctl --user start lam-hidraw-helper.service lam-daemon.service 2>/dev/null || true; fi
        die "installation failed (nothing from the new version is active). Report it: $ISSUES_URL"
    fi
    apply_capability

    confirm "$(sec 'Services')
  Enable and start  lam-hidraw-helper.service  and  lam-daemon.service  in your user session
  (systemctl --user enable --now). They start at login and are removed by 'uninstall'."
    if ! start_services; then
        warn "the daemon did not become active; recent log:"
        journalctl --user -u lam-daemon.service -u lam-hidraw-helper.service -n 25 --no-pager 2>/dev/null || true
        if [ -n "$old_ver" ]; then
            warn "rolling back to $old_ver"
            systemctl --user stop lam-daemon.service lam-hidraw-helper.service 2>/dev/null || true
            rm -rf "$LIBDIR/venv"; rollback_restore; rm -rf "$ROLLBACK"
            systemctl --user start lam-hidraw-helper.service lam-daemon.service 2>/dev/null || true
        fi
        die "the services failed to start. Report it with 'install.sh --diagnose': $ISSUES_URL"
    fi

    # Receipt: version/channel + every file/dir, so uninstall removes exactly this.
    { echo "version=$VER"; echo "target=$TARGET"; echo "channel=$CHANNEL"; echo "prefix=$PREFIX"; echo "unitdir=$UNITDIR"
      echo "config_home=$XDG_CONFIG"; echo "libdir=$LIBDIR"; echo "dir=$LIBDIR/uv"; echo "dir=$LIBDIR/python"
      cat "$NEWRECEIPT"; } > "$RECEIPT.new" && mv -f "$RECEIPT.new" "$RECEIPT"
    # Drop files the previous version shipped that this one no longer does.
    if [ -f "$TMP/receipt.old" ]; then
        sed -n 's/^file=//p' "$TMP/receipt.old" | while IFS= read -r f; do
            grep -qxF "file=$f" "$RECEIPT" || rm -f "$f"
        done
    fi
    rm -rf "$ROLLBACK"
    refresh_desktop_caches
    gui_libs_check

    if have systemctl && systemctl --user is-active --quiet wireplumber.service 2>/dev/null; then
        if [ "$ASSUME_YES" = 0 ] && ( : </dev/tty ) 2>/dev/null; then
            confirm "$(sec 'WirePlumber')
  The optional \"hide physical sink\" policy script is loaded by WirePlumber at start-up.
  Restarting it now activates it but briefly interrupts audio; otherwise it loads at next login."
            systemctl --user restart wireplumber.service || true
        else
            say "Note: restart WirePlumber (or log out/in) to activate the optional sink-hiding script."
        fi
    fi

    case ":$PATH:" in *":$BINDIR:"*) ;; *) warn "$BINDIR is not in your PATH (the desktop entries use absolute paths and work anyway)" ;; esac
    say ""
    step "Linux Arctis Manager $VER installed. Launch it from your application menu, or run: $BINDIR/lam-gui"
    say "Issues and device support requests: $ISSUES_URL"
}

resolve_release_or_local() {
    if [ -z "$LOCAL_ASSETS" ]; then resolve_release; return 0; fi
    TARBALL=$(basename "$(find "$LOCAL_ASSETS" -maxdepth 1 -name "lam-*-$ARCH-*.tar.gz" | head -n1)")
    [ "$TARBALL" != . ] && [ -n "$TARBALL" ] || die "no lam-*-$ARCH-*.tar.gz in $LOCAL_ASSETS"
    VER=$(echo "$TARBALL" | sed -n "s/^lam-\(.*\)-$ARCH-.*\.tar\.gz$/\1/p")
    TARGET=$(echo "$TARBALL" | sed -n "s/^lam-.*-$ARCH-\(.*\)\.tar\.gz$/\1/p")
    TAG=v$VER
}

do_uninstall() {
    [ -f "$RECEIPT" ] || die "no install receipt at $RECEIPT: nothing installed by this script under $PREFIX"
    ver=$(receipt_get version)
    confirm "$(sec 'Uninstall')
  Stop and disable the user services, then remove everything installed by version $ver
  (listed in $RECEIPT).$( [ "$PURGE" = 1 ] && echo "
  --purge: ALSO delete your settings in ${XDG_CONFIG_HOME:-$HOME/.config}/arctis_manager.")"
    if have systemctl; then
        systemctl --user disable --now lam-daemon.service lam-hidraw-helper.service >/dev/null 2>&1 || true
    fi
    remove_entries "$RECEIPT"
    rm -rf "$LIBDIR"
    if [ "$PURGE" = 1 ]; then rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/arctis_manager"; fi
    if have systemctl; then systemctl --user daemon-reload || true; fi
    refresh_desktop_caches
    step "Uninstalled. (Restart WirePlumber or log out/in to unload the sink-hiding script.)"
}

main() {
    parse_args "$@"
    detect_env
    have curl || [ -n "$LOCAL_ASSETS" ] || die "curl is required"
    TMP=$(mktemp -d "${TMPDIR:-/tmp}/lam-install.XXXXXX")
    case $CMD in
        install)   do_install ;;
        uninstall) do_uninstall ;;
    esac
}

main "$@"
