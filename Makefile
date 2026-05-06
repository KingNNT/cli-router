.DEFAULT_GOAL := help

include makefiles/install.mk

## General ────────────────────────────────────────────

help: ## Show this help
	@echo ""
	@echo "\033[1;35m  ╔══════════════════════════════════════════════╗\033[0m"
	@echo "\033[1;35m  ║\033[0m              \033[1;37m🛰  CLI Router\033[0m                 \033[1;35m║\033[0m"
	@echo "\033[1;35m  ╚══════════════════════════════════════════════╝\033[0m"
	@echo ""
	@awk ' \
		/^## / { \
			section = $$0; \
			sub(/^## /, "", section); \
			printf "\n  \033[1;33m━━ %s\033[0m\n\n", section; \
			next; \
		} \
		/^[a-zA-Z0-9_-]+:.*?## / { \
			target = $$0; \
			sub(/:.*/, "", target); \
			desc = $$0; \
			sub(/^[^#]*## /, "", desc); \
			printf "    \033[36m%-20s\033[0m \033[2m│\033[0m %s\n", target, desc; \
		} \
	' $(MAKEFILE_LIST)
	@echo ""
