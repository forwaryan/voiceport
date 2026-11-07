#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
# Compatibility entry point; the active shared core is Rust.
cargo test -p voiceport-core --all-targets --locked
