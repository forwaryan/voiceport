# Local arm64 macOS Rust development package

Run the complete offline path from any working directory:

```sh
python3 -I /absolute/path/to/voiceport/scripts/verify.py --app-smoke
```

Requires Rust/Cargo 1.95, rustfmt, clippy, Python 3.11+ and macOS codesign/lipo/plutil. Dependencies must already be cached: verification sets `CARGO_NET_OFFLINE=true` and uses the committed lockfile. The default package and verification paths contain no Swift build or test commands. The obsolete Swift sources, manifest and tests have been removed from the current tree and remain available in Git history; compatibility test scripts run Rust checks.

No credentials are needed. Each verifier run creates a fresh synthetic `CODEX_HOME`, canonical `TMPDIR` and unused `VOICEPORT_APP_DIRECTORY` inside an owned temporary scope, then removes that scope afterward. It saves per-command stdout/stderr, argv/cwd/environment differences, expected/actual exits, timeouts, input inventories and artifact hashes under `.voiceport/evidence/` (the exact report path is printed). CLI negative cases deliberately return 1/2 and are distinguished from failed verification. The CLI invalid-auth case reads only the deliberately invalid synthetic fixture.

The entry runs Rust workspace tests/fmt/clippy, the normal locked Rust release package, Python loop/packaging checks, the separate Rust CLI's help/demo/doctor/argument and synthetic-auth cases, its ready/status/shutdown host protocol, and bundle/notice/signature correspondence. The app directly links `voiceport-core`. Its GUI executable is `Contents/MacOS/voiceport-app`, built from `voiceport-macos`; the public CLI is the separate signed `Contents/MacOS/voiceport`, built from `voiceport-cli`. These distinct names coexist on case-insensitive macOS filesystems. No helper daemon or Swift runtime is packaged. The local-model C bridge is compiled into this main using the exact sherpa-onnx 1.12.37 header and macOS Command Line Tools clang/ar; no Cargo dependency was added. Paraformer and Whisper load the separately installed arm64 Orca runtime from `/Applications/Orca.app/Contents/Resources/node_modules/sherpa-onnx-darwin-arm64` by absolute path, so a Finder launch requires no Node or shell PATH. Models remain read-only under `~/Library/Application Support/orca/speech-models`; neither model files nor runtime dylibs are copied into the app. An unavailable selected local model fails clearly instead of using cloud auth. See [local models](LOCAL_MODELS.md) for exact files, supported version, architecture, permissions and limitations. Validation preserves the standalone `release/voiceport-rs` compatibility interface and checks the public CLI. The exact repository `skills/voiceport/` tree is installed under `Contents/Resources/skills/voiceport/`; its core and references are also embedded directly in both CLI entry points for offline, version-matched `skills list/get`. See [CLI and Skill](CLI.md).

`--app-smoke` additionally executes only the bundle's `--smoke-test` and `--check-input-permission` diagnostics. Smoke constructs the native menu and exits before initializing the controller, preferences, permissions, storage, worker or hotkey. Its stdout is exactly `VoicePort Rust app launched: menu ready, no recording or network` followed by a newline. The permission diagnostic emits JSON with boolean `accessibilityTrusted`/`secureInput`, string `bundlePath`/`version`, and `implementation: "rust"`. It requests no permission and inspects no input target. Neither path reads credentials, records audio, accesses the network, delivers input or accesses application storage. Direct execution verifies the permission JSON shape only; an effective TCC identity claim requires a separate Launch Services launch. Verification never uses `--show-window` or an ordinary app launch. Real user audio, targets and active GUI exit remain separate.

## Packaging and product correspondence

```sh
VOICEPORT_APP_DIRECTORY=/unused/path/VoicePort.app bash scripts/package_app.sh
```

Without `VOICEPORT_APP_DIRECTORY`, the destination is `dist/VoicePort.app`. Packaging refuses **any existing destination**, including a symlink, before building or installing. Choose a new unused output path for each run; this protects an existing app that may be running. Packaging stages and validates the complete bundle before reserving the chosen destination and installing it. Tests and the verifier use owned temporary paths and never replace the default app.

`bash scripts/package_app.sh` runs `cargo build --workspace --release --locked`. `--skip-build` performs no compilation and requires existing executable `release/voiceport-macos` and `release/voiceport` products under Cargo metadata's actual `target_directory`. Both modes check supplied licence resources, install both executables, the exact Skill tree and source Info.plist/AppIcon.icns/notices, sign the CLI and bundle with the selected identity (ad-hoc only when no identity is configured), then validate them. `--distribution` additionally requires a Developer ID Application certificate for `VOICEPORT_TEAM_ID`, enables hardened runtime and secure timestamps, and validates exact app/CLI identities and entitlements; it may be combined with `--skip-build`. See [macOS signing, notarization and update identity](MACOS_UPDATES.md) for `release_macos.py`, the manual CI workflow, credentials and interrupted-submission recovery. The committed original microphone icon is installed without regeneration. Version 0.3.3 uses a compact native window with Dictation and Settings tabs, a contextual status notice, and result actions shown when applicable. It is a menu bar utility (LSUIElement=true, Accessory activation policy): its entry belongs in the upper-right menu bar and it does not occupy the Dock. Ordinary launch stays in the menu bar. Clicking its status icon or reopening the app presents only a compact source switcher, hiding any existing full window. The switcher has one open-window icon; Dictation and Settings are tabs inside that explicitly opened window; `--show-window` also explicitly requests the full window on launch. Closing the full window leaves the menu bar utility running. None of these actions starts recording. Unknown/extra arguments fail before Cargo runs. The script resolves the repository from its own path, so an external cwd is supported. `CARGO_TARGET_DIR` is honored through metadata.

Bundle checking takes two explicit paths:

```sh
python3 -I scripts/check_bundle.py APP RUST_MACOS_RELEASE
```

The Python API is `check_bundle.validate(app, rust_macos_release)`. The reviewed architecture is native arm64; no universal/Intel/Windows assertion is made. The checker rejects linked roots/descendants and special files, unexpected bundle files or helper directories, a missing/non-executable main, wrong architecture, altered Info.plist/icon/notices, licence corruption and invalid signatures. Info.plist must equal the source bytes and identify executable `voiceport-app` with mode 0755. The exact MacOS file set is `voiceport-app` and `voiceport`, each mode 0755; no legacy `VoicePort` alias is installed. The Skill directory and file bytes must equal the repository source. The source bundle ID is preserved.

A same-name bundle twin is made with the specified current Rust `voiceport-macos` product and its sibling `voiceport` CLI product, given the same executable modes and independently re-signed after normalizing certificate, timestamp, runtime flags, entitlements and identifiers in temporary copies. Original distribution signatures are verified separately and never modified by this comparison. Each executable must hash identically to its corresponding installed file. This accounts for bundle Info/identifier signature slots rather than equating original and signed bytes. No substituted main is executed. Tests replace the app main with the wrong Rust CLI, re-sign and successfully verify that signature, then require product-correspondence rejection.

Standalone packaging tests require an already built locked workspace release. They use an existing `VOICEPORT_APP_DIRECTORY` read-only when provided; otherwise they package existing products with `--skip-build` into a fresh temporary app:

```sh
python3 -I -m unittest discover -s Tests/PackagingTests -v
```

Negative cases cover unknown arguments, missing release products without implicit rebuilding, custom metadata/output paths from an external cwd, existing output protection, executable modes, wrong re-signed GUI and CLI products, tampered/extra Skill files, missing/tampered/unlisted licence material, unknown declarations/selections, omitted packages, root/descendant symlinks, extra helpers and inventory drift.

## Isolated macOS test ZIP

```sh
python3 -I scripts/macos_artifacts.py test-build dist/test-01
```

Use an unused output directory with an existing parent. The command takes the shared host and checkout verification locks, builds from cached dependencies with the locked workspace, and uses the same package and bundle correspondence checks as a normal build. `--skip-build` reuses the selected Cargo target directory's release products from the current checkout. The package checks both products expose the test profile before creating the ZIP. Output contains `VoicePort-Test-macos-arm64.zip`, `artifact-manifest.json`, and the manifest digest in the JSON receipt on stdout. Existing outputs are rejected. Signing is always ad-hoc for this profile.

Extract into a new location. Finder shows **VoicePort Test** with bundle ID `io.github.forwaryan.voiceport.test`. If launch has been authorized, open that exact `.app`, and use its bundled `Contents/MacOS/voiceport-test` for control:

```sh
"/absolute/extracted/VoicePort Test.app/Contents/MacOS/voiceport-test" --json app status
"/absolute/extracted/VoicePort Test.app/Contents/MacOS/voiceport-test" --json app update status
```

Confirm `data.bundle_path` and `pid` before mutations. App and CLI use `/private/tmp/voiceport-test-control-UID/control.sock`. The App ignores the inherited `VOICEPORT_CONTROL_SOCKET`; the test CLI accepts only its own endpoint, including when `--socket` is supplied. Keep both executables inside the bundle with their shipped names so the test profile stays active. macOS uses the separate bundle ID for preferences. Sources, managed models and default ChatGPT auth live under `~/Library/Application Support/VoicePort Test/`; the auth file is `codex/auth.json`. Source, model and Codex home overrides used by the regular build are ignored for these defaults. Recordings use `~/Library/Caches/io.voiceport.test-rust-recordings-v1`; plugin credentials use the `VoicePortTest.TranscriptionSources.v1` Keychain service. The test profile does not discover models or runtime libraries from the regular Orca installation. Global dictation shortcut and automatic input are off on first launch. Use synthetic data in isolation; macOS permissions and hardware are still managed by the OS.

The workbench shows **VoicePort Test** and its Software Update page shows updates disabled. `app update status` returns `state:"disabled"` and `app update check` returns `unsupported`. Automatic checks, downloads and installs are disabled. See [verification of a final ZIP and its attachments](MACOS_UPDATES.md#核验最终-zip-和附件) for checking the ZIP before opening it.

## Product inputs and licence materials

Complete product manifests include `.gitignore`, Cargo manifests and lockfile, README/THIRD_PARTY_NOTICES, and all regular inputs under Tests/crates/scripts/resources/automation/docs/skills, including new/untracked files. Entries have path/bytes/SHA256/mode and a directory set. New/deleted/content/mode changes fail start/end equality; symlink/special inputs fail closed. target/.build/dist, private evidence/coordination, root planning and generated pycache are excluded. This is a run-integrity check, not the coordinator's independent acceptance freeze.

`resources/third-party/manifest.json` records the locked native target/feature graph, crate public source/version/SPDX, scope and choice, source-relative material paths, byte counts/hashes and Cargo input hashes. The installed copy is `Contents/Resources/third-party/`. Runtime reachability does not mean every object is linked. Build tools/proc macros, dev overlap and excluded-platform lock records are separately labelled. Dev/build materials are included conservatively; lock-only excluded platforms are recorded without pretending they ship in this native executable.

Full supplied licence alternatives are retained. The recorded selection prefers MIT in OR expressions, else Apache-2.0; AND terms are retained together. Embedded components keep their own notices regardless of the crate-level choice. Source copyright/attribution files are copied in full as supplementary materials, including ring/BoringSSL/fiat/once_cell headers and licence files. The MPL-2.0 package includes its complete cached package source rather than only a link. CDLA certificate-data and Unicode terms are included. Existing GPT Voice MIT attribution remains in the top-level notice and bundle.

The native inventory currently contains 153 packages and 954 material files. For objc2-family archives that omit their repository licensing declaration and full standard terms, `resources/license-sources/` retains declarations from each exact upstream commit and full applicable SPDX 3.26.0 MIT/Apache-2.0/Zlib texts. The generator pins their hashes, checks the crate's version/commit/declaration, records source URLs, and installs those texts with the original Cargo metadata and commit evidence. The standard templates and upstream SDK derivation caveat are retained unchanged; copyright holders are not invented. Missing, altered or linked supplemental sources fail offline validation.

To refresh after a deliberate dependency/feature change, run `python3 -I scripts/license_materials.py --generate`, review actual source materials/choices and the manifest diff, then verify. Packaging only checks; it never silently regenerates or uses a cache path as a substitute for shipped texts. Missing/unknown declarations, missing/tampered/unlisted resources and Cargo input drift fail. Only the native aarch64-apple-darwin graph is reviewed. This is local development material completeness, not legal approval, project licence selection, notarization or permission for external release.

## Trust boundaries

The public inventory uses schema 2: file rows preserve sorted(Path) order, directory names are sorted strings, and directory_modes maps every actual directory path to its POSIX mode. Canonical JSON (sort_keys, compact separators) of schema/missing/files/directories/directory_modes is hashed before adding sha256. Every required root and descendant is lstat checked before enumeration; missing roots, wrong types, symlinks and special files fail. Old three-field freeze digests and schema-1 public digests are different formats.

Licence checking reconstructs the expected native Cargo metadata package/feature/scope graph and reviewed choices from locked inputs and read-only cached upstream source. It recomputes the expected complete material set, bytes and hashes without writing or regenerating resources. The source manifest must match this authority, then the installed manifest must equal that validated source. Missing/extra/duplicate packages, unknown declaration or choice, omitted build inputs, changed feature/scope/source identity and self-consistent material omission fail. The cache is used as a pinned source reference, not as a substitute for installed texts. Native metadata features include conservative workspace/dev unification; the separately recorded normal/build Cargo tree distinguishes release reachability from that superset.
