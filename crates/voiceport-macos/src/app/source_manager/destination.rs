//! Audio destination copy based only on saved source data and its descriptor.
use crate::plugins::{Adapter, Manifest, Source, validate_endpoint};
use std::net::IpAddr;

const RETRY: &str = "转写失败且录音可用时，VoicePort 会在本机保留这段录音供手动重试。重试仍使用录音开始时的原来源与配置，可能再次向该目的地交付音频。成功、取消或正常退出时，VoicePort 会尝试清理本机录音。远端服务和外部程序按自己的规则处理音频。VoicePort 无法确认对方是否保留或何时删除。";

pub(super) enum Destination {
    OnDevice,
    ChatGpt,
    LoopbackHttp(String),
    RemoteHttp(String),
    HttpNotConfigured,
    ExternalProgram,
    Unknown,
}

impl Destination {
    pub(super) fn configured(plugin: Option<&Manifest>, source: Option<&Source>) -> Self {
        let Some(plugin) = plugin.filter(|plugin| {
            source.is_none_or(|source| {
                source.plugin_id == plugin.id && source.plugin_version == plugin.version
            })
        }) else {
            return Self::Unknown;
        };
        match plugin.adapter {
            Adapter::Chatgpt => Self::ChatGpt,
            Adapter::Paraformer
            | Adapter::Whisper
            | Adapter::SenseVoice
            | Adapter::Parakeet
            | Adapter::Nemotron => Self::OnDevice,
            Adapter::Jsonl => Self::ExternalProgram,
            Adapter::Http => {
                let url = source
                    .and_then(|source| source.config.get("endpoint"))
                    .and_then(|endpoint| validate_endpoint(endpoint).ok());
                let Some(url) = url else {
                    return Self::HttpNotConfigured;
                };
                let origin = url.origin().ascii_serialization();
                if url.host_str().is_some_and(|host| {
                    host == "localhost"
                        || host
                            .trim_matches(['[', ']'])
                            .parse::<IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                }) {
                    Self::LoopbackHttp(origin)
                } else {
                    Self::RemoteHttp(origin)
                }
            }
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::OnDevice => "本机模型",
            Self::ChatGpt | Self::RemoteHttp(_) => "远端服务",
            Self::LoopbackHttp(_) => "本机地址",
            Self::HttpNotConfigured => "HTTP 地址待确认",
            Self::ExternalProgram => "外部程序",
            Self::Unknown => "音频去向未知",
        }
    }

    pub(super) fn details(&self) -> String {
        let destination = match self {
            Self::OnDevice => "音频在此设备由已安装的模型转写。VoicePort 不会为此来源上传录音。".into(),
            Self::ChatGpt => "停止录音后会将音频发给 ChatGPT 转写服务。可选的系统语音预览在本机处理。".into(),
            Self::LoopbackHttp(origin) => format!(
                "音频将发往配置的本机地址 {origin}。该服务可能另行联网或转发音频。",
            ),
            Self::RemoteHttp(origin) => format!("音频将发往配置的转写服务 {origin}。"),
            Self::HttpNotConfigured => "音频将发往配置的 HTTP 转写地址。目前没有可确认的有效地址，请在来源设置中查看。".into(),
            Self::ExternalProgram => "VoicePort 将本机录音路径交给配置的外部程序（JSONL）。该程序可读取录音，也可能联网或转发音频。".into(),
            Self::Unknown => "插件不可用，无法确认此来源的音频去向。".into(),
        };
        format!("{destination}\n\n{RETRY}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn actual_http_configuration_controls_the_destination_without_contacting_it() {
        let store = crate::plugins::Store::default();
        let plugin = store.plugin(crate::plugins::HTTP).unwrap();
        for (endpoint, label, origin) in [
            (
                "http://127.88.0.1:8080/transcribe?token=synthetic",
                "本机地址",
                "http://127.88.0.1:8080",
            ),
            ("https://[::1]/speech", "本机地址", "https://[::1]"),
            (
                "https://localhost.example.invalid/speech",
                "远端服务",
                "https://localhost.example.invalid",
            ),
            ("http://example.invalid/speech", "HTTP 地址待确认", ""),
            ("", "HTTP 地址待确认", ""),
        ] {
            let source = Source {
                id: "synthetic".into(),
                name: "Synthetic".into(),
                plugin_id: plugin.id.clone(),
                plugin_version: plugin.version.clone(),
                enabled: true,
                config: BTreeMap::from([("endpoint".into(), endpoint.into())]),
                credential_refs: BTreeMap::new(),
            };
            let destination = Destination::configured(Some(&plugin), Some(&source));
            assert_eq!(destination.label(), label);
            let detail = destination.details();
            assert!(detail.contains(origin));
            assert!(!detail.contains("token=synthetic"));
            if label == "本机地址" {
                assert!(detail.contains("可能另行联网或转发"));
            }
        }
    }

    #[test]
    fn each_adapter_explains_its_destination_and_retry_without_running_a_program() {
        let store = crate::plugins::Store::default();
        let mut plugin = store.plugin(crate::plugins::CHATGPT_PLUGIN).unwrap();
        for (adapter, label, message) in [
            (Adapter::Chatgpt, "远端服务", "ChatGPT 转写服务"),
            (Adapter::Paraformer, "本机模型", "不会为此来源上传录音"),
            (Adapter::Whisper, "本机模型", "不会为此来源上传录音"),
            (Adapter::SenseVoice, "本机模型", "不会为此来源上传录音"),
            (Adapter::Parakeet, "本机模型", "不会为此来源上传录音"),
            (Adapter::Nemotron, "本机模型", "不会为此来源上传录音"),
            (Adapter::Jsonl, "外部程序", "可能联网或转发音频"),
        ] {
            plugin.adapter = adapter;
            let destination = Destination::configured(Some(&plugin), None);
            assert_eq!(destination.label(), label);
            let details = destination.details();
            assert!(details.contains(message));
            assert!(details.contains("录音开始时的原来源与配置"));
            assert!(details.contains("远端服务和外部程序按自己的规则"));
            assert!(details.contains("无法确认对方是否保留或何时删除"));
        }
        assert_eq!(Destination::configured(None, None).label(), "音频去向未知");
    }
}
