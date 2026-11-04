#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
if [[ "${1:-}" == "--skip-build" ]]; then
    :
elif [[ $# -eq 0 ]]; then
    swift build -c release
else
    printf 'Usage: bash scripts/package_app.sh [--skip-build]\n' >&2
    exit 2
fi
binary_directory="$(swift build -c release --show-bin-path)"
app_directory="$project_directory/dist/VoicePort.app"
mkdir -p "$app_directory/Contents/MacOS" "$app_directory/Contents/Resources"
install -m 755 "$binary_directory/VoicePort" "$app_directory/Contents/MacOS/VoicePort"
install -m 644 resources/Info.plist "$app_directory/Contents/Info.plist"
install -m 644 THIRD_PARTY_NOTICES.md "$app_directory/Contents/Resources/THIRD_PARTY_NOTICES.md"
plutil -lint "$app_directory/Contents/Info.plist"
codesign --force --sign - "$app_directory"
codesign --verify --deep --strict "$app_directory"
printf 'Built %s\n' "$app_directory"
