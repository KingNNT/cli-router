.PHONY: install install-proxy install-proxy-tui install-analysis uninstall uninstall-proxy uninstall-proxy-tui uninstall-analysis _install-proxy-bin prod

CARGO          ?= cargo
CARGO_INSTALL  ?= $(CARGO) install --locked --force

PROXY_CRATE     := crates/proxy
PROXY_TUI_CRATE := crates/proxy-tui
ANALYSIS_CRATE  := crates/analysis

## Production ─────────────────────────────────────────

prod: install service-install ## Ship to production: build all binaries, install globally, register and start service

## Install ────────────────────────────────────────────

install: install-proxy install-proxy-tui install-analysis ## Install proxy, proxy-tui, and analysis globally

install-proxy: _install-proxy-bin ## Install the proxy binary into ~/.cargo/bin (auto-restarts service if installed)
	@if [ -f "$(SERVICE_PLIST)" ]; then \
		echo "service plist detected — restarting to pick up new binary"; \
		$(MAKE) --no-print-directory service-restart; \
	fi

# Internal: just builds + installs the binary. Used by both install-proxy
# (which adds the auto-restart hook) and service-install (which does its
# own load and shouldn't trigger a redundant restart).
_install-proxy-bin:
	$(CARGO_INSTALL) --path $(PROXY_CRATE)

install-proxy-tui: ## Install the proxy-tui binary into ~/.cargo/bin
	$(CARGO_INSTALL) --path $(PROXY_TUI_CRATE)

install-analysis: ## Install the analysis binary into ~/.cargo/bin
	$(CARGO_INSTALL) --path $(ANALYSIS_CRATE)

uninstall: uninstall-proxy uninstall-proxy-tui uninstall-analysis ## Uninstall proxy, proxy-tui, and analysis

uninstall-proxy: ## Uninstall the proxy binary
	$(CARGO) uninstall proxy

uninstall-proxy-tui: ## Uninstall the proxy-tui binary
	$(CARGO) uninstall proxy-tui

uninstall-analysis: ## Uninstall the cli-router-analysis binary
	-$(CARGO) uninstall analysis
