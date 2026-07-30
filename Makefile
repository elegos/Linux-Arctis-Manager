PREFIX     ?= /usr/local
LIBEXECDIR ?= $(PREFIX)/libexec
BINDIR     ?= $(PREFIX)/bin
CARGO      ?= cargo
SYSTEMD_USER_DIR ?= $(HOME)/.config/systemd/user

HELPER_BIN := daemon/target/release/lam-hidraw-helper
DAEMON_BIN := daemon/target/release/lam-daemon

.PHONY: build install-helper install uninstall help

help:
	@echo "Targets:"
	@echo "  build          Build all Rust binaries (release)"
	@echo "  install-helper Install lam-hidraw-helper with CAP_DAC_OVERRIDE (requires sudo)"
	@echo "  install        install-helper + install lam-daemon"
	@echo "  uninstall      Remove installed binaries"

build:
	$(CARGO) build --release --manifest-path daemon/Cargo.toml

$(HELPER_BIN) $(DAEMON_BIN): build

install-helper: $(HELPER_BIN)
	install -Dm755 $(HELPER_BIN) $(DESTDIR)$(LIBEXECDIR)/lam-hidraw-helper
# setcap stores the capability in the inode xattr of the installed file.
# It must be applied after the final copy and must not run when DESTDIR is set
# (packaging tools apply capabilities via post-install hooks instead).
ifndef DESTDIR
	chown root:root $(LIBEXECDIR)/lam-hidraw-helper
	setcap cap_dac_override+eip $(LIBEXECDIR)/lam-hidraw-helper
	@echo "lam-hidraw-helper installed at $(LIBEXECDIR)/lam-hidraw-helper"
	@echo "Capability: cap_dac_override+eip"
endif

install: install-helper $(DAEMON_BIN)
	install -Dm755 $(DAEMON_BIN) $(DESTDIR)$(BINDIR)/lam-daemon
ifndef DESTDIR
	@echo "Run 'systemctl --user enable --now lam-daemon' to start the service."
endif

uninstall:
	rm -f $(LIBEXECDIR)/lam-hidraw-helper
	rm -f $(BINDIR)/lam-daemon
