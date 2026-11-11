#!/bin/bash

# test_all.sh - Run all tests for transientdb (native and WASM), and the examples
#
# Usage:
#   ./test_all.sh          # Run all tests
#   ./test_all.sh native   # Run only native tests and examples
#   ./test_all.sh wasm     # Run only WASM tests and the web example build
#   ./test_all.sh --help   # Show help

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

print_header() {
    echo ""
    echo -e "${YELLOW}════════════════════════════════════════════════════════════${NC}"
    echo -e "${YELLOW}  $1${NC}"
    echo -e "${YELLOW}════════════════════════════════════════════════════════════${NC}"
    echo ""
}

print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

print_error() {
    echo -e "${RED}✗ $1${NC}"
}

show_help() {
    echo "transientdb test runner"
    echo ""
    echo "Usage: ./test_all.sh [target]"
    echo ""
    echo "Targets:"
    echo "  (none)    Run all tests (native + WASM)"
    echo "  native    Run only native tests, then run the native examples"
    echo "  wasm      Run only WASM tests, then build the web example for wasm32"
    echo "  --help    Show this help message"
    echo ""
    echo "Requirements:"
    echo "  - Rust toolchain (cargo)"
    echo "  - The wasm32-unknown-unknown target: rustup target add wasm32-unknown-unknown"
    echo "  - wasm-pack (for WASM tests): cargo install wasm-pack"
    echo "  - Chrome or Firefox (for headless WASM tests)"
    echo ""
    echo "Environment variables:"
    echo "  WASM_BROWSER   Browser for WASM tests (chrome|firefox), default: chrome"
}

run_native_tests() {
    print_header "Running Native Tests"
    
    echo "Running cargo test..."
    if ! cargo test; then
        print_error "Native tests failed"
        return 1
    fi
    print_success "Native tests passed"

    for example in memory_example file_example; do
        echo "Running example $example..."
        if ! cargo run --example "$example"; then
            print_error "Example $example failed"
            return 1
        fi
    done
    print_success "Native examples ran"
    return 0
}

run_wasm_tests() {
    print_header "Running WASM Tests"
    
    # Check for wasm-pack
    if ! command -v wasm-pack &> /dev/null; then
        print_error "wasm-pack not found. Install with: cargo install wasm-pack"
        return 1
    fi
    
    # Use environment variable or default to chrome
    BROWSER="${WASM_BROWSER:-chrome}"
    
    echo "Running wasm-pack test (browser: $BROWSER)..."
    # The test runner gives each test binary 20 seconds by default; the browser suite
    # can take longer on a slow machine.
    if ! WASM_BINDGEN_TEST_TIMEOUT="${WASM_BINDGEN_TEST_TIMEOUT:-300}" wasm-pack test --headless --"$BROWSER" --features web; then
        print_error "WASM tests failed"
        return 1
    fi
    print_success "WASM tests passed"

    # examples/web is its own crate, with its own lockfile and target directory.
    echo "Building the web example for wasm32..."
    if ! (cd "$(dirname "$0")/examples/web" && cargo build --locked --target wasm32-unknown-unknown); then
        print_error "Web example build failed"
        return 1
    fi
    print_success "Web example built"
    return 0
}

# Main
case "${1:-all}" in
    --help|-h)
        show_help
        exit 0
        ;;
    native)
        run_native_tests
        ;;
    wasm)
        run_wasm_tests
        ;;
    all|"")
        NATIVE_RESULT=0
        WASM_RESULT=0
        
        run_native_tests || NATIVE_RESULT=1
        run_wasm_tests || WASM_RESULT=1
        
        print_header "Summary"
        
        if [ $NATIVE_RESULT -eq 0 ]; then
            print_success "Native tests: PASSED"
        else
            print_error "Native tests: FAILED"
        fi
        
        if [ $WASM_RESULT -eq 0 ]; then
            print_success "WASM tests: PASSED"
        else
            print_error "WASM tests: FAILED"
        fi
        
        echo ""
        
        if [ $NATIVE_RESULT -eq 0 ] && [ $WASM_RESULT -eq 0 ]; then
            print_success "All tests passed! 🍺"
            exit 0
        else
            print_error "Some tests failed"
            exit 1
        fi
        ;;
    *)
        echo "Unknown target: $1"
        echo "Run './test_all.sh --help' for usage"
        exit 1
        ;;
esac
