# ── Directory variables (override with make PREFIX=/usr install) ───────────────
PREFIX     ?= /usr/local
BINDIR     ?= $(PREFIX)/bin
LIBEXECDIR ?= $(PREFIX)/libexec
DATADIR    ?= $(PREFIX)/share
SYSTEMD_USER_DIR ?= $(PREFIX)/lib/systemd/user

# Derived paths
DEVICE_CONFIGS_DIR := $(DATADIR)/linux-arctis-manager/devices
LAM_DATADIR        := $(DATADIR)/linux-arctis-manager

# ── Build inputs / outputs ─────────────────────────────────────────────────────
CARGO      ?= cargo
MANIFEST   := daemon/Cargo.toml
HELPER_BIN := daemon/target/release/lam-hidraw-helper
DAEMON_BIN := daemon/target/release/lam-daemon

SERVICE_HELPER_IN  := packaging/systemd/user/lam-hidraw-helper.service.in
SERVICE_DAEMON_IN  := packaging/systemd/user/lam-daemon.service.in
SERVICE_HELPER_OUT := packaging/systemd/user/lam-hidraw-helper.service
SERVICE_DAEMON_OUT := packaging/systemd/user/lam-daemon.service

DEVICE_YAMLS := $(wildcard daemon/device-configs/*.yaml)

.PHONY: build generate-services install install-helper uninstall enable disable help

# ── Default target ─────────────────────────────────────────────────────────────
help:
	@echo "Targets:"
	@echo "  build              Build release binaries (pass PREFIX= to bake the data dir)"
	@echo "  install            Build + install everything (requires sudo for setcap)"
	@echo "  uninstall          Remove installed files"
	@echo "  enable             Enable and start user services (no sudo needed)"
	@echo "  disable            Stop and disable user services"
	@echo ""
	@echo "Variables (defaults shown):"
	@echo "  PREFIX=$(PREFIX)"
	@echo "  BINDIR=$(BINDIR)"
	@echo "  LIBEXECDIR=$(LIBEXECDIR)"
	@echo "  DATADIR=$(DATADIR)"
	@echo "  SYSTEMD_USER_DIR=$(SYSTEMD_USER_DIR)"
	@echo "  DESTDIR (empty by default; used by packaging tools for staged installs)"

# ── Build ──────────────────────────────────────────────────────────────────────
build:
	LAM_DATADIR=$(LAM_DATADIR) $(CARGO) build --release --manifest-path $(MANIFEST)

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

# ── Install ────────────────────────────────────────────────────────────────────
install: build generate-services
	# Daemon binary
	install -Dm755 $(DAEMON_BIN) $(DESTDIR)$(BINDIR)/lam-daemon
	# Privileged helper
	install -Dm755 $(HELPER_BIN) $(DESTDIR)$(LIBEXECDIR)/lam-hidraw-helper
	# Device config YAML files
	install -dm755 $(DESTDIR)$(DEVICE_CONFIGS_DIR)
	install -Dm644 $(DEVICE_YAMLS) -t $(DESTDIR)$(DEVICE_CONFIGS_DIR)/
	# Systemd user service units
	install -Dm644 $(SERVICE_HELPER_OUT) $(DESTDIR)$(SYSTEMD_USER_DIR)/lam-hidraw-helper.service
	install -Dm644 $(SERVICE_DAEMON_OUT)  $(DESTDIR)$(SYSTEMD_USER_DIR)/lam-daemon.service
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
	rm -f $(DESTDIR)$(LIBEXECDIR)/lam-hidraw-helper
	rm -f $(DESTDIR)$(SYSTEMD_USER_DIR)/lam-hidraw-helper.service
	rm -f $(DESTDIR)$(SYSTEMD_USER_DIR)/lam-daemon.service
	rm -rf $(DESTDIR)$(DEVICE_CONFIGS_DIR)
	-systemctl --user daemon-reload 2>/dev/null

# ── Service management (non-root, for direct installs) ─────────────────────────
enable:
	systemctl --user daemon-reload
	systemctl --user enable --now lam-hidraw-helper.service lam-daemon.service

disable:
	systemctl --user stop    lam-daemon.service lam-hidraw-helper.service
	systemctl --user disable lam-daemon.service lam-hidraw-helper.service
