# NanoKVM-RS Makefile
# Build system for the Rust-based NanoKVM server

.PHONY: help build build-docker package clean check fmt test shell

# Configuration
IMAGE_NAME := nanokvm-rs-builder
OUTPUT_DIR := output
VERSION := $(shell git describe --tags --always --dirty 2>/dev/null || echo "dev")

# Default target
all: build

help:
	@echo "NanoKVM-RS Build System"
	@echo ""
	@echo "Available targets:"
	@echo "  help         - Show this help message"
	@echo "  build        - Build using Docker (cross-compile for RISC-V)"
	@echo "  build-native - Build natively (requires RISC-V toolchain)"
	@echo "  package      - Create deployment package"
	@echo "  check        - Run cargo check"
	@echo "  fmt          - Format code with rustfmt"
	@echo "  test         - Run tests"
	@echo "  clean        - Clean build artifacts"
	@echo "  shell        - Enter Docker build environment"
	@echo ""
	@echo "Version: $(VERSION)"

# Build using Docker (recommended)
build:
	@echo "Building NanoKVM-RS $(VERSION) for RISC-V..."
	@mkdir -p $(OUTPUT_DIR)
	docker build --target packager -t $(IMAGE_NAME) .
	docker create --name extract-$(IMAGE_NAME) $(IMAGE_NAME)
	docker cp extract-$(IMAGE_NAME):/package/. $(OUTPUT_DIR)/
	docker rm extract-$(IMAGE_NAME)
	@echo ""
	@echo "Build complete! Output in $(OUTPUT_DIR)/"
	@ls -la $(OUTPUT_DIR)/

# Build natively (requires toolchain)
build-native:
	@echo "Building natively for RISC-V..."
	cargo build --release --target riscv64gc-unknown-linux-gnu

# Create deployment package
package: build
	@echo "Creating deployment package..."
	cd $(OUTPUT_DIR) && tar -czvf ../nanokvm-rs-$(VERSION).tar.gz .
	@echo ""
	@echo "Package created: nanokvm-rs-$(VERSION).tar.gz"
	@ls -la nanokvm-rs-$(VERSION).tar.gz

# Run cargo check
check:
	cargo check --workspace

# Format code
fmt:
	cargo fmt --all

# Run tests
test:
	cargo test --workspace

# Clean build artifacts
clean:
	cargo clean
	rm -rf $(OUTPUT_DIR)
	rm -f nanokvm-rs-*.tar.gz
	-docker rmi $(IMAGE_NAME) 2>/dev/null || true
	@echo "Clean complete."

# Enter Docker shell for debugging
shell:
	docker build --target builder -t $(IMAGE_NAME)-shell .
	docker run -it --rm -v $(PWD):/build $(IMAGE_NAME)-shell /bin/bash
