//! Presentation metadata for the visible GUI result. No transcript or audio is stored here.
use super::{Catalog, SessionSource};
use crate::plugins::{Adapter, validate_endpoint};
use std::net::IpAddr;
use voiceport_control::{GuiDictationDiagnostics, GuiDictationReceipt};

#[derive(Default)]
pub(crate) struct GuiResultContext {
    pub(crate) source: Option<String>,
    pub(crate) target: Option<String>,
}

impl GuiResultContext {
    pub(super) fn at_start(
        source: &SessionSource,
        language: &str,
        catalog: &Catalog,
        source_id: &str,
    ) -> Self {
        let descriptor = source.descriptor();
        let title = catalog
            .entries
            .iter()
            .find(|entry| entry.id == source_id)
            .map_or(descriptor.name.as_str(), |entry| entry.title.as_str());
        let language = voiceport_core::language::LANGUAGES
            .iter()
            .find(|(code, _)| *code == language)
            .map_or(language, |(_, name)| *name);
        let destination = match source {
            SessionSource::Local(_) => "在本机处理",
            SessionSource::ChatGpt => "远端转写服务",
            SessionSource::Plugin(plugin) => match descriptor.adapter {
                Adapter::Http => {
                    let endpoint = plugin
                        .source
                        .config
                        .get("endpoint")
                        .and_then(|value| validate_endpoint(value).ok());
                    match endpoint.as_ref().and_then(|url| url.host_str()) {
                        Some(host)
                            if host == "localhost"
                                || host
                                    .trim_matches(['[', ']'])
                                    .parse::<IpAddr>()
                                    .is_ok_and(|ip| ip.is_loopback()) =>
                        {
                            "本机 HTTP 服务（可能转发）"
                        }
                        Some(_) => "远端 HTTP 服务",
                        None => "HTTP 服务（地址未确认）",
                    }
                }
                Adapter::Jsonl => "外部程序（可能转发）",
                Adapter::Chatgpt => "远端转写服务",
                _ => "在本机处理",
            },
        };
        Self {
            source: Some(format!("{title} · {language} · {destination}")),
            target: None,
        }
    }
}

/// Kept only while a newer start leaves the previous final text visible.
pub(crate) struct RetainedGuiResult {
    pub(crate) receipt: GuiDictationReceipt,
    pub(crate) diagnostics: GuiDictationDiagnostics,
    pub(crate) context: GuiResultContext,
}
