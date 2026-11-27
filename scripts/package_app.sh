#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
skip_build=false
distribution=false
test_build=false
for argument in "$@"; do
    case "$argument" in
        --skip-build) [[ "$skip_build" == false ]] || exit 2; skip_build=true ;;
        --distribution) [[ "$distribution" == false ]] || exit 2; distribution=true ;;
        --test) [[ "$test_build" == false ]] || exit 2; test_build=true ;;
        *) printf 'Usage: bash scripts/package_app.sh [--skip-build] [--distribution | --test]\n' >&2; exit 2 ;;
    esac
done
if [[ "$distribution" == true && "$test_build" == true ]]; then
    printf '%s\n' '--test and --distribution are mutually exclusive' >&2
    exit 2
fi
app_name=VoicePort.app
cli_name=voiceport
main_name=voiceport-app
if [[ "$test_build" == true ]]; then
    app_name='VoicePort Test.app'
    cli_name=voiceport-test
    main_name=voiceport-test-app
fi
app_directory="${VOICEPORT_APP_DIRECTORY:-$project_directory/dist/$app_name}"
if [[ "$test_build" == true && "$(basename -- "$app_directory")" != "$app_name" ]]; then
    printf 'Test bundle must be named %s\n' "$app_name" >&2
    exit 2
fi
app_directory="$(python3 -I -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$app_directory")"
# Never replace a bundle that might be running. Verification uses an owned,
# previously unused destination rather than updating the user's dist app.
if [[ -e "$app_directory" || -L "$app_directory" ]]; then
    printf 'App destination already exists; choose an unused VOICEPORT_APP_DIRECTORY: %s\n' "$app_directory" >&2
    exit 1
fi
# Resolve and validate before building or staging; errors never fall back to ad-hoc.
if [[ "$test_build" == true ]]; then
    export CARGO_NET_OFFLINE=true
elif [[ "$distribution" == true ]]; then
    signing_identity="$(python3 -I scripts/signing_identity.py --distribution)"
else
    signing_identity="$(python3 -I scripts/signing_identity.py)"
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
install -m 755 "$rust_main" "$staged_app/Contents/MacOS/$main_name"
install -m 755 "$rust_cli" "$staged_app/Contents/MacOS/$cli_name"
cp -R skills "$staged_app/Contents/Resources/skills"
bundle_check=(python3 -I scripts/check_bundle.py)
if [[ "$test_build" == true ]]; then
    python3 -I scripts/check_bundle.py --write-test-info "$staged_app/Contents/Info.plist"
    bundle_check+=(--test)
else
    install -m 644 resources/Info.plist "$staged_app/Contents/Info.plist"
fi
install -m 644 resources/AppIcon.icns "$staged_app/Contents/Resources/AppIcon.icns"
install -m 644 THIRD_PARTY_NOTICES.md "$staged_app/Contents/Resources/THIRD_PARTY_NOTICES.md"
cp -R resources/third-party "$staged_app/Contents/Resources/third-party"
plutil -lint "$staged_app/Contents/Info.plist"
# Use the verified identity selected before the build for both signatures.
if [[ "$distribution" == true ]]; then
    codesign --force --sign "$signing_identity" --options runtime --timestamp \
        --entitlements resources/VoicePortCLI.entitlements \
        --identifier io.github.forwaryan.voiceport.cli "$staged_app/Contents/MacOS/voiceport"
    codesign --force --sign "$signing_identity" --options runtime --timestamp \
        --entitlements resources/VoicePort.entitlements "$staged_app"
    python3 -I scripts/distribution_signing.py "$staged_app" --team-id "$VOICEPORT_TEAM_ID"
elif [[ "$test_build" == true ]]; then
    codesign --force --sign - --timestamp=none --identifier io.github.forwaryan.voiceport.test.cli "$staged_app/Contents/MacOS/$cli_name"
    codesign --force --sign - --timestamp=none "$staged_app"
else
    codesign --force --sign "$signing_identity" "$staged_app/Contents/MacOS/$cli_name"
    codesign --force --sign "$signing_identity" "$staged_app"
fi
"${bundle_check[@]}" "$staged_app" "$rust_main"
# Reserve the destination exclusively, including against another packaging run.
mkdir "$app_directory"
mv "$staged_app/Contents" "$app_directory/Contents"
printf 'Built %s\n' "$app_directory"
