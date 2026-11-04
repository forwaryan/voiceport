#!/bin/bash
set -euo pipefail
project_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_directory"
mkdir -p .build
swiftc -swift-version 6 -parse-as-library -module-name VoicePortChecks \
    Sources/VoicePortCore/*.swift Tests/VoicePortCoreTests/*.swift \
    -o .build/voiceport-core-checks
.build/voiceport-core-checks
