# Third-party references

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
| tempfile | 3.27.0 | Isolated test fixtures only | MIT OR Apache-2.0 |

Full pinned native-target dependency, feature/scope and license-material inventory is now included in [resources/third-party/manifest.json](resources/third-party/manifest.json). Full applicable license/NOTICE files and supplementary attribution-bearing source files (including ring embedded BoringSSL/fiat/once_cell components) are bundled under Contents/Resources/third-party; MPL source is retained there. Dev/build materials are conservatively included and labelled; excluded-platform lock entries are not labelled actual linked dependencies. See [packaging rules](docs/PACKAGING.md) for choices, source provenance, checks and limitations. No project license or external distribution approval is asserted.

VP-013 adds a target-specific `windows-sys`0.61.2 Win32 binding dependency (MIT OR Apache-2.0) already resolved in Cargo.lock; the adapter selects MIT and leaves macOS bindings cfg-excluded. The native macOS license generator was rerun unchanged because workspace membership/build-input hashes changed; all687 accepted material files are retained byte-for-byte. This regeneration is a macOS graph check, not Windows binary/license delivery acceptance. Native Windows runtime packaging and its complete applicable materials remain outstanding; see docs/WINDOWS.md.
