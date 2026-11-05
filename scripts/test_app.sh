#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
mkdir -p .build
cargo build --workspace --release --locked
rust_target_directory="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
install -m 755 "$rust_target_directory/release/voiceport-rs" .build/vp011-real-helper
install -m 755 Tests/VoicePortAppTests/Fixtures/rust_host.py .build/vp011-host-fixture
swift build --target VoicePortCore
binary_directory="$(swift build --show-bin-path)"
swiftc -swift-version 6 -parse-as-library -module-name VoicePortAppChecks \
    -I "$binary_directory/Modules" \
    Sources/VoicePortApp/DictationDependencies.swift \
    Sources/VoicePortApp/RustBridge.swift \
    Sources/VoicePortApp/DictationController.swift \
    Sources/VoicePortApp/AudioRecorder.swift \
    Sources/VoicePortApp/RecordingStorage.swift \
    Sources/VoicePortApp/PasteCoordinator.swift \
    Tests/VoicePortAppTests/*.swift \
    "$binary_directory"/VoicePortCore.build/*.swift.o \
    -o .build/voiceport-app-checks
.build/voiceport-app-checks
