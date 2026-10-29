# Docs site workspace Makefile
# ============================

INCAN ?= incan

.PHONY: docs  ## docs - Build and serve the documentation site locally
docs:
	@python -m mkdocs --version >/dev/null 2>&1 || $(MAKE) docs-install
	@$(MAKE) docs-serve

.PHONY: docs-install  ## docs - Install docs site dependencies (MkDocs + Material)
docs-install:
	@echo "\033[1mInstalling docs dependencies...\033[0m"
	@python -m pip install --upgrade pip
	@python -m pip install -r requirements-docs.txt
	@echo "\033[32m✓ Docs dependencies installed\033[0m"

.PHONY: docs-generate-rfcs  ## docs - Generate RFC index and reference snippets
docs-generate-rfcs:
	@$(INCAN) run utils/gen_rfc_index_snippet.incn >/dev/null

.PHONY: docs-check-components  ## docs - Validate Incapunk component and asset contracts
docs-check-components: docs-generate-rfcs
	@echo "\033[1mChecking Incapunk component contracts...\033[0m"
	@$(INCAN) run scripts/check_incapunk_components.incn
	@node scripts/check_incan_mermaid_runtime.mjs
	@echo "\033[32m✓ Incapunk component contracts passed\033[0m"

.PHONY: docs-check-learning  ## docs - Validate learning routes and canonical tutorial contracts
docs-check-learning:
	@echo "\033[1mChecking learning journey contracts...\033[0m"
	@$(INCAN) run scripts/check_learning_journey.incn
	@echo "\033[32m✓ Learning journey contracts passed\033[0m"

.PHONY: docs-check-v06-roadmap  ## docs - Validate the v0.6 roadmap renderer contract
docs-check-v06-roadmap:
	@echo "\033[1mChecking v0.6 roadmap renderer contract...\033[0m"
	@sh scripts/v0_6_roadmap/check_roadmap.sh
	@echo "\033[32m✓ v0.6 roadmap renderer contract passed\033[0m"

.PHONY: docs-check  ## docs - Run every lightweight docs contract
docs-check: docs-check-components docs-check-learning docs-check-v06-roadmap

.PHONY: docs-refresh  ## docs - Refresh every checked-in documentation snapshot
docs-refresh: docs-refresh-v06-roadmap

.PHONY: docs-refresh-v06-roadmap  ## docs - Refresh the checked-in v0.6 issue map from GitHub
docs-refresh-v06-roadmap:
	@echo "\033[1mRefreshing the v0.6 roadmap snapshot from GitHub...\033[0m"
	@sh scripts/v0_6_roadmap/refresh_github.sh
	@sh scripts/v0_6_roadmap/render_roadmap.sh
	@echo "\033[32m✓ v0.6 roadmap snapshot refreshed\033[0m"

.PHONY: docs-build  ## docs - Build docs site (MkDocs strict)
docs-build: docs-check
	@echo "\033[1mBuilding docs site...\033[0m"
	@python -m mkdocs build --strict
	@echo "\033[32m✓ Docs site built\033[0m"

.PHONY: docs-serve  ## docs - Serve docs site locally (MkDocs)
docs-serve: docs-generate-rfcs
	@echo "\033[1mServing docs site at http://127.0.0.1:8000 ...\033[0m"
	@python -m mkdocs serve -a 127.0.0.1:8000

.PHONY: docs-lint  ## docs - Lint markdown docs (markdownlint-cli2 via npx)
# Override to customize lint scope, e.g.:
#   make docs-lint DOCS_LINT_GLOBS="docs/**/*.md"
# By default we exclude the generated language reference.
DOCS_LINT_GLOBS ?= docs/**/*.md !docs/reference/language.md
docs-lint:
	@echo "\033[1mLinting docs markdown...\033[0m"
	@NPM_CONFIG_CACHE="$(PWD)/.npm-cache" npx --yes markdownlint-cli2 $(DOCS_LINT_GLOBS)
	@echo "\033[32m✓ Docs markdown lint passed\033[0m"
