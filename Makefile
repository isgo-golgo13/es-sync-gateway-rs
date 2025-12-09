# ═══════════════════════════════════════════════════════════════════════════════
# ES-SYNC-GATEWAY-RS
# Production Makefile
# ═══════════════════════════════════════════════════════════════════════════════

SHELL := /bin/bash
.DEFAULT_GOAL := help

# ───────────────────────────────────────────────────────────────────────────────
# Configuration
# ───────────────────────────────────────────────────────────────────────────────

PROJECT_NAME := es-sync-gateway-rs
VERSION := $(shell grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
GIT_SHA := $(shell git rev-parse --short HEAD 2>/dev/null || echo "unknown")
BUILD_TIME := $(shell date -u '+%Y-%m-%dT%H:%M:%SZ')

# Binaries
BINARIES := es-watcher nats-gateway es-writer

# Targets
TARGET_NATIVE := $(shell rustc -vV | grep host | awk '{print $$2}')
TARGET_MUSL := x86_64-unknown-linux-musl
TARGET_ARM := aarch64-unknown-linux-musl

# Directories
BUILD_DIR := target
RELEASE_DIR := $(BUILD_DIR)/release
MUSL_DIR := $(BUILD_DIR)/$(TARGET_MUSL)/release
DIST_DIR := dist
DOCKER_DIR := deploy/docker

# Docker
DOCKER_REGISTRY ?= ghcr.io/enginevector
DOCKER_TAG ?= $(VERSION)

# Rust flags
RUSTFLAGS_RELEASE := -C target-cpu=native
RUSTFLAGS_MUSL := -C target-feature=+crt-static -C link-self-contained=yes

# ───────────────────────────────────────────────────────────────────────────────
# Colors
# ───────────────────────────────────────────────────────────────────────────────

CYAN := \033[36m
GREEN := \033[32m
YELLOW := \033[33m
RED := \033[31m
RESET := \033[0m
BOLD := \033[1m

# ───────────────────────────────────────────────────────────────────────────────
# Help
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: help
help: ## Show this help
	@echo ""
	@echo "$(BOLD)$(PROJECT_NAME)$(RESET) v$(VERSION)"
	@echo ""
	@echo "$(BOLD)Usage:$(RESET)"
	@echo "  make $(CYAN)<target>$(RESET)"
	@echo ""
	@echo "$(BOLD)Targets:$(RESET)"
	@awk 'BEGIN {FS = ":.*##"} /^[a-zA-Z_-]+:.*##/ { printf "  $(CYAN)%-20s$(RESET) %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@echo ""

# ───────────────────────────────────────────────────────────────────────────────
# Development
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: build
build: ## Build debug binaries
	@echo "$(CYAN)Building debug binaries...$(RESET)"
	cargo build --workspace

.PHONY: release
release: ## Build release binaries (native)
	@echo "$(CYAN)Building release binaries...$(RESET)"
	RUSTFLAGS="$(RUSTFLAGS_RELEASE)" cargo build --workspace --release

.PHONY: check
check: ## Run cargo check
	@echo "$(CYAN)Running cargo check...$(RESET)"
	cargo check --workspace --all-targets

.PHONY: test
test: ## Run all tests
	@echo "$(CYAN)Running tests...$(RESET)"
	cargo test --workspace

.PHONY: test-verbose
test-verbose: ## Run tests with output
	@echo "$(CYAN)Running tests (verbose)...$(RESET)"
	cargo test --workspace -- --nocapture

.PHONY: bench
bench: ## Run benchmarks
	@echo "$(CYAN)Running benchmarks...$(RESET)"
	cargo bench --workspace

# ───────────────────────────────────────────────────────────────────────────────
# Code Quality
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: fmt
fmt: ## Format code
	@echo "$(CYAN)Formatting code...$(RESET)"
	cargo fmt --all

.PHONY: fmt-check
fmt-check: ## Check code formatting
	@echo "$(CYAN)Checking code format...$(RESET)"
	cargo fmt --all -- --check

.PHONY: clippy
clippy: ## Run clippy lints
	@echo "$(CYAN)Running clippy...$(RESET)"
	cargo clippy --workspace --all-targets -- -D warnings

.PHONY: lint
lint: fmt-check clippy ## Run all lints

.PHONY: audit
audit: ## Security audit dependencies
	@echo "$(CYAN)Auditing dependencies...$(RESET)"
	cargo audit

.PHONY: outdated
outdated: ## Check for outdated dependencies
	@echo "$(CYAN)Checking outdated dependencies...$(RESET)"
	cargo outdated -R

.PHONY: deny
deny: ## Run cargo-deny checks
	@echo "$(CYAN)Running cargo-deny...$(RESET)"
	cargo deny check

# ───────────────────────────────────────────────────────────────────────────────
# Static Builds (Firecracker / Alpine)
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: musl-setup
musl-setup: ## Install musl target
	@echo "$(CYAN)Installing musl target...$(RESET)"
	rustup target add $(TARGET_MUSL)

.PHONY: musl
musl: musl-setup ## Build static musl binaries (x86_64)
	@echo "$(CYAN)Building static musl binaries...$(RESET)"
	RUSTFLAGS="$(RUSTFLAGS_MUSL)" cargo build --workspace --release --target $(TARGET_MUSL)
	@echo "$(GREEN)Binaries at: $(MUSL_DIR)/$(RESET)"
	@ls -lh $(MUSL_DIR)/es-watcher $(MUSL_DIR)/nats-gateway $(MUSL_DIR)/es-writer 2>/dev/null || true

.PHONY: musl-arm
musl-arm: ## Build static musl binaries (aarch64)
	@echo "$(CYAN)Building ARM64 musl binaries...$(RESET)"
	rustup target add $(TARGET_ARM)
	RUSTFLAGS="$(RUSTFLAGS_MUSL)" cargo build --workspace --release --target $(TARGET_ARM)

# ───────────────────────────────────────────────────────────────────────────────
# Distribution
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: dist
dist: release ## Create distribution package (native)
	@echo "$(CYAN)Creating distribution package...$(RESET)"
	@mkdir -p $(DIST_DIR)
	@for bin in $(BINARIES); do \
		cp $(RELEASE_DIR)/$$bin $(DIST_DIR)/; \
		strip $(DIST_DIR)/$$bin; \
	done
	@cp -r config $(DIST_DIR)/
	@cp README.md $(DIST_DIR)/
	@echo "$(GREEN)Distribution created at: $(DIST_DIR)/$(RESET)"
	@ls -lh $(DIST_DIR)/

.PHONY: dist-musl
dist-musl: musl ## Create distribution package (musl static)
	@echo "$(CYAN)Creating musl distribution package...$(RESET)"
	@mkdir -p $(DIST_DIR)/musl
	@for bin in $(BINARIES); do \
		cp $(MUSL_DIR)/$$bin $(DIST_DIR)/musl/; \
	done
	@cp -r config $(DIST_DIR)/musl/
	@cp README.md $(DIST_DIR)/musl/
	@echo "$(GREEN)Musl distribution created at: $(DIST_DIR)/musl/$(RESET)"
	@ls -lh $(DIST_DIR)/musl/

.PHONY: tarball
tarball: dist-musl ## Create release tarball
	@echo "$(CYAN)Creating release tarball...$(RESET)"
	tar -czvf $(DIST_DIR)/$(PROJECT_NAME)-$(VERSION)-linux-x86_64-musl.tar.gz -C $(DIST_DIR)/musl .
	@echo "$(GREEN)Tarball: $(DIST_DIR)/$(PROJECT_NAME)-$(VERSION)-linux-x86_64-musl.tar.gz$(RESET)"

# ───────────────────────────────────────────────────────────────────────────────
# Docker
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: docker-build
docker-build: ## Build all Docker images
	@echo "$(CYAN)Building Docker images...$(RESET)"
	@for bin in $(BINARIES); do \
		echo "$(CYAN)Building $$bin...$(RESET)"; \
		docker build \
			--build-arg BINARY=$$bin \
			--build-arg VERSION=$(VERSION) \
			--build-arg GIT_SHA=$(GIT_SHA) \
			--build-arg BUILD_TIME=$(BUILD_TIME) \
			-t $(DOCKER_REGISTRY)/$$bin:$(DOCKER_TAG) \
			-t $(DOCKER_REGISTRY)/$$bin:latest \
			-f $(DOCKER_DIR)/Dockerfile .; \
	done

.PHONY: docker-push
docker-push: ## Push Docker images to registry
	@echo "$(CYAN)Pushing Docker images...$(RESET)"
	@for bin in $(BINARIES); do \
		docker push $(DOCKER_REGISTRY)/$$bin:$(DOCKER_TAG); \
		docker push $(DOCKER_REGISTRY)/$$bin:latest; \
	done

.PHONY: docker-build-watcher
docker-build-watcher: ## Build es-watcher Docker image
	docker build \
		--build-arg BINARY=es-watcher \
		-t $(DOCKER_REGISTRY)/es-watcher:$(DOCKER_TAG) \
		-f $(DOCKER_DIR)/Dockerfile .

.PHONY: docker-build-gateway
docker-build-gateway: ## Build nats-gateway Docker image
	docker build \
		--build-arg BINARY=nats-gateway \
		-t $(DOCKER_REGISTRY)/nats-gateway:$(DOCKER_TAG) \
		-f $(DOCKER_DIR)/Dockerfile .

.PHONY: docker-build-writer
docker-build-writer: ## Build es-writer Docker image
	docker build \
		--build-arg BINARY=es-writer \
		-t $(DOCKER_REGISTRY)/es-writer:$(DOCKER_TAG) \
		-f $(DOCKER_DIR)/Dockerfile .

# ───────────────────────────────────────────────────────────────────────────────
# Firecracker
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: firecracker-rootfs
firecracker-rootfs: musl ## Build Firecracker rootfs
	@echo "$(CYAN)Building Firecracker rootfs...$(RESET)"
	@./firecracker/build-rootfs.sh
	@echo "$(GREEN)Rootfs created at: firecracker/rootfs.ext4$(RESET)"

.PHONY: firecracker-kernel
firecracker-kernel: ## Download Firecracker kernel
	@echo "$(CYAN)Downloading Firecracker kernel...$(RESET)"
	@mkdir -p firecracker
	@curl -fsSL -o firecracker/vmlinux \
		https://s3.amazonaws.com/spec.ccfc.min/ci-artifacts/kernels/x86_64/vmlinux-5.10.217
	@echo "$(GREEN)Kernel downloaded to: firecracker/vmlinux$(RESET)"

# ───────────────────────────────────────────────────────────────────────────────
# Local Development Stack
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: dev-up
dev-up: ## Start local dev stack (NATS + ES)
	@echo "$(CYAN)Starting development stack...$(RESET)"
	docker compose -f deploy/docker/docker-compose.dev.yaml up -d
	@echo "$(GREEN)NATS: nats://localhost:4222$(RESET)"
	@echo "$(GREEN)Elasticsearch: http://localhost:9200$(RESET)"

.PHONY: dev-down
dev-down: ## Stop local dev stack
	@echo "$(CYAN)Stopping development stack...$(RESET)"
	docker compose -f deploy/docker/docker-compose.dev.yaml down

.PHONY: dev-logs
dev-logs: ## View dev stack logs
	docker compose -f deploy/docker/docker-compose.dev.yaml logs -f

# ───────────────────────────────────────────────────────────────────────────────
# Run
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: run-watcher
run-watcher: build ## Run es-watcher (debug)
	RUST_LOG=debug cargo run -p es-watcher -- \
		--es-hosts http://localhost:9200 \
		--nats-url nats://localhost:4222 \
		--index-patterns "test-*" \
		--tenant dev

.PHONY: run-gateway
run-gateway: build ## Run nats-gateway (debug)
	RUST_LOG=debug cargo run -p nats-gateway -- \
		--upstream-url nats://localhost:4222 \
		--include-indices "test-*"

.PHONY: run-gateway-embedded
run-gateway-embedded: build ## Run nats-gateway with embedded NATS
	RUST_LOG=debug cargo run -p nats-gateway -- \
		--embedded \
		--store-dir /tmp/jetstream \
		--include-indices "test-*"

.PHONY: run-writer
run-writer: build ## Run es-writer (debug)
	RUST_LOG=debug cargo run -p es-writer -- \
		--nats-url nats://localhost:4222 \
		--es-hosts http://localhost:9200

# ───────────────────────────────────────────────────────────────────────────────
# Documentation
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: doc
doc: ## Generate documentation
	@echo "$(CYAN)Generating documentation...$(RESET)"
	cargo doc --workspace --no-deps --document-private-items

.PHONY: doc-open
doc-open: doc ## Generate and open documentation
	cargo doc --workspace --no-deps --open

# ───────────────────────────────────────────────────────────────────────────────
# Installation
# ───────────────────────────────────────────────────────────────────────────────

PREFIX ?= /usr/local

.PHONY: install
install: release ## Install binaries to PREFIX
	@echo "$(CYAN)Installing to $(PREFIX)/bin...$(RESET)"
	@install -d $(PREFIX)/bin
	@for bin in $(BINARIES); do \
		install -m 755 $(RELEASE_DIR)/$$bin $(PREFIX)/bin/; \
	done
	@echo "$(GREEN)Installed: $(BINARIES)$(RESET)"

.PHONY: uninstall
uninstall: ## Uninstall binaries from PREFIX
	@echo "$(CYAN)Uninstalling from $(PREFIX)/bin...$(RESET)"
	@for bin in $(BINARIES); do \
		rm -f $(PREFIX)/bin/$$bin; \
	done

# ───────────────────────────────────────────────────────────────────────────────
# Cleanup
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: clean
clean: ## Clean build artifacts
	@echo "$(CYAN)Cleaning build artifacts...$(RESET)"
	cargo clean
	rm -rf $(DIST_DIR)

.PHONY: clean-all
clean-all: clean ## Clean everything including caches
	@echo "$(CYAN)Cleaning all caches...$(RESET)"
	rm -rf ~/.cargo/registry/cache
	rm -rf ~/.cargo/git/checkouts

# ───────────────────────────────────────────────────────────────────────────────
# CI Targets
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: ci
ci: fmt-check clippy test ## Run CI checks

.PHONY: ci-full
ci-full: ci audit ## Run full CI checks including audit

# ───────────────────────────────────────────────────────────────────────────────
# Info
# ───────────────────────────────────────────────────────────────────────────────

.PHONY: info
info: ## Show build info
	@echo "$(BOLD)Build Information$(RESET)"
	@echo "  Project:     $(PROJECT_NAME)"
	@echo "  Version:     $(VERSION)"
	@echo "  Git SHA:     $(GIT_SHA)"
	@echo "  Build Time:  $(BUILD_TIME)"
	@echo "  Native:      $(TARGET_NATIVE)"
	@echo "  Musl:        $(TARGET_MUSL)"
	@echo ""
	@echo "$(BOLD)Rust Toolchain$(RESET)"
	@rustc --version
	@cargo --version

.PHONY: tree
tree: ## Show project structure
	@tree -I 'target|node_modules|dist' -a --dirsfirst
