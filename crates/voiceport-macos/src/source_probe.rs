//! An explicit synthetic sample, never a recorder, input target or saved audio.
use crate::{
    local_models::SessionSource,
    plugin_process::{Custody, Failure},
};
use std::io::Write;
use voiceport_core::{CancellationToken, Error, TranscriptionClient};

pub(crate) fn execute(
    source: SessionSource,
    language: &str,
    cancel: &CancellationToken,
    custody: &Custody,
) -> Result<(), Failure> {
    crate::plugin_runtime::check(cancel)?;
    match source {
        SessionSource::Plugin(plugin) => plugin
            .execute(crate::plugin_runtime::sample(), language, cancel, custody)
            .map(|_| ()),
        SessionSource::Local(model) => {
            crate::local_runtime::probe(&model, language, cancel).map_err(Failure::from)
        }
        SessionSource::ChatGpt => {
            // A private, automatically removed WAV contains only generated silence.
            let mut sample = tempfile::Builder::new()
                .prefix("voiceport-source-test-")
                .suffix(".wav")
                .tempfile()
                .map_err(|_| "无法创建内置测试样例。")?;
            sample
                .write_all(&crate::plugin_runtime::sample())
                .map_err(|_| "无法写入内置测试样例。")?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "无法启动来源测试。")?;
            let result = runtime.block_on(async {
                let auth = voiceport_core::auth::default_path()?;
                let proxy = crate::proxy::https_proxy();
                let client = TranscriptionClient::new_with_proxy(auth, proxy.as_deref())?;
                client
                    .transcribe(
                        sample.path(),
                        (!language.is_empty()).then_some(language),
                        cancel,
                    )
                    .await
            });
            response(result).map_err(|error| Failure::from(error.to_string()))
        }
    }
}

fn response(result: Result<String, Error>) -> Result<(), Error> {
    match result {
        // The fixed endpoint returned HTTP 200 and a valid {text:""} object.
        // Silence is expected; it proves response shape, not speech accuracy.
        Ok(_) | Err(Error::EmptyTranscript) => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jsonl_probe_sends_generated_silence_and_releases_its_owned_process() {
        use crate::plugins::*;
        use std::collections::BTreeMap;
        let script = r#"import base64,json,sys,struct
r=json.loads(sys.stdin.readline())
assert r['plugin_id']=='example.synthetic-probe' and r['language']=='zh'
w=base64.b64decode(r['audio']['base64'],validate=True)
assert len(w)==32044 and w[:4]==b'RIFF'
assert struct.unpack('<I',w[24:28])[0]==16000 and not any(w[44:])
print(json.dumps({'protocol_version':1,'event':'final','text':'discard this synthetic response'}))"#;
        let manifest = Manifest {
            schema_version: 1,
            protocol_version: 1,
            id: "example.synthetic-probe".into(),
            version: "1".into(),
            name: "Synthetic probe".into(),
            origin: "Local controlled transport test".into(),
            adapter: Adapter::Jsonl,
            capabilities: Capabilities {
                batch: true,
                streaming: false,
                system_preview: false,
            },
            fields: vec![],
            program: Some(Program {
                executable: "/usr/bin/python3".into(),
                args: vec!["-I".into(), "-c".into(), script.into()],
            }),
        };
        let mut store = Store::default();
        store.plugins.push(manifest.clone());
        let source = Source {
            id: "synthetic-jsonl-probe".into(),
            name: "Synthetic JSONL".into(),
            plugin_id: manifest.id.clone(),
            plugin_version: manifest.version.clone(),
            enabled: true,
            config: BTreeMap::new(),
            credential_refs: BTreeMap::new(),
        };
        let snapshot = crate::plugin_runtime::Snapshot::configured(&store, &source)
            .unwrap()
            .pin()
            .unwrap();
        let custody = Custody::default();
        assert!(
            execute(
                SessionSource::Plugin(Box::new(snapshot)),
                "zh",
                &CancellationToken::new(),
                &custody
            )
            .is_ok()
        );
        assert!(!custody.unresolved());
    }

    #[test]
    fn only_a_valid_empty_response_counts_as_a_silent_sample_response() {
        assert_eq!(response(Ok("ignored sample output".into())), Ok(()));
        assert_eq!(response(Err(Error::EmptyTranscript)), Ok(()));
        for error in [
            Error::InvalidResponse,
            Error::Service(401),
            Error::Service(403),
            Error::Service(429),
            Error::TimedOut,
            Error::Network,
            Error::Cancelled,
        ] {
            assert_eq!(response(Err(error)), Err(error));
        }
    }
}
