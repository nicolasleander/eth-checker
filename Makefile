# Configuration
NUM_MNEMONICS ?= 50
MODE ?= infura
THREADS ?= 0  # 0 means use all available cores

# Build targets
TARGET := x86_64-unknown-linux-musl
RELEASE_DIR := target/$(TARGET)/release

# Commands
CARGO := cargo
RUSTUP := rustup

# Rust optimization flags
RUSTFLAGS_PROD := -C target-cpu=native \
                  -C opt-level=3 \
                  -C codegen-units=1 \
                  -C lto=fat \
                  -C panic=abort \
                  -C debuginfo=0

# Commands
CARGO_PROD := RUSTFLAGS="$(RUSTFLAGS_PROD)" $(CARGO)

# Database file
DB_FILE := eth_checker.db

.PHONY: all prod dev clean help check-env install-deps build-prod build-dev run-prod run-dev benchmark profile

default: help

# Install dependencies and targets
install-deps:
	@echo "Installing required targets and dependencies..."
	@$(RUSTUP) target add $(TARGET)
	@if [ -f /etc/debian_version ]; then \
		echo "Installing musl-tools on Debian/Ubuntu..."; \
		sudo apt-get update && sudo apt-get install -y musl-tools; \
	fi

# Environment checks
check-env: install-deps
	@rustc --version >/dev/null 2>&1 || (echo "Error: Rust is not installed" && exit 1)
	@cargo --version >/dev/null 2>&1 || (echo "Error: Cargo is not installed" && exit 1)

# Production build
build-prod: check-env
	@echo "Building production optimized binary..."
	@$(CARGO_PROD) build --release --target $(TARGET)
	@echo "Stripping debug symbols..."
	@strip $(RELEASE_DIR)/eth-checker || true
	@echo "Build complete: $(RELEASE_DIR)/eth-checker"

# Development build
build-dev: check-env
	@echo "Building development binary..."
	@$(CARGO) build

# Production run
run-prod: build-prod
	@echo "Running production build with $(NUM_MNEMONICS) mnemonics using $(MODE)..."
	@./$(RELEASE_DIR)/eth-checker -n $(NUM_MNEMONICS) $(if $(filter local,$(MODE)),-l,) $(if $(filter 0,$(THREADS)),-t $$(nproc),$(if $(THREADS),-t $(THREADS),))

# Development run
run-dev: build-dev
	@echo "Running development build..."
	@cargo run -- -n $(NUM_MNEMONICS) $(if $(filter local,$(MODE)),-l,)

# Clean build artifacts and database
clean:
	@echo "Cleaning build artifacts and database..."
	@$(CARGO) clean
	@rm -f $(DB_FILE)
	@echo "Clean complete"

# Help
help:
	@echo "ETH Checker Production Makefile"
	@echo
	@echo "Setup Commands:"
	@echo "  make install-deps                   - Install required dependencies"
	@echo
	@echo "Production Commands:"
	@echo "  make run-prod                      - Run production build"
	@echo "  make build-prod                    - Build production binary"
	@echo
	@echo "Development Commands:"
	@echo "  make run-dev                       - Run development build"
	@echo "  make build-dev                     - Build development binary"
	@echo
	@echo "Maintenance Commands:"
	@echo "  make clean                         - Clean build artifacts"
	@echo
	@echo "Configuration options:"
	@echo "  NUM_MNEMONICS=1000                - Number of mnemonics to check"
	@echo "  MODE=local                        - Use local node (default: infura)"
	@echo "  THREADS=16                        - Number of threads (0 = all cores)"
	@echo
	@echo "Current settings:"
	@echo "  NUM_MNEMONICS = $(NUM_MNEMONICS)"
	@echo "  MODE          = $(MODE)"
	@echo "  THREADS       = $(THREADS)"
	@echo "  TARGET        = $(TARGET)"
	@echo
	@echo "Examples:"
	@echo "  make install-deps"
	@echo "  make run-prod NUM_MNEMONICS=1000 MODE=local THREADS=16"