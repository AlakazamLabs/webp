#!/usr/bin/env bash
# Fuzzes one target (FUZZ_TARGET: still or anim, default still) with
# cargo-fuzz (nightly) for FUZZ_SECS seconds (default 1200) on FUZZ_JOBS
# processes (default 1). The corpus lives in $WEBP_CI_CACHE (default
# .ci-cache in the repo) so later runs build on it. Prints the total
# executions; any crash input is copied to out/artifacts.
set -euo pipefail
target="${FUZZ_TARGET:-still}"
secs="${FUZZ_SECS:-1200}"
jobs="${FUZZ_JOBS:-1}"
cache="${WEBP_CI_CACHE:-$PWD/.ci-cache}/fuzz"
mkdir -p "$cache/corpus-$target" "$cache/bin"
export PATH="$cache/bin/bin:$PATH"
if ! command -v cargo-fuzz >/dev/null; then
  cargo install --locked --root "$cache/bin" cargo-fuzz
fi
rustc --version
cd fuzz
status=0
# Release, debug assertions (overflow checks) on; no sanitizer (the crate
# forbids unsafe code), unless FUZZ_SANITIZER says otherwise.
san="${FUZZ_SANITIZER:-none}"
cargo fuzz build -O -a --sanitizer "$san" "$target"
cargo fuzz run -O -a --sanitizer "$san" "$target" "$cache/corpus-$target" -- -max_total_time="$secs" -rss_limit_mb=2048 \
  -max_len=8192 -print_final_stats=1 -fork="$jobs" -ignore_crashes=0 ${FUZZ_EXTRA:-} 2>&1 | tee ../out-fuzz.log | { grep -v "^#\|NEW_FUNC" || true; } || status=$?
grep -o "stat::number_of_executed_units: [0-9]*\|NEW_FUNC\|INFO: fuzzed for [0-9]* seconds" ../out-fuzz.log | tail -3 || true
grep -o "#[0-9]*" ../out-fuzz.log | tail -1 | xargs echo "last exec count line:"
mkdir -p ../out
cp -r artifacts ../out/ 2>/dev/null || true
ls "$cache/corpus-$target" | wc -l | xargs echo "corpus entries:"
exit $status
