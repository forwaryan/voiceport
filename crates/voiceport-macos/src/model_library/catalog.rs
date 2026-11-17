//! Pinned upstream artifacts. See docs/LOCAL_MODELS.md for provenance and licenses.
use super::{Asset, Definition};
use crate::local_models::{
    ModelKind, NEMOTRON, PARAFORMER, PARAKEET_TDT_V2, PARAKEET_TDT_V3, PARAKEET_UNIFIED,
    SENSEVOICE, WHISPER, WHISPER_TURBO,
};

pub const WHISPER_LICENSE: &str = include_str!("WHISPER_LICENSE.txt");
pub const WHISPER_ATTRIBUTION: &str = "Whisper — OpenAI\nOriginal models: https://github.com/openai/whisper\nONNX conversion: csukuangfj / sherpa-onnx\nTiny: https://huggingface.co/csukuangfj/sherpa-onnx-whisper-tiny/tree/65176e2deb88badc814a94058666cadccc29b61c\nLarge v3 Turbo: https://huggingface.co/csukuangfj/sherpa-onnx-whisper-turbo/tree/2ca6ff69fc878651b770880507669577ac41c2ff\nLicense: MIT, Copyright (c) 2022 OpenAI\nLicense source: https://github.com/openai/whisper/blob/86098128c0b4f24f0e2aa2994de830614b474227/LICENSE\n";

pub const SENSEVOICE_LICENSE: &str = include_str!("SENSEVOICE_MODEL_LICENSE.txt");
pub const SENSEVOICE_ATTRIBUTION: &str = "SenseVoiceSmall — Alibaba Group / FunAudioLLM\nOriginal model: https://huggingface.co/FunAudioLLM/SenseVoiceSmall\nONNX conversion: csukuangfj / sherpa-onnx\nhttps://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/tree/2365baeacb507f821a0c8120fcee3d484dba7a07\nLicense: FunASR Model Open Source License Agreement v1.1 (custom terms)\nLicense source: https://github.com/modelscope/FunASR/blob/e227484d98ca90aa829a0ae180731c9ee2e87e77/MODEL_LICENSE\n";

pub const CC_BY_4_LICENSE: &str = include_str!("CC_BY_4_LICENSE.txt");
pub const NVIDIA_OPEN_MODEL_LICENSE: &str = include_str!("NVIDIA_OPEN_MODEL_LICENSE.txt");
pub const OPENMDW_LICENSE: &str = include_str!("OPENMDW_LICENSE.txt");
pub const PARAKEET_V3_ATTRIBUTION: &str = "Parakeet TDT 0.6B v3 — NVIDIA Corporation\nOriginal model: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/tree/541d1f99c6b0c3cd0b11a95167540bb8edefd82b\nONNX int8 conversion: csukuangfj / sherpa-onnx; export and quantization modify the original format/precision.\nhttps://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/tree/ed5d745623472f97cfc2e6c2916ee248798fc5a8\nLicense: CC BY 4.0, https://creativecommons.org/licenses/by/4.0/\nNo warranty or NVIDIA endorsement. See MODEL_LICENSE.txt for the complete license.\n";
pub const PARAKEET_V2_ATTRIBUTION: &str = "Parakeet TDT 0.6B v2 — NVIDIA Corporation\nOriginal model: https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2/tree/ae9ad07059c7c739ffaf932226a8fe64ae2620b0\nONNX int8 conversion: csukuangfj / sherpa-onnx; export and quantization modify the original format/precision.\nhttps://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/tree/84b478d541cdbf0c296a7ab5f1e20a6f8f998114\nLicense: CC BY 4.0, https://creativecommons.org/licenses/by/4.0/\nNo warranty or NVIDIA endorsement. See MODEL_LICENSE.txt for the complete license.\n";
pub const PARAKEET_UNIFIED_ATTRIBUTION: &str = "Parakeet Unified EN 0.6B — NVIDIA Corporation\nLicensed by NVIDIA Corporation under the NVIDIA Open Model License\nOriginal model: https://huggingface.co/nvidia/parakeet-unified-en-0.6b/tree/fe53cd885760c96b6a5f51a0bfd362cb4584a98b\nONNX int8 buffered-streaming conversion (560ms): csukuangfj / sherpa-onnx; exported/quantized from the original model.\nhttps://huggingface.co/csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-560ms/tree/7551fd26fc810cc1e4e043e608db4d13b59be31e\nLicense: NVIDIA Open Model License Agreement, October 24, 2025\nhttps://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/\nModel conversion source: https://github.com/k2-fsa/sherpa-onnx/tree/af2d8d81dbfa61c4cea4c568da78f3982edca005/scripts/nemo/parakeet-unified-en-0.6b\nNo NVIDIA endorsement. See MODEL_LICENSE.txt and Notice.txt.\n";
pub const NEMOTRON_ATTRIBUTION: &str = "Nemotron 3.5 ASR Streaming 0.6B — NVIDIA Corporation\nOriginal multilingual model: https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b/tree/ea30d66debe3740a08b573244286791d423d6b3e\nONNX int8 cache-aware streaming conversion (560ms): csukuangfj / sherpa-onnx; exported/quantized from the original model.\nhttps://huggingface.co/csukuangfj2/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11/tree/ab43d895f5985b1bbab8b6eac8607fcdc05343f3\nLicense: OpenMDW License Agreement version 1.1, https://openmdw.ai/license/1-1/\nModel conversion source: https://github.com/k2-fsa/sherpa-onnx/tree/af2d8d81dbfa61c4cea4c568da78f3982edca005/scripts/nemo/nemotron-3.5-asr-streaming-0.6b\nThe eight adaptation-only locales in the original card require fine-tuning and are not advertised as out-of-box supported languages.\nNo NVIDIA endorsement. See MODEL_LICENSE.txt.\n";

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
        languages: "99 种语言 · 不含粤语",
        description: "下载较小；停止录音后在本机分段转写。",
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
        languages: "100 种语言 · 含粤语",
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
    Definition {
        id: PARAKEET_TDT_V3,
        directory: "parakeet-tdt-v3",
        name: "Parakeet TDT v3",
        languages: "25 种欧洲语言 · 自动识别",
        description: "欧洲语言离线识别，停止后在本机分段转写。",
        recommendation: "多语言离线",
        kind: ModelKind::ParakeetTdtV3,
        assets: &[
            Asset {
                name: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/ed5d745623472f97cfc2e6c2916ee248798fc5a8/encoder.int8.onnx",
                bytes: 652282294,
                sha256: "1afbe69c1ffdabee94755f42e4daf1d93549131a3da11356bda84fcef0f2c479",
            },
            Asset {
                name: "decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/ed5d745623472f97cfc2e6c2916ee248798fc5a8/decoder.int8.onnx",
                bytes: 11845274,
                sha256: "0222a86147299d4dee90edf95b6d13827edb42ad8b7c9cf0b2582f030937c0b5",
            },
            Asset {
                name: "joiner.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/ed5d745623472f97cfc2e6c2916ee248798fc5a8/joiner.int8.onnx",
                bytes: 6355277,
                sha256: "cbd02f3696e9944970111fb439d9c3fd365a80374565f13f2b9c7007d253fc98",
            },
            Asset {
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/ed5d745623472f97cfc2e6c2916ee248798fc5a8/tokens.txt",
                bytes: 93939,
                sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
            },
        ],
    },
    Definition {
        id: PARAKEET_TDT_V2,
        directory: "parakeet-tdt-v2",
        name: "Parakeet TDT v2",
        languages: "English",
        description: "停止后在本机分段识别英语；质量未做同音频比较。",
        recommendation: "英语离线",
        kind: ModelKind::ParakeetTdtV2,
        assets: &[
            Asset {
                name: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/encoder.int8.onnx",
                bytes: 652282309,
                sha256: "92a10bd9c23936ea0566c29f963bc57c2a5df4261b6a8921c39abea66557ca8b",
            },
            Asset {
                name: "decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/decoder.int8.onnx",
                bytes: 7257753,
                sha256: "4d097bae3bac5c96018d5293ba39f1b657296102b142affda40378f38b020f41",
            },
            Asset {
                name: "joiner.int8.onnx",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/joiner.int8.onnx",
                bytes: 1739080,
                sha256: "77a29f3e5ac7d37074e0af465391b80f314015d5b97f0f90c6c7767f1d56f428",
            },
            Asset {
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/84b478d541cdbf0c296a7ab5f1e20a6f8f998114/tokens.txt",
                bytes: 9384,
                sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d",
            },
        ],
    },
    Definition {
        id: PARAKEET_UNIFIED,
        directory: "parakeet-unified-en",
        name: "Parakeet Unified EN",
        languages: "English",
        description: "英语流式预览；固定 560ms 缓冲上下文，CPU 实际延迟取决于设备。",
        recommendation: "英语流式",
        kind: ModelKind::ParakeetUnified,
        assets: &[
            Asset {
                name: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-560ms/resolve/7551fd26fc810cc1e4e043e608db4d13b59be31e/encoder.int8.onnx",
                bytes: 654046389,
                sha256: "e566c3f014598a41724f2df028779a2d4cf7943cbefa324964f6a72e8ee255fb",
            },
            Asset {
                name: "decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-560ms/resolve/7551fd26fc810cc1e4e043e608db4d13b59be31e/decoder.int8.onnx",
                bytes: 7257777,
                sha256: "34fea72425d2506600772ba191a6d3f99c0710abdb68d9a3dc89fa8cb2aa473a",
            },
            Asset {
                name: "joiner.int8.onnx",
                url: "https://huggingface.co/csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-560ms/resolve/7551fd26fc810cc1e4e043e608db4d13b59be31e/joiner.int8.onnx",
                bytes: 1735860,
                sha256: "869f43f7d24595c55581ad3bf249a935fb8a71389fbdaa7504b9f46f93140f8a",
            },
            Asset {
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-560ms/resolve/7551fd26fc810cc1e4e043e608db4d13b59be31e/tokens.txt",
                bytes: 8952,
                sha256: "dc0b4584ab2e4ddbf888425c076c61b736e7356a015250db7d307e6f1a8188ff",
            },
        ],
    },
    Definition {
        id: NEMOTRON,
        directory: "nemotron-3.5",
        name: "Nemotron 3.5",
        languages: "28 种语言 · 包含中文、English、日本語、한국어",
        description: "多语言流式预览；560ms 缓存上下文，CPU 实际延迟取决于设备。",
        recommendation: "多语言流式",
        kind: ModelKind::Nemotron,
        assets: &[
            Asset {
                name: "encoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj2/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11/resolve/ab43d895f5985b1bbab8b6eac8607fcdc05343f3/encoder.int8.onnx",
                bytes: 657601403,
                sha256: "012e9321373af99021415e0b0eb3ec827b4be3153be6f30d9b448fe65e896e68",
            },
            Asset {
                name: "decoder.int8.onnx",
                url: "https://huggingface.co/csukuangfj2/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11/resolve/ab43d895f5985b1bbab8b6eac8607fcdc05343f3/decoder.int8.onnx",
                bytes: 14978075,
                sha256: "19f9c98fc6d0a2c33a65a43b36fdb2e914c26c0aa9764be3aebc502a1e982fb0",
            },
            Asset {
                name: "joiner.int8.onnx",
                url: "https://huggingface.co/csukuangfj2/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11/resolve/ab43d895f5985b1bbab8b6eac8607fcdc05343f3/joiner.int8.onnx",
                bytes: 9504438,
                sha256: "4101c7c679a0bc30483794b27a059e34e79232aa2068d78d51231a22c8b0d7ce",
            },
            Asset {
                name: "tokens.txt",
                url: "https://huggingface.co/csukuangfj2/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11/resolve/ab43d895f5985b1bbab8b6eac8607fcdc05343f3/tokens.txt",
                bytes: 131440,
                sha256: "729cc103155bafa785f9cd45746cd41cabe97eab7182fc04d594129587958f8a",
            },
        ],
    },
];
pub const RUNTIME_ARCHIVE: Asset = Asset {
    name: "sherpa-onnx 2.0.1 · 本地识别引擎",
    url: "https://registry.npmjs.org/sherpa-onnx-darwin-arm64/-/sherpa-onnx-darwin-arm64-2.0.1.tgz",
    bytes: 10013200,
    sha256: "afc9415260e232c4dadc25a29a8645acb0ba06ef989060d32274f4c749a4830e",
};
pub const RUNTIME_INTEGRITY: &str =
    "agXmbbwiVPUSMViMhWhYAt59iG21ui7x6HxQFU35XzHOfQe9x6Pg9V160Pzak/66x0ePMjixsLUtMaaUrLfCwQ==";
pub const RUNTIME_NOTICES: &[Asset] = &[
    Asset {
        name: "LICENSE-ONNX-Runtime.txt",
        url: "https://raw.githubusercontent.com/microsoft/onnxruntime/v1.28.2/LICENSE",
        bytes: 1073,
        sha256: "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
    },
    Asset {
        name: "ThirdPartyNotices-ONNX-Runtime.txt",
        url: "https://raw.githubusercontent.com/microsoft/onnxruntime/v1.28.2/ThirdPartyNotices.txt",
        bytes: 325054,
        sha256: "0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2",
    },
];
pub const RUNTIME_FILES: &[&str] = &[
    "package.json",
    "libsherpa-onnx-c-api.dylib",
    "libonnxruntime.dylib",
];
