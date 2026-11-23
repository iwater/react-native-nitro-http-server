#!/bin/bash
set -e

# Change to the directory of the script
cd "$(dirname "$0")"

echo "========================================"
echo "   Running All Tests (Unit & Integration)"
echo "========================================"

# Run all tests
cargo test -- --test-threads=1
TEST_EXIT_CODE=$?

echo ""
echo "========================================"
echo "   Generating HTML Coverage Report"
echo "========================================"

# Check if cargo-tarpaulin is installed
if ! command -v cargo-tarpaulin &> /dev/null; then
    echo "cargo-tarpaulin could not be found."
    echo "Please install it using: cargo install cargo-tarpaulin"
    exit 1
fi

# Clean previous coverage results
rm -rf coverage

# Run tarpaulin
# --out Html: Generate HTML report
# --output-dir coverage: Save to 'coverage' directory
# --timeout 120: Increase timeout for slower machines/tests
cargo tarpaulin --out Html --output-dir coverage --timeout 120

echo ""
echo "========================================"
echo "   Done!"
echo "========================================"
echo "Coverage report available at: $(pwd)/coverage/tarpaulin-report.html"

# Auto-open on macOS
if [[ "$OSTYPE" == "darwin"* ]]; then
    open coverage/tarpaulin-report.html
fi
