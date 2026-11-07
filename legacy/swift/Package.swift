// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "VoicePort",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "VoicePortCore", targets: ["VoicePortCore"]),
        .executable(name: "voiceport-cli", targets: ["VoicePortCLI"]),
        .executable(name: "VoicePort", targets: ["VoicePortApp"]),
    ],
    targets: [
        .target(name: "VoicePortCore"),
        .executableTarget(name: "VoicePortCLI", dependencies: ["VoicePortCore"]),
        .executableTarget(name: "VoicePortApp", dependencies: ["VoicePortCore"]),
    ]
)
