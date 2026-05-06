.PHONY: install install-proxy install-proxy-tui uninstall uninstall-proxy uninstall-proxy-tui

CARGO          ?= cargo
CARGO_INSTALL  ?= $(CARGO) install --locked --force

PROXY_CRATE     := crates/proxy
PROXY_TUI_CRATE := crates/proxy-tui

## Install ────────────────────────────────────────────

install: install-proxy install-proxy-tui ## Install proxy and proxy-tui globally

install-proxy: ## Install the proxy binary into ~/.cargo/bin
	$(CARGO_INSTALL) --path $(PROXY_CRATE)

install-proxy-tui: ## Install the proxy-tui binary into ~/.cargo/bin
	$(CARGO_INSTALL) --path $(PROXY_TUI_CRATE)

uninstall: uninstall-proxy uninstall-proxy-tui ## Uninstall proxy and proxy-tui

uninstall-proxy: ## Uninstall the proxy binary
	$(CARGO) uninstall proxy

uninstall-proxy-tui: ## Uninstall the proxy-tui binary
	$(CARGO) uninstall proxy-tui
