.PHONY: install-service uninstall-service service-status service-restart service-logs

LAUNCH_AGENT_DIR := $(HOME)/Library/LaunchAgents
SERVICE_LABEL    := com.cli-router.proxy
SERVICE_PLIST    := $(LAUNCH_AGENT_DIR)/$(SERVICE_LABEL).plist
SERVICE_BIN      := $(HOME)/.cargo/bin/proxy
SERVICE_LOG_DIR  := $(HOME)/Library/Logs
SERVICE_LOG_OUT  := $(SERVICE_LOG_DIR)/cli-router-proxy.log
SERVICE_LOG_ERR  := $(SERVICE_LOG_DIR)/cli-router-proxy.err

# Synchronous launchctl targets. `bootout` blocks until the process exits and
# its sockets are released, so a follow-up `bootstrap` can't race on AddrInUse
# the way `load` after `unload` could.
SERVICE_DOMAIN   := gui/$(shell id -u)
SERVICE_TARGET   := $(SERVICE_DOMAIN)/$(SERVICE_LABEL)

## Service (macOS launchd) ────────────────────────────

install-service: _install-proxy-bin ## Build proxy, install it, and run it as a LaunchAgent (one-shot deploy)
	@mkdir -p $(LAUNCH_AGENT_DIR) $(SERVICE_LOG_DIR)
	@printf '%s\n' \
		'<?xml version="1.0" encoding="UTF-8"?>' \
		'<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
		'<plist version="1.0">' \
		'<dict>' \
		'  <key>Label</key><string>$(SERVICE_LABEL)</string>' \
		'  <key>ProgramArguments</key>' \
		'  <array><string>$(SERVICE_BIN)</string></array>' \
		'  <key>RunAtLoad</key><true/>' \
		'  <key>KeepAlive</key><true/>' \
		'  <key>StandardOutPath</key><string>$(SERVICE_LOG_OUT)</string>' \
		'  <key>StandardErrorPath</key><string>$(SERVICE_LOG_ERR)</string>' \
		'</dict>' \
		'</plist>' \
		> $(SERVICE_PLIST)
	@if launchctl print $(SERVICE_TARGET) >/dev/null 2>&1; then \
		echo "service already loaded — kickstarting (atomic respawn, no AddrInUse race)"; \
		launchctl kickstart -k $(SERVICE_TARGET); \
	else \
		echo "first-time install — bootstrapping"; \
		launchctl bootstrap $(SERVICE_DOMAIN) $(SERVICE_PLIST); \
	fi
	@: > $(SERVICE_LOG_ERR)
	@echo "service installed: $(SERVICE_PLIST)"
	@echo "logs: $(SERVICE_LOG_OUT)"
	@echo "      $(SERVICE_LOG_ERR)"

uninstall-service: ## Stop and remove the proxy LaunchAgent
	@if [ -f "$(SERVICE_PLIST)" ]; then \
		launchctl bootout $(SERVICE_TARGET) 2>/dev/null || true; \
		rm -f $(SERVICE_PLIST); \
		echo "service uninstalled"; \
	else \
		echo "service not installed"; \
	fi

service-restart: ## Restart the proxy service (atomic kickstart — picks up new binary at the same path)
	@if [ ! -f "$(SERVICE_PLIST)" ]; then \
		echo "error: service not installed. Run 'make install-service' first."; \
		exit 1; \
	fi
	@if launchctl print $(SERVICE_TARGET) >/dev/null 2>&1; then \
		launchctl kickstart -k $(SERVICE_TARGET); \
	else \
		launchctl bootstrap $(SERVICE_DOMAIN) $(SERVICE_PLIST); \
	fi
	@: > $(SERVICE_LOG_ERR)
	@echo "service restarted"

service-status: ## Show launchd status for the proxy service
	@launchctl list | awk 'NR==1 || /$(SERVICE_LABEL)/' | \
		(read header; echo "$$header"; \
		 lines=$$(cat); \
		 if [ -z "$$lines" ]; then echo "  (not loaded)"; else echo "$$lines"; fi)

service-logs: ## Tail proxy service stdout + stderr logs
	@touch $(SERVICE_LOG_OUT) $(SERVICE_LOG_ERR)
	@tail -f $(SERVICE_LOG_OUT) $(SERVICE_LOG_ERR)
