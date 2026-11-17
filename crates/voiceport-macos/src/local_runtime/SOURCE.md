# Versioned C interface

`c-api.h` is an unmodified copy of https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/v1.12.37/sherpa-onnx/c-api/c-api.h (retrieved 2026-11-08).

SHA-256: `437b1279047877167d8fadc74a60d47f3df514d703fdac1c1b6851da9bc2fdb4`.

Copyright (c) 2023 Xiaomi Corporation. The full upstream Apache-2.0 license is copied unchanged from https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/v1.12.37/LICENSE into `LICENSE`.

`bridge.cc` is VoicePort code. Its C ABI catches C++ inference exceptions before returning errors to Rust. Compiling against this header keeps native configuration layouts and function signatures defined by the exact upstream ABI. Dynamic loading checks `SherpaOnnxGetVersionStr()` before creating any recognizer. Only runtime version 1.12.37 and arm64 are currently supported. The implementation and model weights are not bundled in the app. The model library can explicitly download pinned official runtime/model artifacts into VoicePort's user-data directory, with size/hash checks and runtime license notices; compatible Orca dependencies remain read-only optional sources. See `docs/LOCAL_MODELS.md` and `model_library/catalog.rs`.

The bridge selects distinct Paraformer streaming, Whisper Tiny, Whisper Large v3 Turbo and SenseVoice offline configurations. Turbo uses its 128-dimensional feature configuration; SenseVoice uses the header's native SenseVoice fields, automatic/explicit language and inverse text normalization. SenseVoice installation also retains its custom FunASR Model License v1.1 and author/source attribution; this is separate from the sherpa runtime's Apache license.

The build invokes the existing macOS Command Line Tools clang and ar from a Cargo build script. No downloaded build tool is required. Model installation uses the existing reqwest/Tokio stack and ring's SHA-256/SHA-512 digest implementation.
