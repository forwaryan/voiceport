#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
cargo test --workspace --all-targets --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
# The existing macOS CI invokes this entry point; keep experimental streaming
# checks offline, with no authentication or network access.
python3 -I -B -m unittest discover -s Tests/StreamingTests -v
