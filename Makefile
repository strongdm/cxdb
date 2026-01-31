SHELL := /bin/bash

.PHONY: default
default: help

.PHONY: help
help: ## Dynamically list available make targets
	@printf "\nCommon targets:\n\n"
	@{ \
		awk 'BEGIN {FS = ":.*##"} /^[[:alnum:]_][^:]*:.*##/ {if ($$1 ~ /^_/) next; sub(/^[[:space:]]+/, "", $$2); printf "  make %-16s - %s\n", $$1, $$2}' $(MAKEFILE_LIST); \
	} | sort
	@printf '\n'

# === Build targets ===

.PHONY: build
build: ## Build Rust backend (debug)
	cargo build --release

.PHONY: release
release: ## Build Rust backend (release)
	cargo build --release

.PHONY: test
test: ## Run all tests
	cargo test --workspace

.PHONY: check
check: ## Type check without building
	cargo check --workspace

.PHONY: clippy
clippy: ## Lint with clippy
	cargo clippy --workspace -- -D warnings

.PHONY: fmt
fmt: ## Format Rust code
	cargo fmt --all

.PHONY: fmt-check
fmt-check: ## Check Rust formatting
	cargo fmt --all -- --check

.PHONY: clean
clean: ## Clean build artifacts
	cargo clean
	rm -rf frontend/node_modules frontend/.next frontend/out
	rm -rf gateway/bin gateway/data

# === Frontend targets ===

.PHONY: ui-install
ui-install: ## Install frontend dependencies
	cd frontend && npm install

.PHONY: ui-build
ui-build: ui-install ## Build frontend
	cd frontend && npm run build

.PHONY: ui-dev
ui-dev: ## Run frontend dev server
	cd frontend && npm run dev

# === Gateway targets ===

.PHONY: gateway-build
gateway-build: ## Build Go gateway
	cd gateway && go build -o bin/gateway ./cmd/server

.PHONY: gateway-dev
gateway-dev: ## Run gateway in dev mode (no OAuth)
	@mkdir -p gateway/data
	@if [ ! -f gateway/.env ]; then \
		echo "Creating gateway/.env for dev mode..."; \
		echo 'DEV_MODE=true' > gateway/.env; \
		echo 'DEV_EMAIL=dev@localhost' >> gateway/.env; \
		echo 'DEV_NAME=Developer' >> gateway/.env; \
		echo 'PUBLIC_BASE_URL=http://localhost:8080' >> gateway/.env; \
		echo 'CXDB_BACKEND_URL=http://127.0.0.1:9010' >> gateway/.env; \
		echo 'PORT=8080' >> gateway/.env; \
		echo 'DATABASE_PATH=./data/sessions.db' >> gateway/.env; \
		echo 'GOOGLE_CLIENT_ID=unused' >> gateway/.env; \
		echo 'GOOGLE_CLIENT_SECRET=unused' >> gateway/.env; \
		echo 'GOOGLE_ALLOWED_DOMAIN=localhost' >> gateway/.env; \
		echo 'SESSION_SECRET=0000000000000000000000000000000000000000000000000000000000000000' >> gateway/.env; \
	fi
	cd gateway && go run ./cmd/server

# === Development targets ===

.PHONY: dev
dev: ## Run full dev stack (backend + gateway + frontend) in tmux
	@mkdir -p .scratch/local-data
	@tmux kill-session -t cxdb 2>/dev/null || true
	@tmux new-session -d -s cxdb -n backend
	@tmux send-keys -t cxdb:backend "cd $(PWD) && CXDB_DATA_DIR=.scratch/local-data CXDB_HTTP_BIND=127.0.0.1:9010 ./target/release/cxdb-server" Enter
	@sleep 2
	@tmux new-window -t cxdb -n gateway
	@tmux send-keys -t cxdb:gateway "cd $(PWD)/gateway && make -C .. gateway-dev" Enter
	@sleep 2
	@tmux new-window -t cxdb -n frontend
	@tmux send-keys -t cxdb:frontend "cd $(PWD)/frontend && npm run dev" Enter
	@echo ""
	@echo "Dev stack started in tmux session 'cxdb'"
	@echo ""
	@echo "  Frontend:  http://localhost:3000"
	@echo "  Gateway:   http://localhost:8080  (DEV_MODE - no OAuth)"
	@echo "  Backend:   http://localhost:9010"
	@echo ""
	@echo "  tmux attach -t cxdb"
	@echo ""

.PHONY: dev-stop
dev-stop: ## Stop dev stack
	@tmux kill-session -t cxdb 2>/dev/null && echo "Stopped cxdb session" || echo "No cxdb session running"

.PHONY: sync-prod-data
sync-prod-data: ## Sync production data to .scratch/local-data/
	@echo "Syncing production data..."
	@mkdir -p .scratch/local-data
	kubectl cp cxdb/cxdb-0:/data/turns .scratch/local-data/turns
	kubectl cp cxdb/cxdb-0:/data/blobs .scratch/local-data/blobs
	kubectl cp cxdb/cxdb-0:/data/registry .scratch/local-data/registry
	@echo "Done. Data in .scratch/local-data/"

# === Precommit ===

.PHONY: precommit
precommit: fmt-check clippy test ## Run all checks before commit
