.PHONY: dev-proxy dev-proxy-tui dev-init dev-reset dev-paths dev-seed-requests

DEV_PORT         := 8788
DEV_DATA_DIR     := $(HOME)/.local/share/cli-router/dev
DEV_DB           := $(DEV_DATA_DIR)/proxy.db
PROD_DB          := $(HOME)/.local/share/cli-router/proxy.db

# Source ./.env (if present) so a recipe's cargo subprocess inherits the vars.
# `set -a` auto-exports every assignment; restored with `set +a`.
LOAD_DOTENV = if [ -f .env ]; then set -a; . ./.env; set +a; fi

## Development ────────────────────────────────────────

dev-proxy: ## Run proxy in foreground against the dev DB (port 8788). Auto-sources ./.env.
	@if [ ! -f "$(DEV_DB)" ]; then \
		echo "no dev DB at $(DEV_DB). Run 'make dev-init' first."; \
		exit 1; \
	fi
	@$(LOAD_DOTENV); \
		cargo run -p proxy -- --db $(DEV_DB)

dev-proxy-tui: ## Run proxy-tui pointed at the dev proxy (default 127.0.0.1:8788). Auto-sources ./.env.
	@$(LOAD_DOTENV); \
		: $${CLI_ROUTER_PROXY_URL:=http://127.0.0.1:$(DEV_PORT)}; export CLI_ROUTER_PROXY_URL; \
		cargo run -p proxy-tui

dev-init: ## Clone prod DB into dev with port override (proxy 8788, dev paths). Refuses to overwrite.
	@if [ -f "$(DEV_DB)" ]; then \
		echo "error: $(DEV_DB) already exists. Run 'make dev-reset' to recreate."; \
		exit 1; \
	fi
	@if [ ! -f "$(PROD_DB)" ]; then \
		echo "error: no prod DB at $(PROD_DB)."; \
		exit 1; \
	fi
	@mkdir -p $(DEV_DATA_DIR)
	@cp $(PROD_DB) $(DEV_DB)
	@sqlite3 "$(DEV_DB)" \
		"INSERT OR REPLACE INTO settings (key, value) VALUES ('port', '$(DEV_PORT)');" \
		"INSERT OR REPLACE INTO settings (key, value) VALUES ('proxy_db', '$(DEV_DB)');" \
		"INSERT OR REPLACE INTO settings (key, value) VALUES ('pricing_db', '$(DEV_DATA_DIR)/pricing.db');" \
		"DELETE FROM requests;" \
		"DELETE FROM api_keys;"
	@echo "cloned prod DB → $(DEV_DB)"
	@echo "  port:     $(DEV_PORT)"
	@echo "  data dir: $(DEV_DATA_DIR)"
	@echo ""
	@echo "providers/routing were copied from prod — edit via the admin API to change them."

dev-reset: ## Delete dev DBs, then re-clone from prod
	@rm -rf $(DEV_DATA_DIR)
	@$(MAKE) --no-print-directory dev-init

dev-paths: ## Print resolved dev paths and ports
	@echo "dev DB:       $(DEV_DB)"
	@echo "dev data dir: $(DEV_DATA_DIR)"
	@echo "dev port:     $(DEV_PORT)"
	@echo "prod DB:      $(PROD_DB)"
	@echo "prod port:    8787 (managed by launchd via make service-install)"

dev-seed-requests: ## Insert mock requests into the dev DB (default 200). Override: make dev-seed-requests SEED_COUNT=500
	@if [ ! -f "$(DEV_DB)" ]; then \
		echo "error: $(DEV_DB) not found. Run 'make dev-init' first."; \
		exit 1; \
	fi
	@count=$$(sqlite3 "$(DEV_DB)" "SELECT COUNT(*) FROM requests;" 2>/dev/null || echo 0); \
		echo "requests before seed: $$count"; \
		sqlite3 "$(DEV_DB)" < scripts/seed-dev-requests.sql; \
		count=$$(sqlite3 "$(DEV_DB)" "SELECT COUNT(*) FROM requests;"); \
		echo "requests after seed:  $$count"
