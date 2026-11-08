# Versioned C interface

`c-api.h` is an unmodified copy of https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/v1.12.37/sherpa-onnx/c-api/c-api.h (retrieved 2026-11-08).

SHA-256: `437b1279047877167d8fadc74a60d47f3df514d703fdac1c1b6851da9bc2fdb4`.

Copyright (c) 2023 Xiaomi Corporation. The full upstream Apache-2.0 license is copied unchanged from https://raw.githubusercontent.com/k2-fsa/sherpa-onnx/v1.12.37/LICENSE into `LICENSE`.

`bridge.cc` is VoicePort code. Its C ABI catches C++ inference exceptions before returning errors to Rust. Compiling against this header keeps native configuration layouts and function signatures defined by the exact upstream ABI. Dynamic loading checks `SherpaOnnxGetVersionStr()` before creating any recognizer. Only runtime version 1.12.37 and arm64 are currently supported. The implementation and model weights are external, read-only installation dependencies; they are not vendored or redistributed.

The build invokes the existing macOS Command Line Tools clang and ar from a Cargo build script. No new Cargo dependency or downloaded build tool is required.
