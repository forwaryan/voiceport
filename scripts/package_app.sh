#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
skip_build=false
if [[ $# -eq 1 && "${1:-}" == "--skip-build" ]]; then
    skip_build=true
elif [[ $# -ne 0 ]]; then
    printf 'Usage: bash scripts/package_app.sh [--skip-build]\n' >&2
    exit 2
fi
app_directory="${VOICEPORT_APP_DIRECTORY:-$project_directory/dist/VoicePort.app}"
app_directory="$(python3 -I -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$app_directory")"
# Never replace a bundle that might be running. Verification uses an owned,
# previously unused destination rather than updating the user's dist app.
if [[ -e "$app_directory" || -L "$app_directory" ]]; then
    printf 'App destination already exists; choose an unused VOICEPORT_APP_DIRECTORY: %s\n' "$app_directory" >&2
    exit 1
fi
if [[ "$skip_build" == false ]]; then
    cargo build --workspace --release --locked
fi
rust_target_directory="$(cargo metadata --locked --no-deps --format-version 1 | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
rust_main="$rust_target_directory/release/voiceport-macos"
rust_cli="$rust_target_directory/release/voiceport"
for product in "$rust_main" "$rust_cli"; do
    [[ -f "$product" && ! -L "$product" && -x "$product" ]] || { printf 'Rust release product is required: %s\n' "$product" >&2; exit 1; }
done
python3 -I scripts/license_materials.py
mkdir -p "$(dirname -- "$app_directory")"
staging_directory="$(mktemp -d "$(dirname -- "$app_directory")/.voiceport-package.XXXXXX")"
trap 'rm -rf "$staging_directory"' EXIT
staged_app="$staging_directory/$(basename -- "$app_directory")"
mkdir -p "$staged_app/Contents/MacOS" "$staged_app/Contents/Resources"
install -m 755 "$rust_main" "$staged_app/Contents/MacOS/voiceport-app"
install -m 755 "$rust_cli" "$staged_app/Contents/MacOS/voiceport"
cp -R skills "$staged_app/Contents/Resources/skills"
install -m 644 resources/Info.plist "$staged_app/Contents/Info.plist"
install -m 644 resources/AppIcon.icns "$staged_app/Contents/Resources/AppIcon.icns"
install -m 644 THIRD_PARTY_NOTICES.md "$staged_app/Contents/Resources/THIRD_PARTY_NOTICES.md"
cp -R resources/third-party "$staged_app/Contents/Resources/third-party"
plutil -lint "$staged_app/Contents/Info.plist"
# Reuse one certificate across builds for a stable designated requirement.
# Never silently fall back to ad-hoc signing when a chosen identity fails.
signing_identity="${VOICEPORT_CODESIGN_IDENTITY:--}"
if [[ "$signing_identity" == "-" ]]; then
    printf 'Using ad-hoc signing; macOS permissions may require approval after updates.\n' >&2
fi
codesign --force --sign "$signing_identity" "$staged_app/Contents/MacOS/voiceport"
codesign --force --sign "$signing_identity" "$staged_app"
python3 -I scripts/check_bundle.py "$staged_app" "$rust_main"
# Reserve the destination exclusively, including against another packaging run.
mkdir "$app_directory"
mv "$staged_app/Contents" "$app_directory/Contents"
printf 'Built %s\n' "$app_directory"
