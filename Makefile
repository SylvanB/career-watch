.DEFAULT_GOAL := help

# ── Config ──────────────────────────────────────────────────────────────────

DB_URL  ?= postgres://career_watch:password@localhost:5432/career_watch
COMPOSE  = docker compose

# ── Help ────────────────────────────────────────────────────────────────────

.PHONY: help
help:
	@echo ""
	@echo "  career-watch local development"
	@echo ""
	@echo "  Infrastructure"
	@echo "    make up            Start Postgres + Mailpit"
	@echo "    make down          Stop all containers"
	@echo "    make logs          Tail container logs"
	@echo "    make psql          Open a psql shell"
	@echo ""
	@echo "  Database"
	@echo "    make migrate       Run pending migrations"
	@echo "    make db-reset      Drop and recreate the local database"
	@echo ""
	@echo "  Run (native)"
	@echo "    make run-api           cargo run --bin career-watch-api"
	@echo "    make run-worker        cargo run --bin career-watch-worker"
	@echo "    make run-discovery     cargo run --bin career-watch-discovery"
	@echo "    make run-test-server   fake career page on :9090"
	@echo ""
	@echo "  Testing (requires test-server + api to be running)"
	@echo "    make test-watcher  Create a watcher pointed at the test server"
	@echo "    make test-add-job  Add a job to the test server"
	@echo "    make test-rm-job   Remove 'senior-rust-engineer' from test server"
	@echo "    make test-reset    Reset test server jobs to defaults"
	@echo ""
	@echo "  Ollama (local LLM)"
	@echo "    make ollama-up     Start Ollama container"
	@echo "    make ollama-pull   Pull the configured model (qwen2.5:3b)"
	@echo ""
	@echo "  Frontend (web/)"
	@echo "    make web-install   npm install"
	@echo "    make web-dev       Astro dev server on :4321 (proxies API to :8080)"
	@echo "    make web-build     Build static output to web/dist/"
	@echo ""
	@echo "  Build"
	@echo "    make test          Run all tests"
	@echo "    make build         cargo build --release"
	@echo "    make docker-build  Build all Docker images"
	@echo ""

# ── Infrastructure ───────────────────────────────────────────────────────────

.PHONY: up
up:
	$(COMPOSE) up -d postgres mailpit
	@echo ""
	@echo "  Postgres  → localhost:5432"
	@echo "  Mailpit   → http://localhost:8025"
	@echo ""

.PHONY: down
down:
	$(COMPOSE) down

.PHONY: logs
logs:
	$(COMPOSE) logs -f

.PHONY: psql
psql:
	$(COMPOSE) exec postgres psql -U career_watch -d career_watch

# ── Database ─────────────────────────────────────────────────────────────────

# Note: migrations also run automatically on API/worker startup.
# Requires: cargo install sqlx-cli --no-default-features --features postgres
.PHONY: migrate
migrate:
	cargo sqlx migrate run --database-url "$(DB_URL)"

.PHONY: db-reset
db-reset: scripts/db-reset.sh
	@bash scripts/db-reset.sh

# ── Run (native, expects infra to be up) ─────────────────────────────────────

.PHONY: run-api
run-api:
	cargo run --bin career-watch-api

.PHONY: run-worker
run-worker:
	cargo run --bin career-watch-worker

.PHONY: run-discovery
run-discovery:
	cargo run --bin career-watch-discovery

.PHONY: run-test-server
run-test-server:
	cargo run --bin career-watch-test-server

# ── Test-server helpers ───────────────────────────────────────────────────────

TEST_SERVER ?= http://localhost:9090
API_SERVER  ?= http://localhost:8080
TEST_EMAIL  ?= test@localhost

.PHONY: test-watcher
test-watcher:
	curl -s -X POST $(API_SERVER)/api/watches \
	  -H 'Content-Type: application/json' \
	  -d '{"company_name":"AcmeCorp (test)","url":"$(TEST_SERVER)/careers","email":"$(TEST_EMAIL)","job_selector":"div.job-card h3.job-title"}' \
	  | jq .

.PHONY: test-add-job
test-add-job:
	curl -s -X POST $(TEST_SERVER)/admin/jobs \
	  -H 'Content-Type: application/json' \
	  -d '{"title":"Staff Engineer","department":"Platform","location":"Remote","job_type":"Full-time"}' \
	  && echo " → job added"

.PHONY: test-rm-job
test-rm-job:
	curl -s -X DELETE $(TEST_SERVER)/admin/jobs/senior-rust-engineer \
	  && echo " → job removed"

.PHONY: test-reset
test-reset:
	curl -s -X POST $(TEST_SERVER)/admin/reset \
	  && echo " → reset to defaults"

# ── Ollama ───────────────────────────────────────────────────────────────────

OLLAMA_MODEL            ?= qwen2.5:3b
OLLAMA_EXTRACTION_MODEL ?= GFalcon-UA/ReaderLM-v2:Q8

.PHONY: ollama-up
ollama-up:
	$(COMPOSE) --profile ai up -d ollama
	@echo "  Ollama → http://localhost:11434"

.PHONY: ollama-pull
ollama-pull:
	@echo "Pulling discovery model: $(OLLAMA_MODEL)"
	$(COMPOSE) exec ollama ollama pull $(OLLAMA_MODEL)
	@echo "Pulling extraction model: $(OLLAMA_EXTRACTION_MODEL)"
	$(COMPOSE) exec ollama ollama pull $(OLLAMA_EXTRACTION_MODEL)
	@echo "  Models ready"

# ── Frontend ─────────────────────────────────────────────────────────────────

.PHONY: web-install
web-install:
	cd web && npm install

.PHONY: web-dev
web-dev:
	cd web && npm run dev

.PHONY: web-build
web-build:
	cd web && npm run build

# ── Build & test ─────────────────────────────────────────────────────────────

.PHONY: test
test:
	cargo test

.PHONY: build
build:
	cargo build --release

.PHONY: docker-build
docker-build:
	docker build --target api       -t career-watch-api:local .
	docker build --target worker    -t career-watch-worker:local .
	docker build --target discovery -t career-watch-discovery:local .
