#!/bin/bash

# if anything fails, we don't release.
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

print_step() {
    echo ""
    echo -e "${YELLOW}▶ $1${NC}"
}

print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

# =============================================================================
# Formatting
# =============================================================================

print_step "Running cargo fmt..."
cargo fmt
print_success "Formatting complete"

# =============================================================================
# Native Checks
# =============================================================================

print_step "Running native clippy..."
cargo clippy -- -D warnings
print_success "Native clippy passed"

print_step "Running native tests..."
cargo test
print_success "Native tests passed"

# =============================================================================
# WASM Checks
# =============================================================================

# Check for wasm target
if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
    print_step "Installing wasm32-unknown-unknown target..."
    rustup target add wasm32-unknown-unknown
fi

print_step "Running WASM clippy..."
cargo clippy --target wasm32-unknown-unknown --features web -- -D warnings
print_success "WASM clippy passed"

# Check for wasm-pack
if command -v wasm-pack &> /dev/null; then
    print_step "Running WASM tests..."
    wasm-pack test --headless --chrome --features web
    print_success "WASM tests passed"
else
    echo -e "${YELLOW}⚠ wasm-pack not found, skipping WASM tests${NC}"
    echo "  Install with: cargo install wasm-pack"
fi

# =============================================================================
# Examples
# =============================================================================

print_step "Running examples..."
for example in examples/*.rs; do
    if [ -f "$example" ]; then
        example_name=$(basename "$example" .rs)
        echo "  Running example: $example_name"
        cargo run --example "$example_name" &> /dev/null
    fi
done
print_success "Examples passed"

# =============================================================================
# Git Checks
# =============================================================================

# Check for and commit any formatting changes
if ! git diff --quiet; then
    print_step "Committing formatting changes..."
    git add .
    git commit -m "Updated formatting & clippy results"
    print_success "Changes committed"
fi

# Check if gh is installed
if ! command -v gh &> /dev/null; then
    echo -e "${RED}Error: GitHub CLI (gh) is not installed. Please install it first:${NC}"
    echo "  brew install gh"
    exit 1
fi

# Check if we're on main or master branch
CURRENT_BRANCH=$(git branch --show-current)
if [ "$CURRENT_BRANCH" != "main" ] && [ "$CURRENT_BRANCH" != "master" ]; then
    echo -e "${RED}Error: Must be on 'main' or 'master' branch to release. Currently on '$CURRENT_BRANCH'${NC}"
    exit 1
fi

# =============================================================================
# Release
# =============================================================================

print_step "Running release process..."
cargo run --bin release

echo ""
print_success "Release complete! 🍺"
