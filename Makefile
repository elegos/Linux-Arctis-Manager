# ── Directory variables (override with make PREFIX=/usr install) ───────────────
PREFIX     ?= /usr/local
BINDIR     ?= $(PREFIX)/bin
LIBDIR     ?= $(PREFIX)/lib
LIBEXECDIR ?= $(PREFIX)/libexec
DATADIR    ?= $(PREFIX)/share
SYSTEMD_USER_DIR ?= $(PREFIX)/lib/systemd/user

# Derived paths
DEVICE_CONFIGS_DIR  := $(DATADIR)/linux-arctis-manager/devices
LAM_DATADIR         := $(DATADIR)/linux-arctis-manager
VENVDIR             := $(LIBDIR)/linux-arctis-manager/venv
DESKTOP_DIR         := $(DATADIR)/applications
DESKTOP_FILES       := $(wildcard src/linux_arctis_manager/desktop/*.desktop)

# A standalone copy of the translation files, outside the venv's site-packages
# (whose path is Python-version-dependent), so non-Python UIs — e.g. the
# Plasma widget in packaging/plasma6/ — have a stable path to read them from.
LANG_DIR            := $(LAM_DATADIR)/lang
LANG_FILES          := $(wildcard src/linux_arctis_manager/lang/*.ini)

# KDE Plasma 6 widget (plasmoid). Installed on every distro (harmless on a
# non-Plasma system, same as shipping a .desktop file); Fedora splits it into
# its own subpackage (see packaging/fedora/linux-arctis-manager.spec),
# Debian/Arch don't split it yet (follow-up).
PLASMOID_ID         := name.giacomofurlan.arctismanager
PLASMOID_SRC_DIR    := packaging/plasma6/$(PLASMOID_ID)
PLASMOID_DEST_DIR   := $(DATADIR)/plasma/plasmoids/$(PLASMOID_ID)

# ── Build tools ────────────────────────────────────────────────────────────────
CARGO            ?= cargo
UV               ?= uv
CONTAINER_ENGINE ?= $(shell command -v podman 2>/dev/null || echo docker)

# ── Container build variables ──────────────────────────────────────────────────
FEDORA_VERSION  ?= 44
DEBIAN_VERSION  ?= 24.04
CONTAINER_DIR   := packaging/containers
DIST_DIR        := dist

# Sentinel files: Make uses these as proxies for "base image is built locally".
# Delete a sentinel (or run container-refresh-*) to force an OS update + dep rebuild.
_SENTINEL_FEDORA := .container-base-fedora
_SENTINEL_DEBIAN := .container-base-debian
_SENTINEL_ARCH   := .container-base-arch

# ── Build inputs / outputs ─────────────────────────────────────────────────────
MANIFEST   := daemon/Cargo.toml
HELPER_BIN := daemon/target/release/lam-hidraw-helper
DAEMON_BIN := daemon/target/release/lam-daemon

SERVICE_HELPER_IN  := packaging/systemd/user/lam-hidraw-helper.service.in
SERVICE_DAEMON_IN  := packaging/systemd/user/lam-daemon.service.in
SERVICE_HELPER_OUT := packaging/systemd/user/lam-hidraw-helper.service
SERVICE_DAEMON_OUT := packaging/systemd/user/lam-daemon.service

GUI_WRAPPER_IN  := packaging/scripts/lam-gui.in
GUI_WRAPPER_OUT := packaging/scripts/lam-gui

DEVICE_YAMLS := $(wildcard daemon/device-configs/*.yaml)

.PHONY: build build-python sync-version generate-services generate-gui-wrapper \
        install install-python uninstall enable disable \
        container-build-rpm container-build-deb container-build-pkg container-build-all \
        container-refresh-fedora container-refresh-debian container-refresh-arch container-refresh-all \
        help

# ── Default target ─────────────────────────────────────────────────────────────
help:
	@echo "Targets:"
	@echo "  build              Build Rust release binaries (pass PREFIX= to bake the data dir)"
	@echo "  build-python       Alias: checks uv lockfile is up to date"
	@echo "  install            Build + install everything (requires sudo for setcap)"
	@echo "  install-python     Install Python venv + lam-gui wrapper (called by install)"
	@echo "  uninstall          Remove installed files"
	@echo "  enable             Enable and start user services (no sudo needed)"
	@echo "  disable            Stop and disable user services"
	@echo ""
	@echo "Variables (defaults shown):"
	@echo "  PREFIX=$(PREFIX)"
	@echo "  BINDIR=$(BINDIR)"
	@echo "  LIBDIR=$(LIBDIR)"
	@echo "  LIBEXECDIR=$(LIBEXECDIR)"
	@echo "  DATADIR=$(DATADIR)"
	@echo "  SYSTEMD_USER_DIR=$(SYSTEMD_USER_DIR)"
	@echo "  UV=$(UV)"
	@echo "  DESTDIR (empty by default; used by packaging tools for staged installs)"
	@echo ""
	@echo "Container build targets (Docker/Podman):"
	@echo "  container-build-rpm    Build RPM inside Fedora container → dist/"
	@echo "  container-build-deb    Build .deb inside Ubuntu container → dist/"
	@echo "  container-build-pkg    Build .pkg.tar.zst inside Arch container → dist/"
	@echo "  container-build-all    Build all three package formats"
	@echo "  container-refresh-fedora|debian|arch   Force OS update in base image"
	@echo "  container-refresh-all  Force OS update in all base images"
	@echo ""
	@echo "Container variables (defaults shown):"
	@echo "  CONTAINER_ENGINE=$(CONTAINER_ENGINE)"
	@echo "  FEDORA_VERSION=$(FEDORA_VERSION)"
	@echo "  DEBIAN_VERSION=$(DEBIAN_VERSION)"

# ── Build ──────────────────────────────────────────────────────────────────────
build:
	LAM_DATADIR=$(LAM_DATADIR) $(CARGO) build --release --manifest-path $(MANIFEST)

build-python:
	$(UV) sync --frozen --no-dev

$(HELPER_BIN) $(DAEMON_BIN): build

# ── Generate service files from templates ─────────────────────────────────────
generate-services: $(SERVICE_HELPER_OUT) $(SERVICE_DAEMON_OUT)

$(SERVICE_HELPER_OUT): $(SERVICE_HELPER_IN) Makefile
	sed \
		-e 's|@LIBEXECDIR@|$(LIBEXECDIR)|g' \
		-e 's|@BINDIR@|$(BINDIR)|g' \
		$< > $@

$(SERVICE_DAEMON_OUT): $(SERVICE_DAEMON_IN) Makefile
	sed \
		-e 's|@LIBEXECDIR@|$(LIBEXECDIR)|g' \
		-e 's|@BINDIR@|$(BINDIR)|g' \
		$< > $@

# ── Generate GUI wrapper script from template ──────────────────────────────────
generate-gui-wrapper: $(GUI_WRAPPER_OUT)

$(GUI_WRAPPER_OUT): $(GUI_WRAPPER_IN) Makefile
	sed -e 's|@VENVDIR@|$(VENVDIR)|g' $< > $@
	chmod +x $@

# uv_build (pyproject.toml's build backend) requires a static `version =`
# field — it has no dynamic/file-sourced version support (unlike the Rust
# side's build.rs reading VERSION directly) — so keep pyproject.toml in sync
# by patching it from the same VERSION file right before it's built, instead
# of relying on someone remembering to bump both.
sync-version:
	sed -i 's/^version = .*/version = "$(shell cat VERSION)"/' pyproject.toml

# ── Install Python venv + GUI wrapper ─────────────────────────────────────────
# Plain stdlib venv + pip, not uv: uv itself isn't packaged (or isn't
# packaged under that name) on every distro, which turns "is uv on this
# machine" into its own per-distro compatibility problem — exactly the kind
# of thing this step exists to avoid. venv (python3-venv on Debian/Ubuntu)
# and pip are universal. pyproject.toml's build-system backend is still
# uv_build, but that's fetched by pip's own build isolation on demand, not a
# system-wide `uv` binary — unaffected by this.
install-python: generate-gui-wrapper sync-version
	install -dm755 $(DESTDIR)$(dir $(VENVDIR))
	python3 -m venv --clear $(DESTDIR)$(VENVDIR)
	# activate/activate.{csh,fish,nu,bat} bake in an absolute VIRTUAL_ENV path
	# at creation time (here, the DESTDIR buildroot) and are never sourced —
	# lam-gui invokes $(VENVDIR)/bin/python3 directly. Left in place, rpmbuild's
	# check-buildroot fails the package: the buildroot path leaks into an
	# installed file. pyvenv.cfg's `command =` line (Python 3.11+, records
	# the exact `python -m venv <path>` invocation) leaks the same buildroot
	# path and is just as cosmetic — nothing reads it back at runtime.
	rm -f $(DESTDIR)$(VENVDIR)/bin/activate*
	sed -i '/^command = /d' $(DESTDIR)$(VENVDIR)/pyvenv.cfg
	$(DESTDIR)$(VENVDIR)/bin/pip install --disable-pip-version-check -q --upgrade pip
	$(DESTDIR)$(VENVDIR)/bin/pip install --disable-pip-version-check -q .
ifdef DESTDIR
	# Whole-file, not just the shebang line: a long enough buildroot path
	# pushes pip's generated console-script shebang past the kernel's ~127
	# byte limit, and pip falls back to a `#!/bin/sh` + `'''exec' <path> ...`
	# wrapper with the interpreter path on line 2, not line 1.
	find $(DESTDIR)$(VENVDIR)/bin -maxdepth 1 -type f \
		-exec sed -i "s|$(DESTDIR)||g" {} \;
endif
	install -Dm755 $(GUI_WRAPPER_OUT) $(DESTDIR)$(BINDIR)/lam-gui

# ── Install ────────────────────────────────────────────────────────────────────
install: build generate-services install-python
	# Daemon binary
	install -Dm755 $(DAEMON_BIN) $(DESTDIR)$(BINDIR)/lam-daemon
	# Privileged helper
	install -Dm755 $(HELPER_BIN) $(DESTDIR)$(LIBEXECDIR)/lam-hidraw-helper
	# Device config YAML files
	install -dm755 $(DESTDIR)$(DEVICE_CONFIGS_DIR)
	install -Dm644 $(DEVICE_YAMLS) -t $(DESTDIR)$(DEVICE_CONFIGS_DIR)/
	# Translation files (standalone copy, see LANG_DIR comment above)
	install -dm755 $(DESTDIR)$(LANG_DIR)
	install -Dm644 $(LANG_FILES) -t $(DESTDIR)$(LANG_DIR)/
	# KDE Plasma widget — cp -a, not `install -Dm644 -t`, because it's a
	# nested tree (contents/ui, contents/config, contents/code) rather than
	# a flat file list.
	install -dm755 $(DESTDIR)$(PLASMOID_DEST_DIR)
	cp -a $(PLASMOID_SRC_DIR)/. $(DESTDIR)$(PLASMOID_DEST_DIR)/
	# Systemd user service units
	install -Dm644 $(SERVICE_HELPER_OUT) $(DESTDIR)$(SYSTEMD_USER_DIR)/lam-hidraw-helper.service
	install -Dm644 $(SERVICE_DAEMON_OUT)  $(DESTDIR)$(SYSTEMD_USER_DIR)/lam-daemon.service
	# Desktop entries
	install -dm755 $(DESTDIR)$(DESKTOP_DIR)
	install -Dm644 $(DESKTOP_FILES) -t $(DESTDIR)$(DESKTOP_DIR)/
ifndef DESTDIR
	# Apply DAC capability to the installed helper binary.
	# Must run after the final copy; packaging tools handle this in post-install hooks.
	chown root:root $(LIBEXECDIR)/lam-hidraw-helper
	setcap cap_dac_override+eip $(LIBEXECDIR)/lam-hidraw-helper
	@echo ""
	@echo "Installation complete.  To activate:"
	@echo "  make enable"
endif

# ── Uninstall ──────────────────────────────────────────────────────────────────
uninstall:
	-systemctl --user stop  lam-daemon.service lam-hidraw-helper.service 2>/dev/null
	-systemctl --user disable lam-daemon.service lam-hidraw-helper.service 2>/dev/null
	rm -f $(DESTDIR)$(BINDIR)/lam-daemon
	rm -f $(DESTDIR)$(BINDIR)/lam-gui
	rm -f $(DESTDIR)$(LIBEXECDIR)/lam-hidraw-helper
	rm -f $(DESTDIR)$(SYSTEMD_USER_DIR)/lam-hidraw-helper.service
	rm -f $(DESTDIR)$(SYSTEMD_USER_DIR)/lam-daemon.service
	rm -f $(addprefix $(DESTDIR)$(DESKTOP_DIR)/,$(notdir $(DESKTOP_FILES)))
	rm -rf $(DESTDIR)$(DEVICE_CONFIGS_DIR)
	rm -rf $(DESTDIR)$(LANG_DIR)
	rm -rf $(DESTDIR)$(PLASMOID_DEST_DIR)
	rm -rf $(DESTDIR)$(LIBDIR)/linux-arctis-manager
	-systemctl --user daemon-reload 2>/dev/null

# ── Service management (non-root, for direct installs) ─────────────────────────
enable:
	systemctl --user daemon-reload
	systemctl --user enable --now lam-hidraw-helper.service lam-daemon.service

disable:
	systemctl --user stop    lam-daemon.service lam-hidraw-helper.service
	systemctl --user disable lam-daemon.service lam-hidraw-helper.service

# ── Container build (Docker / Podman) ─────────────────────────────────────────
#
# Base images contain the updated OS + build deps and are cached locally as
# named images.  The sentinel files let Make track whether the base is built
# without re-running docker build on every invocation.
#
# Flow for each distro:
#   1. sentinel target  → builds (or reuses) the named base image
#   2. container-build-* → builds a fresh image FROM the base, COPYs source,
#      produces the package, then runs it to copy artifacts into dist/
#
# To force a full OS re-update (e.g. after a month):
#   make container-refresh-fedora   (or -debian / -arch / -all)

$(DIST_DIR):
	mkdir -p $(DIST_DIR)

# ── Base image sentinels ───────────────────────────────────────────────────────
$(_SENTINEL_FEDORA): $(CONTAINER_DIR)/fedora.base.dockerfile
	$(CONTAINER_ENGINE) build \
		--build-arg FEDORA_VERSION=$(FEDORA_VERSION) \
		-f $< -t lam-builder-base-fedora $(CONTAINER_DIR)
	@touch $@

$(_SENTINEL_DEBIAN): $(CONTAINER_DIR)/debian.base.dockerfile
	$(CONTAINER_ENGINE) build \
		--build-arg DEBIAN_VERSION=$(DEBIAN_VERSION) \
		-f $< -t lam-builder-base-debian $(CONTAINER_DIR)
	@touch $@

$(_SENTINEL_ARCH): $(CONTAINER_DIR)/arch.base.dockerfile
	$(CONTAINER_ENGINE) build \
		-f $< -t lam-builder-base-arch $(CONTAINER_DIR)
	@touch $@

# ── Force-refresh targets ──────────────────────────────────────────────────────
container-refresh-fedora:
	rm -f $(_SENTINEL_FEDORA)
	$(MAKE) $(_SENTINEL_FEDORA)

container-refresh-debian:
	rm -f $(_SENTINEL_DEBIAN)
	$(MAKE) $(_SENTINEL_DEBIAN)

container-refresh-arch:
	rm -f $(_SENTINEL_ARCH)
	$(MAKE) $(_SENTINEL_ARCH)

container-refresh-all: container-refresh-fedora container-refresh-debian container-refresh-arch

# ── Package build targets ──────────────────────────────────────────────────────
container-build-rpm: $(_SENTINEL_FEDORA) | $(DIST_DIR)
	$(CONTAINER_ENGINE) build \
		-f $(CONTAINER_DIR)/fedora.build.dockerfile \
		-t lam-builder-fedora .
	$(CONTAINER_ENGINE) run --rm \
		-v "$(CURDIR)/$(DIST_DIR):/out:z" \
		lam-builder-fedora

container-build-deb: $(_SENTINEL_DEBIAN) | $(DIST_DIR)
	$(CONTAINER_ENGINE) build \
		-f $(CONTAINER_DIR)/debian.build.dockerfile \
		-t lam-builder-debian .
	$(CONTAINER_ENGINE) run --rm \
		-v "$(CURDIR)/$(DIST_DIR):/out:z" \
		lam-builder-debian

container-build-pkg: $(_SENTINEL_ARCH) | $(DIST_DIR)
	$(CONTAINER_ENGINE) build \
		-f $(CONTAINER_DIR)/arch.build.dockerfile \
		-t lam-builder-arch .
	$(CONTAINER_ENGINE) run --rm \
		-v "$(CURDIR)/$(DIST_DIR):/out:z" \
		lam-builder-arch

container-build-all: container-build-rpm container-build-deb container-build-pkg
