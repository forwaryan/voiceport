#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
if [[ $# -eq 1 && "${1:-}" == "--skip-build" ]]; then
    :
elif [[ $# -eq 0 ]]; then
    swift build -c release
    cargo build --workspace --release --locked
else
    printf 'Usage: bash scripts/package_app.sh [--skip-build]\n' >&2
    exit 2
fi
binary_directory="$(swift build -c release --show-bin-path)"
app_directory="${VOICEPORT_APP_DIRECTORY:-$project_directory/dist/VoicePort.app}"
rust_target_directory="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
python3 -I scripts/license_materials.py
[[ -x "$rust_target_directory/release/voiceport-rs" && -x "$binary_directory/VoicePort" ]] || { printf 'Both Swift and Rust release products are required\n' >&2; exit 1; }
mkdir -p "$app_directory/Contents/MacOS" "$app_directory/Contents/Resources" "$app_directory/Contents/Helpers"
install -m 755 "$rust_target_directory/release/voiceport-rs" "$app_directory/Contents/Helpers/voiceport-rs"
codesign --force --sign - "$app_directory/Contents/Helpers/voiceport-rs"
install -m 755 "$binary_directory/VoicePort" "$app_directory/Contents/MacOS/VoicePort"
install -m 644 resources/Info.plist "$app_directory/Contents/Info.plist"
install -m 644 THIRD_PARTY_NOTICES.md "$app_directory/Contents/Resources/THIRD_PARTY_NOTICES.md"
rm -rf "$app_directory/Contents/Resources/third-party"
cp -R resources/third-party "$app_directory/Contents/Resources/third-party"
python3 -I scripts/license_materials.py --directory "$app_directory/Contents/Resources/third-party"
plutil -lint "$app_directory/Contents/Info.plist"
codesign --force --sign - "$app_directory"
codesign --verify --deep --strict "$app_directory"
python3 -I scripts/check_bundle.py "$app_directory" "$rust_target_directory/release/voiceport-rs" "$binary_directory/VoicePort"
printf 'Built %s\n' "$app_directory"
