#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
# Compatibility entry point; hardware-free native app regressions.
cargo test -p voiceport-macos --all-targets --locked
