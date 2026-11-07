# Local arm64 macOS Rust development package

Run the complete offline path from any working directory:

```sh
python3 -I /absolute/path/to/voiceport/scripts/verify.py --app-smoke
```

Requires Rust/Cargo 1.95, rustfmt, clippy, Python 3.11+ and macOS codesign/lipo/plutil. Dependencies must already be cached: verification sets `CARGO_NET_OFFLINE=true` and uses the committed lockfile. The default package and verification paths contain no Swift build or test commands. The obsolete Swift sources, manifest and tests have been removed from the current tree and remain available in Git history; compatibility test scripts run Rust checks.

No credentials are needed. Each verifier run creates a fresh synthetic `CODEX_HOME`, canonical `TMPDIR` and unused `VOICEPORT_APP_DIRECTORY` inside an owned temporary scope, then removes that scope afterward. It saves per-command stdout/stderr, argv/cwd/environment differences, expected/actual exits, timeouts, input inventories and artifact hashes under `.voiceport/evidence/` (the exact report path is printed). CLI negative cases deliberately return 1/2 and are distinguished from failed verification. The CLI invalid-auth case reads only the deliberately invalid synthetic fixture.

The entry runs Rust workspace tests/fmt/clippy, the normal locked Rust release package, Python loop/packaging checks, the separate Rust CLI's help/demo/doctor/argument and synthetic-auth cases, its ready/status/shutdown host protocol, and bundle/notice/signature correspondence. The app directly links `voiceport-core`; it has one runtime executable, `Contents/MacOS/VoicePort`, built from `voiceport-macos`. No helper executable or Swift runtime is packaged. CLI validation uses `release/voiceport-rs` separately; the CLI is not installed in the app.

`--app-smoke` additionally executes only the bundle's `--smoke-test` and `--check-input-permission` diagnostics. Smoke constructs the native menu and exits before initializing the controller, preferences, permissions, storage, worker or hotkey. Its stdout is exactly `VoicePort Rust app launched: menu ready, no recording or network` followed by a newline. The permission diagnostic emits JSON with boolean `accessibilityTrusted`/`secureInput`, string `bundlePath`/`version`, and `implementation: "rust"`. It requests no permission and inspects no input target. Neither path reads credentials, records audio, accesses the network, delivers input or accesses application storage. Direct execution verifies the permission JSON shape only; an effective TCC identity claim requires a separate Launch Services launch. Verification never uses `--show-window` or an ordinary app launch. Real user audio, targets and active GUI exit remain separate.

## Packaging and product correspondence

```sh
VOICEPORT_APP_DIRECTORY=/unused/path/VoicePort.app bash scripts/package_app.sh
```

Without `VOICEPORT_APP_DIRECTORY`, the destination is `dist/VoicePort.app`. Packaging refuses **any existing destination**, including a symlink, before building or installing. Choose a new unused output path for each run; this protects an existing app that may be running. Packaging stages and validates the complete bundle before reserving the chosen destination and installing it. Tests and the verifier use owned temporary paths and never replace the default app.

`bash scripts/package_app.sh` runs `cargo build --workspace --release --locked`. `--skip-build` performs no compilation and requires an existing executable `release/voiceport-macos` under Cargo metadata's actual `target_directory`. It does not require the separate CLI product. Both modes check supplied licence resources, install the Rust main and unchanged source Info.plist/notices, ad-hoc sign the bundle, then validate it. Unknown/extra arguments fail before Cargo runs. The script resolves the repository from its own path, so an external cwd is supported. `CARGO_TARGET_DIR` is honored through metadata.

Bundle checking takes two explicit paths:

```sh
python3 -I scripts/check_bundle.py APP RUST_MACOS_RELEASE
```

The Python API is `check_bundle.validate(app, rust_macos_release)`. The reviewed architecture is native arm64; no universal/Intel/Windows assertion is made. The checker rejects linked roots/descendants and special files, unexpected bundle files or helper directories, a missing/non-executable main, wrong architecture, altered Info.plist/notices, licence corruption and invalid signatures. Info.plist must equal the source bytes and identify executable `VoicePort` with mode 0755. The source bundle ID is preserved.

A same-name bundle twin is made with the specified current Rust `voiceport-macos` product, given the same executable mode and independently re-signed. Its main must hash identically to the installed main. This accounts for bundle Info/identifier signature slots rather than equating original and signed bytes. No substituted main is executed. Tests replace the app main with the wrong Rust CLI, re-sign and successfully verify that signature, then require product-correspondence rejection.

Standalone packaging tests require an already built locked workspace release. They use an existing `VOICEPORT_APP_DIRECTORY` read-only when provided; otherwise they package existing products with `--skip-build` into a fresh temporary app:

```sh
python3 -I -m unittest discover -s Tests/PackagingTests -v
```

Negative cases cover unknown arguments, missing release products without implicit rebuilding, custom metadata/output paths from an external cwd, existing output protection, executable modes, wrong re-signed Rust mains, missing/tampered/unlisted licence material, unknown declarations/selections, omitted packages, root/descendant symlinks, extra helpers and inventory drift.

## Product inputs and licence materials

Complete product manifests include `.gitignore`, Cargo manifests and lockfile, README/THIRD_PARTY_NOTICES, and all regular inputs under Tests/crates/scripts/resources/automation/docs, including new/untracked files. Entries have path/bytes/SHA256/mode and a directory set. New/deleted/content/mode changes fail start/end equality; symlink/special inputs fail closed. target/.build/dist, private evidence/coordination, root planning and generated pycache are excluded. This is a run-integrity check, not the coordinator's independent acceptance freeze.

`resources/third-party/manifest.json` records the locked native target/feature graph, crate public source/version/SPDX, scope and choice, source-relative material paths, byte counts/hashes and Cargo input hashes. The installed copy is `Contents/Resources/third-party/`. Runtime reachability does not mean every object is linked. Build tools/proc macros, dev overlap and excluded-platform lock records are separately labelled. Dev/build materials are included conservatively; lock-only excluded platforms are recorded without pretending they ship in this native executable.

Full supplied licence alternatives are retained. The recorded selection prefers MIT in OR expressions, else Apache-2.0; AND terms are retained together. Embedded components keep their own notices regardless of the crate-level choice. Source copyright/attribution files are copied in full as supplementary materials, including ring/BoringSSL/fiat/once_cell headers and licence files. The MPL-2.0 package includes its complete cached package source rather than only a link. CDLA certificate-data and Unicode terms are included. Existing GPT Voice MIT attribution remains in the top-level notice and bundle.

The native inventory currently contains 153 packages and 954 material files. For objc2-family archives that omit their repository licensing declaration and full standard terms, `resources/license-sources/` retains declarations from each exact upstream commit and full applicable SPDX 3.26.0 MIT/Apache-2.0/Zlib texts. The generator pins their hashes, checks the crate's version/commit/declaration, records source URLs, and installs those texts with the original Cargo metadata and commit evidence. The standard templates and upstream SDK derivation caveat are retained unchanged; copyright holders are not invented. Missing, altered or linked supplemental sources fail offline validation.

To refresh after a deliberate dependency/feature change, run `python3 -I scripts/license_materials.py --generate`, review actual source materials/choices and the manifest diff, then verify. Packaging only checks; it never silently regenerates or uses a cache path as a substitute for shipped texts. Missing/unknown declarations, missing/tampered/unlisted resources and Cargo input drift fail. Only the native aarch64-apple-darwin graph is reviewed. This is local development material completeness, not legal approval, project licence selection, notarization or permission for external release.

## Trust boundaries

The public inventory uses schema 2: file rows preserve sorted(Path) order, directory names are sorted strings, and directory_modes maps every actual directory path to its POSIX mode. Canonical JSON (sort_keys, compact separators) of schema/missing/files/directories/directory_modes is hashed before adding sha256. Every required root and descendant is lstat checked before enumeration; missing roots, wrong types, symlinks and special files fail. Old three-field freeze digests and schema-1 public digests are different formats.

Licence checking reconstructs the expected native Cargo metadata package/feature/scope graph and reviewed choices from locked inputs and read-only cached upstream source. It recomputes the expected complete material set, bytes and hashes without writing or regenerating resources. The source manifest must match this authority, then the installed manifest must equal that validated source. Missing/extra/duplicate packages, unknown declaration or choice, omitted build inputs, changed feature/scope/source identity and self-consistent material omission fail. The cache is used as a pinned source reference, not as a substitute for installed texts. Native metadata features include conservative workspace/dev unification; the separately recorded normal/build Cargo tree distinguishes release reachability from that superset.
