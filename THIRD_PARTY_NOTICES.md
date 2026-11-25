# VoicePort and third-party notices

## VoicePort

VoicePort's own code and documentation are licensed under the MIT License in [LICENSE](LICENSE). Third-party components retain their respective licenses and notices below. The project license is reproduced here so that the app's bundled copy of this file also carries the full VoicePort notice.

```text
MIT License

Copyright (c) 2026 forwaryan and contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## GPT Voice

VoicePort's transcription protocol and macOS delivery design were informed by [GPT Voice](https://github.com/entropyconquers/gpt-voice/tree/0f3f29ffcb2bfe83806c0530559549b61d8f7f28). Its source was read at the pinned commit; no upstream application or its dependencies are copied from that project. The following notice is retained for source-derived portions and design attribution.

```text
MIT License

Copyright (c) 2026 Vishesh Raheja

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Other research references: [codex-asr](https://github.com/Wangnov/codex-asr/tree/479f6a7a3db81fe2a23d4755b0ccbeb4400317d4) and the local x-insight-hub loop documentation. Their executables and private data are not included. VoicePort is an independent tool; no affiliation with OpenAI or Codex is implied.

## sherpa-onnx C interface

The unmodified `sherpa-onnx/c-api/c-api.h` from [sherpa-onnx v1.12.37](https://github.com/k2-fsa/sherpa-onnx/blob/v1.12.37/sherpa-onnx/c-api/c-api.h) is retained under `crates/voiceport-macos/src/local_runtime/` with its full Apache-2.0 LICENSE and source provenance. Copyright (c) 2023 Xiaomi Corporation. VoicePort's own `bridge.cc` loads compatible 1.12.37 and 2.0.1 runtimes through this interface, using an explicit zero-default offline CTC/FST tail to match [2.0.1's configuration layout](https://github.com/k2-fsa/sherpa-onnx/blob/af2d8d81dbfa61c4cea4c568da78f3982edca005/sherpa-onnx/c-api/c-api.h). Online/model layouts used here are unchanged. The model library can explicitly download pinned `sherpa-onnx-darwin-arm64@2.0.1` from npm and ONNX int8 models from pinned Hugging Face revisions into VoicePort's user-data directory. No runtime binary or model weight is bundled in the app. Installation retains the full sherpa Apache-2.0 license, ONNX Runtime v1.28.2 MIT license and its ThirdPartyNotices.txt alongside the runtime. Compatible older 1.12.37 VoicePort and Orca installations remain optional read-only runtime sources for the models they support; Unified EN and Nemotron 3.5 require 2.0.1.

The full Apache License 2.0 terms are also present in the shipped bundle at `Contents/Resources/third-party/crates/cfg-if-1.0.5/LICENSE-APACHE`. Downloaded or externally installed runtimes and models retain their own licenses: sherpa-onnx is Apache-2.0; ONNX Runtime is MIT; upstream Whisper code and weights are MIT; the pinned Paraformer conversion model card declares Apache-2.0. SenseVoiceSmall, by FunAudioLLM, uses the custom FunASR Model License v1.1 rather than MIT or Apache-2.0. Its installation retains the complete fixed MODEL_LICENSE.txt and an ATTRIBUTION.txt naming the model, authors and source. Whisper Tiny and Large v3 Turbo installations, including offline imports, retain the complete upstream MIT copyright and permission notice (Copyright (c) 2022 OpenAI) in MODEL_LICENSE.txt, plus an ATTRIBUTION.txt with pinned model and license sources. The fixed upstream license is embedded from `crates/voiceport-macos/src/model_library/WHISPER_LICENSE.txt`. NVIDIA Parakeet TDT v2/v3 weights use CC BY 4.0; Parakeet Unified EN uses the NVIDIA Open Model License Agreement (October 24, 2025); the exact multilingual `nvidia/nemotron-3.5-asr-streaming-0.6b` uses OpenMDW License Agreement v1.1. Their ONNX exports change format/precision, with original-model and conversion revisions identified in the installation's complete MODEL_LICENSE.txt and ATTRIBUTION.txt; Unified EN also retains Notice.txt. Full fixed texts are embedded from `CC_BY_4_LICENSE.txt`, `NVIDIA_OPEN_MODEL_LICENSE.txt`, and `OPENMDW_LICENSE.txt` under `crates/voiceport-macos/src/model_library/`. Conversion repositories and original model authors are identified in the model documentation. See [local model dependencies](docs/LOCAL_MODELS.md).

## Rust dependencies

The Rust workspace has the following direct dependencies, with versions resolved by the committed Cargo.lock. This table describes their use and declared license metadata; it is not a complete inventory of transitive license notices.

| Crate | Locked version | Use | Declared license |
|---|---|---|---|
| reqwest | 0.12.28 | HTTPS and multipart; default features disabled, rustls-tls/multipart enabled | MIT OR Apache-2.0 |
| tokio | 1.54.1 | Runtime, networking, timers and Ctrl-C | MIT |
| tokio-util | 0.7.20 | Cancellation token | MIT |
| serde | 1.0.229 | Typed response decoding | MIT OR Apache-2.0 |
| serde_json | 1.0.154 | Auth and response JSON | MIT OR Apache-2.0 |
| base64 | 0.22.1 | JWT payload decoding | MIT OR Apache-2.0 |
| dirs | 6.0.0 | Platform user directory | MIT OR Apache-2.0 |
| tempfile | 3.27.0 | Isolated fixtures and atomic model installation staging | MIT OR Apache-2.0 |
| ring | 0.17.14 | Model and runtime SHA-256 / SHA-512 integrity | Apache-2.0 AND ISC; bundled notices retain embedded component terms |
| objc2 | 0.6.5 | Native Objective-C runtime bindings | MIT |
| objc2-foundation | 0.3.2 | Foundation types and native paths | MIT |
| objc2-app-kit | 0.3.2 | Native menu, window and input integration | Zlib OR Apache-2.0 OR MIT |
| objc2-av-foundation / objc2-avf-audio | 0.3.2 | Microphone permissions, capture and AAC conversion | Zlib OR Apache-2.0 OR MIT |
| objc2-audio-toolbox / objc2-core-audio | 0.3.2 | Audio unit device selection and Core Audio input device discovery | Zlib OR Apache-2.0 OR MIT |
| objc2-speech | 0.3.2 | On-device speech recognition | Zlib OR Apache-2.0 OR MIT |
| block2 | 0.6.2 | Native framework callback blocks | MIT |
| core-foundation | 0.10.1 | Core Foundation ownership and values | MIT OR Apache-2.0 |
| core-graphics | 0.25.0 | Native input delivery | MIT OR Apache-2.0 |
| libc | 0.2.191 | Descriptor-relative storage and native interfaces | MIT OR Apache-2.0 |
| unicode-segmentation | 1.13.3 | Text delivery boundaries | MIT OR Apache-2.0 |
| uuid | 1.29.0 | Owned recording-session identifiers | Apache-2.0 OR MIT |

Full pinned native-target dependency, feature/scope and license-material inventory is now included in [resources/third-party/manifest.json](resources/third-party/manifest.json). Full applicable license/NOTICE files and supplementary attribution-bearing source files (including ring embedded BoringSSL/fiat/once_cell components) are bundled under Contents/Resources/third-party; MPL source is retained there. Dev/build materials are conservatively included and labelled; excluded-platform lock entries are not labelled actual linked dependencies. See [packaging rules](docs/PACKAGING.md) for choices, source provenance, checks and limitations. This third-party inventory does not change VoicePort's MIT license or the licenses of its dependencies.

VP-013 adds a target-specific `windows-sys` 0.61.2 Win32 binding dependency (MIT OR Apache-2.0) resolved in Cargo.lock; the adapter selects MIT and leaves macOS bindings cfg-excluded. Native Windows runtime packaging and its complete applicable materials remain outstanding; see docs/WINDOWS.md.

The Rust-native macOS regeneration inventories 153 native-target packages and 954 material files. The objc2 family selects MIT only where its published declarations permit that choice. Its crate archives omit the repository licensing declaration and full standard terms, so reviewed supplemental sources are retained under `resources/license-sources/`: the unchanged upstream `LICENSE.md` from each crate's exact `.cargo_vcs_info.json` commit, and full MIT/Apache-2.0/Zlib standard texts from SPDX license-list-data v3.26.0 as applicable. The generated materials retain each crate's original Cargo metadata and commit evidence alongside these texts. Manifest origin fields identify upstream URLs and commits; the checker independently pins the supplemental SHA-256 values and requires the expected complete set. Verification uses no network.

The upstream declaration identifies MIT for objc2, block2, objc2-foundation and objc2-encode, and Zlib/Apache-2.0/MIT alternatives for the other framework crates. It also records the upstream Apple SDK derivation caveat; that declaration is preserved in full. Standard licence templates retain their upstream placeholders unchanged, without inventing copyright holders. This is source/material provenance for a local macOS development package, not Windows delivery acceptance, legal approval or authorization for external distribution.
