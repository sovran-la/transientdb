#!/bin/bash
# Local gate for the transientdb 0.4.0 work. Usage: gate.sh <checkout root>
# Runs every check of the plan's gate that can run on this machine and reports each status.
# Checks that need other hosts (MSRV 1.80.0, Apple simulators, Android, Firefox) run in CI.
export PATH=/Users/bsneed/.cargo/bin:$PATH
cd "${1:-.}" || exit 2
echo "HEAD: $(git rev-parse HEAD) dirty-files: $(git status --porcelain | wc -l | tr -d ' ')"
logs=$(mktemp -d /tmp/gate.XXXXXX)
fail=0
step() {
	local name="$1"
	shift
	echo "=== $name: $*"
	"$@" >"$logs/$name.log" 2>&1
	local code=$?
	if [ $code -eq 0 ]; then echo "PASS $name"; else
		echo "FAIL $name (exit $code)"
		tail -40 "$logs/$name.log"
		fail=1
	fi
	grep -E "^test result:" "$logs/$name.log" | awk '{p+=$4; f+=$6; i+=$8} END {if (NR) print "   tests: passed=" p " failed=" f " ignored=" i " (" NR " result lines)"}'
}
step fmt cargo fmt --all -- --check
step check cargo check --all-features
step clippy cargo clippy --all-features -- -D warnings
step clippy_all_targets cargo clippy --all-features --all-targets -- -D warnings
step unit cargo test --all-features --lib
step doc cargo doc --no-deps
step doctest cargo test --doc
step integration env RUST_MIN_STACK=8388608 cargo test --test '*'
step msrv_proxy_1_83 cargo +1.83.0 check --all-features
step ios_sim_build cargo test --no-run --target aarch64-apple-ios-sim
step examples cargo build --examples
step test_all ./test_all.sh
grep -E "running [0-9]+ tests|test result" "$logs/test_all.log" | tail -6
# CI installs the newest stable (rustc 1.99.1 as of 2026-11-10); run the toolchain-sensitive
# checks with it as well, in their own target directory.
step fmt_1_99 cargo +1.99.1 fmt --all -- --check
step clippy_1_99 env CARGO_TARGET_DIR=target/r199 cargo +1.99.1 clippy --all-features --all-targets -- -D warnings
step wasm_1_99 env RUSTUP_TOOLCHAIN=1.99.1 CARGO_TARGET_DIR=target/r199 WASM_BINDGEN_TEST_TIMEOUT=300 wasm-pack test --headless --chrome --features web
echo "LOGS $logs"
if [ $fail -eq 0 ]; then echo "GATE PASS"; else echo "GATE FAIL"; fi
exit $fail
