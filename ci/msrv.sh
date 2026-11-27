#!/usr/bin/env bash
# The MSRV check: the library on the crate's rust-version, default and all
# features (dev-dependencies need newer Rust, so no tests here).
set -euo pipefail
v=$(grep '^rust-version' webp-lossy/Cargo.toml | cut -d'"' -f2)
export RUSTUP_TOOLCHAIN="$v"
rustc --version
cargo check -p webp-lossy --lib
cargo check -p webp-lossy --lib --all-features
cargo check -p webp-lossy --lib --no-default-features
echo "MSRV $v: ok"
