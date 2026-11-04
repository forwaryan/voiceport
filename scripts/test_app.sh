#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
swift build --target VoicePortCore
binary_directory="$(swift build --show-bin-path)"
swiftc -swift-version 6 -parse-as-library -module-name VoicePortAppChecks \
    -I "$binary_directory/Modules" \
    Sources/VoicePortApp/DictationDependencies.swift \
    Sources/VoicePortApp/DictationController.swift \
    Sources/VoicePortApp/AudioRecorder.swift \
    Sources/VoicePortApp/RecordingStorage.swift \
    Sources/VoicePortApp/PasteCoordinator.swift \
    Tests/VoicePortAppTests/*.swift \
    "$binary_directory"/VoicePortCore.build/*.swift.o \
    -o .build/voiceport-app-checks
.build/voiceport-app-checks
