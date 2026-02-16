.PHONY: build run watch ci check test help

# Build the project
build:
	cargo build

# Run the project
run:
	cargo run

# Watch for changes and run
watch:
	cargo watch -x run

# Run local CI pipeline
ci:
	act push

# Run clippy checks with strict warnings
check:
	cargo clippy -- -D warnings

# Run tests
test:
	cargo test

# Show help
help:
	@echo "Available targets:"
	@echo "  build   : Run cargo build"
	@echo "  run     : Run cargo run"
	@echo "  watch   : Run cargo watch -x run"
	@echo "  ci      : Run act push"
	@echo "  check   : Run cargo clippy -- -D warnings"
	@echo "  test    : Run cargo test"
	@echo "  help    : Show this help message"
