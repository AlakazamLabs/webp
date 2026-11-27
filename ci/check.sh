#!/usr/bin/env bash
# The crate's gates: fmt, clippy (-D warnings), tests, docs (-D warnings),
# examples and the package list. Runs every step and fails if any failed.
set -uo pipefail
fail=0
step() {
  echo "=== $*"
  if ! "$@"; then
    echo "!!! FAILED: $*"
    fail=1
  fi
}
rustc --version
cargo --version
step cargo fmt --all --check
step cargo clippy --workspace --all-targets -- -D warnings
step cargo clippy -p webp-lossy --all-targets --all-features -- -D warnings
step cargo clippy -p webp-lossy --all-targets --no-default-features -- -D warnings
step cargo test -p webp-lossy
step cargo test -p webp-lossy --release --test edge_cases
step cargo test -p webp-lossy --all-features
step cargo test -p webp-oracle
step env RUSTDOCFLAGS="-D warnings" cargo doc -p webp-lossy --no-deps
step env RUSTDOCFLAGS="-D warnings" cargo doc -p webp-lossy --no-deps --all-features
step cargo build -p webp-lossy --examples
step cargo package -p webp-lossy --list --allow-dirty
exit $fail
