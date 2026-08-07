.DEFAULT_GOAL := help
SHELL := /bin/bash
COMPOSE := docker compose
DEV := $(COMPOSE) -f docker-compose.yml -f docker-compose.dev.yml

.PHONY: help
help: ## Show this help
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}'

# ---------------------------------------------------------------- rust ----
.PHONY: fmt lint test check
fmt: ## Format Rust code
	cd bot && cargo fmt

check: ## Type-check the bot
	cd bot && cargo check --all-targets

lint: ## Clippy with warnings denied
	cd bot && cargo fmt --check && cargo clippy --all-targets -- -D warnings

test: ## Run Rust tests
	cd bot && cargo test

# -------------------------------------------------------------- python ----
.PHONY: detector-venv detector-test
detector-venv: ## Create detector/.venv with the test dependencies
	python3 -m venv detector/.venv
	detector/.venv/bin/pip install -q -r detector/requirements-dev.txt

detector-test: ## Run detector unit tests in the isolated venv
	@test -x detector/.venv/bin/python || $(MAKE) detector-venv
	cd detector && .venv/bin/python -m pytest

# -------------------------------------------------------------- docker ----
.PHONY: build up down logs ps restart
build: ## Build all images
	$(COMPOSE) build

up: ## Start the whole stack
	$(COMPOSE) up -d

down: ## Stop the stack (keeps the database volume)
	$(COMPOSE) down

nuke: ## Stop the stack AND delete the database volume
	$(COMPOSE) down -v

logs: ## Tail logs of all services
	$(COMPOSE) logs -f --tail=100

ps: ## Show service status
	$(COMPOSE) ps

restart: ## Rebuild and restart the bot only
	$(COMPOSE) up -d --build bot

# ----------------------------------------------------------------- dev ----
.PHONY: dev-infra
dev-infra: ## Start only postgres + detector (for `cargo run` on the host)
	$(DEV) up -d postgres detector

# --------------------------------------------------------------- assets ----
.PHONY: gen-images eval
gen-images: ## Generate the committed SFW sample images
	python3 assets/generate_sfw_samples.py

eval: ## Score every image in assets/test_images against the running detector
	./scripts/eval_images.sh
