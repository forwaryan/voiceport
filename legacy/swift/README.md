# Swift migration baseline

This directory preserves the replaced Swift implementation and its behavior tests for historical comparison. The active macOS app is `crates/voiceport-macos`; its native AppKit, audio, Speech, permissions and text delivery are implemented in Rust and link `voiceport-core` directly.

Nothing in this directory is built, packaged, launched or tested by the current default workflow. The former root `Package.swift`, `Sources`, and Swift test directories were moved here without changing their source contents. Older validation documents and frozen evidence use their original paths; those records refer to the snapshots they name and remain historical evidence, not verification of the new Rust application.

Current commands from the repository root:

```sh
bash scripts/test_rust.sh
python3 -I scripts/verify.py --app-smoke
```
