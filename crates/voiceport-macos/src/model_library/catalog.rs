//! Pinned upstream artifacts. See docs/LOCAL_MODELS.md for provenance and licenses.
use super::{Asset, Definition};
use crate::local_models::{ModelKind, PARAFORMER, SENSEVOICE, WHISPER, WHISPER_TURBO};

pub const WHISPER_LICENSE: &str = include_str!("WHISPER_LICENSE.txt");
pub const WHISPER_ATTRIBUTION: &str = "Whisper — OpenAI\nOriginal models: https://github.com/openai/whisper\nONNX conversion: csukuangfj / sherpa-onnx\nTiny: https://huggingface.co/csukuangfj/sherpa-onnx-whisper-tiny/tree/65176e2deb88badc814a94058666cadccc29b61c\nLarge v3 Turbo: https://huggingface.co/csukuangfj/sherpa-onnx-whisper-turbo/tree/2ca6ff69fc878651b770880507669577ac41c2ff\nLicense: MIT, Copyright (c) 2022 OpenAI\nLicense source: https://github.com/openai/whisper/blob/86098128c0b4f24f0e2aa2994de830614b474227/LICENSE\n";

pub const SENSEVOICE_LICENSE: &str = include_str!("SENSEVOICE_MODEL_LICENSE.txt");
pub const SENSEVOICE_ATTRIBUTION: &str = "SenseVoiceSmall — Alibaba Group / FunAudioLLM\nOriginal model: https://huggingface.co/FunAudioLLM/SenseVoiceSmall\nONNX conversion: csukuangfj / sherpa-onnx\nhttps://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/tree/2365baeacb507f821a0c8120fcee3d484dba7a07\nLicense: FunASR Model Open Source License Agreement v1.1 (custom terms)\nLicense source: https://github.com/modelscope/FunASR/blob/e227484d98ca90aa829a0ae180731c9ee2e87e77/MODEL_LICENSE\n";

pub static MODELS: &[Definition] = &[
    Definition {
        id: PARAFORMER,
        directory: "paraformer-bilingual-zh-en",
        name: "Paraformer · 中英双语",
        languages: "中文、English",
        description: "边说边显示文字，适合中英双语日常输入。",
        recommendation: "实时输入",
        kind: ModelKind::Paraformer,
        assets: &[
            Asset {
                name: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en/resolve/8e40c43232a1c5c66c82111efc5820d3accca11b/encoder.int8.onnx",
                bytes: 165462184,
                sha256: "81a70226a8934e6ed92aa1d4fc486b428b5398e2f2619ed4897b7294cab90e9a",
            },
            Asset {
                name: "decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en/resolve/8e40c43232a1c5c66c82111efc5820d3accca11b/decoder.int8.onnx",
                bytes: 71664561,
                sha256: "f3cca9f77bb9d93c8fcbfb63ae617b6b1ee96818df3aa3b151c40658fe38594f",
            },
            Asset {
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en/resolve/8e40c43232a1c5c66c82111efc5820d3accca11b/tokens.txt",
                bytes: 75756,
                sha256: "59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6",
            },
        ],
    },
    Definition {
        id: WHISPER,
        directory: "whisper-tiny",
        name: "Whisper Tiny",
        languages: "多语言 · 支持中文、English",
        description: "下载较小；停止录音后转写，准确度低于大模型。",
        recommendation: "轻量体验",
        kind: ModelKind::Whisper,
        assets: &[
            Asset {
                name: "tiny-encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-tiny/resolve/65176e2deb88badc814a94058666cadccc29b61c/tiny-encoder.int8.onnx",
                bytes: 12937772,
                sha256: "d24fb083ae3b1041fc24e97971d60e280c9342201fbb67b0ab428a8b4a51a434",
            },
            Asset {
                name: "tiny-decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-tiny/resolve/65176e2deb88badc814a94058666cadccc29b61c/tiny-decoder.int8.onnx",
                bytes: 89855401,
                sha256: "d2fece8dd42771f1df975c6c0445770d0c292bf7547c2cae04a6c0cc57540925",
            },
            Asset {
                name: "tiny-tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-tiny/resolve/65176e2deb88badc814a94058666cadccc29b61c/tiny-tokens.txt",
                bytes: 816730,
                sha256: "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126",
            },
        ],
    },
    Definition {
        id: SENSEVOICE,
        directory: "sensevoice",
        name: "SenseVoice",
        languages: "中文、English、日本語、한국어、粤语",
        description: "停止后本机转写，自动识别五种语言；含 FunASR 自定义模型许可。",
        recommendation: "中文日常推荐",
        kind: ModelKind::SenseVoice,
        assets: &[
            Asset {
                name: "model.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/model.int8.onnx",
                bytes: 239233841,
                sha256: "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51",
            },
            Asset {
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/tokens.txt",
                bytes: 315894,
                sha256: "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc",
            },
        ],
    },
    Definition {
        id: WHISPER_TURBO,
        directory: "whisper-large-v3-turbo",
        name: "Whisper Large v3 Turbo",
        languages: "多语言 · 支持中文、English",
        description: "停止后本机分段转写；下载约 1 GB，载入及识别需要更多内存。",
        recommendation: "多语言进阶",
        kind: ModelKind::WhisperTurbo,
        assets: &[
            Asset {
                name: "turbo-encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-turbo/resolve/2ca6ff69fc878651b770880507669577ac41c2ff/turbo-encoder.int8.onnx",
                bytes: 674716297,
                sha256: "b02dcdf54f348741e93fe732b67d933c8dcb6735655f710640143081db38878b",
            },
            Asset {
                name: "turbo-decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-turbo/resolve/2ca6ff69fc878651b770880507669577ac41c2ff/turbo-decoder.int8.onnx",
                bytes: 361080764,
                sha256: "20accd02388482eb3a46bd615631adfdc85e1eb2c7db9ea3f02a40ffe6b81547",
            },
            Asset {
                name: "turbo-tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-turbo/resolve/2ca6ff69fc878651b770880507669577ac41c2ff/turbo-tokens.txt",
                bytes: 816730,
                sha256: "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126",
            },
        ],
    },
];
pub const RUNTIME_ARCHIVE: Asset = Asset {
    name: "sherpa-onnx 1.12.37 · 本地识别引擎",
    url: "https://registry.npmjs.org/sherpa-onnx-darwin-arm64/-/sherpa-onnx-darwin-arm64-1.12.37.tgz",
    bytes: 22072791,
    sha256: "1ae67a3faa9abbd15e6b1096bee87c120f848f1e1d3f778cb80216886cab217b",
};
pub const RUNTIME_INTEGRITY: &str =
    "zpqbH+2TI6dvg7mxGm30Mnv17aJL3ZfRGshMiBK85dHBfhzqMbKUHNXCzsHhiKBQTzti6JqG1YoRbYrJwvdUjA==";
pub const RUNTIME_NOTICES: &[Asset] = &[
    Asset {
        name: "LICENSE-ONNX-Runtime.txt",
        url: "https://raw.githubusercontent.com/microsoft/onnxruntime/v1.23.2/LICENSE",
        bytes: 1073,
        sha256: "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
    },
    Asset {
        name: "ThirdPartyNotices-ONNX-Runtime.txt",
        url: "https://raw.githubusercontent.com/microsoft/onnxruntime/v1.23.2/ThirdPartyNotices.txt",
        bytes: 326866,
        sha256: "e9e90971a8e75a9a8ac0c6412e29c1202d079998389915aa485f46c816c3b4cc",
    },
];
pub const RUNTIME_FILES: &[&str] = &[
    "package.json",
    "libsherpa-onnx-c-api.dylib",
    "libonnxruntime.1.23.2.dylib",
];
