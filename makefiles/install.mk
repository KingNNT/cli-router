.PHONY: install install-proxy install-proxy-tui uninstall uninstall-proxy uninstall-proxy-tui _install-proxy-bin

CARGO          ?= cargo
CARGO_INSTALL  ?= $(CARGO) install --locked --force

PROXY_CRATE     := crates/proxy
PROXY_TUI_CRATE := crates/proxy-tui

## Install ────────────────────────────────────────────

install: install-proxy install-proxy-tui ## Install proxy and proxy-tui globally

install-proxy: _install-proxy-bin ## Install the proxy binary into ~/.cargo/bin (auto-restarts service if installed)
	@if [ -f "$(SERVICE_PLIST)" ]; then \
		echo "service plist detected — restarting to pick up new binary"; \
		$(MAKE) --no-print-directory service-restart; \
	fi

# Internal: just builds + installs the binary. Used by both install-proxy
# (which adds the auto-restart hook) and install-service (which does its
# own load and shouldn't trigger a redundant restart).
_install-proxy-bin:
	$(CARGO_INSTALL) --path $(PROXY_CRATE)

install-proxy-tui: ## Install the proxy-tui binary into ~/.cargo/bin
	$(CARGO_INSTALL) --path $(PROXY_TUI_CRATE)

uninstall: uninstall-proxy uninstall-proxy-tui ## Uninstall proxy and proxy-tui

uninstall-proxy: ## Uninstall the proxy binary
	$(CARGO) uninstall proxy

uninstall-proxy-tui: ## Uninstall the proxy-tui binary
	$(CARGO) uninstall proxy-tui
