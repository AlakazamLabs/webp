#!/usr/bin/env bash
# Formats the tree and packs the changed files (for machines that don't
# build locally): out/fmt.tar holds every file cargo fmt changed.
set -euo pipefail
cargo fmt --all
mkdir -p out
git diff --name-only > out/fmt-files.txt
cat out/fmt-files.txt
tar -cf out/fmt.tar -T out/fmt-files.txt
