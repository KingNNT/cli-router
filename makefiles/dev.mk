.PHONY: dev-proxy dev-proxy-tui dev-init dev-reset dev-paths dev-seed-requests

DEV_PORT         := 8788
CONFIG_DIR       := $(HOME)/.config/cli-router
PROD_CONFIG      := $(CONFIG_DIR)/config.toml
DEV_CONFIG       := $(CONFIG_DIR)/config.dev.toml
DEV_DATA_DIR     := $(HOME)/.local/share/cli-router/dev

# Source ./.env (if present) so a recipe's cargo subprocess inherits the vars.
# `set -a` auto-exports every assignment; restored with `set +a`.
LOAD_DOTENV = if [ -f .env ]; then set -a; . ./.env; set +a; fi

## Development ────────────────────────────────────────

dev-proxy: ## Run proxy in foreground with CLI_ROUTER_PROFILE=dev (port 8788, separate DBs). Auto-sources ./.env.
	@if [ ! -f "$(DEV_CONFIG)" ]; then \
		echo "no dev config at $(DEV_CONFIG). Run 'make dev-init' first."; \
		exit 1; \
	fi
	@mkdir -p $(DEV_DATA_DIR)
	@$(LOAD_DOTENV); \
		: $${CLI_ROUTER_PROFILE:=dev}; export CLI_ROUTER_PROFILE; \
		cargo run -p proxy

dev-proxy-tui: ## Run proxy-tui pointed at the dev proxy (default 127.0.0.1:8788). Auto-sources ./.env.
	@$(LOAD_DOTENV); \
		: $${CLI_ROUTER_PROXY_URL:=http://127.0.0.1:$(DEV_PORT)}; export CLI_ROUTER_PROXY_URL; \
		cargo run -p proxy-tui

dev-init: ## Bootstrap config.dev.toml from prod (port 8788, dev DB paths). Refuses to overwrite.
	@if [ -f "$(DEV_CONFIG)" ]; then \
		echo "error: $(DEV_CONFIG) already exists. Edit it manually or 'make dev-reset' to recreate."; \
		exit 1; \
	fi
	@if [ ! -f "$(PROD_CONFIG)" ]; then \
		echo "error: no prod config at $(PROD_CONFIG). Create it first or write a dev config by hand."; \
		exit 1; \
	fi
	@mkdir -p $(CONFIG_DIR) $(DEV_DATA_DIR)
	@sed -E \
		-e 's|^[[:space:]]*port[[:space:]]*=.*|port = $(DEV_PORT)|' \
		-e 's|/\.local/share/cli-router/proxy\.db|/.local/share/cli-router/dev/proxy.db|g' \
		-e 's|/\.local/share/cli-router/pricing\.db|/.local/share/cli-router/dev/pricing.db|g' \
		$(PROD_CONFIG) > $(DEV_CONFIG)
	@echo "wrote $(DEV_CONFIG)"
	@echo "  port: $(DEV_PORT)"
	@echo "  data: $(DEV_DATA_DIR)"
	@echo ""
	@echo "credentials were copied from prod — swap them in $(DEV_CONFIG) if you want dev to use separate keys."

dev-reset: ## Delete config.dev.toml and dev DBs, then re-init from prod
	@rm -f $(DEV_CONFIG)
	@rm -rf $(DEV_DATA_DIR)
	@$(MAKE) --no-print-directory dev-init

dev-paths: ## Print resolved dev paths and ports
	@echo "dev config:     $(DEV_CONFIG)"
	@echo "dev data dir:   $(DEV_DATA_DIR)"
	@echo "dev port:       $(DEV_PORT)"
	@echo "prod config:    $(PROD_CONFIG)"
	@echo "prod port:      8787 (managed by launchd via make service-install)"

DEV_DB := $(DEV_DATA_DIR)/proxy.db

dev-seed-requests: ## Insert mock requests into the dev DB (default 200). Override: make dev-seed-requests SEED_COUNT=500
	@if [ ! -f "$(DEV_DB)" ]; then \
		echo "error: $(DEV_DB) not found. Run 'make dev-proxy' first to create it."; \
		exit 1; \
	fi
	@count=$$(sqlite3 "$(DEV_DB)" "SELECT COUNT(*) FROM requests;" 2>/dev/null || echo 0); \
	echo "requests before seed: $$count"; \
	sqlite3 "$(DEV_DB)" < scripts/seed-dev-requests.sql; \
	count=$$(sqlite3 "$(DEV_DB)" "SELECT COUNT(*) FROM requests;"); \
	echo "requests after seed:  $$count"
