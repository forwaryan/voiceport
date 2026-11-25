//! A read-only projection of the existing App status response. The caller owns
//! the expected App identity and the single status query supplying the snapshot.
use crate::{Response, VERSION};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Instance {
    NotRunning,
    Mismatch,
    Matched,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Granted,
    Denied,
    Restricted,
    NotDetermined,
    Unavailable,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Hotkey {
    Registered,
    Conflict,
    RegistrationFailed,
    Suspended,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NextStep {
    LaunchExpectedApp,
    SelectExpectedInstance,
    CheckAppStatus,
    ReviewMicrophonePermission,
    CheckMicrophoneStatus,
    ReviewAccessibilityOrDisableAutomaticInput,
    CheckAutomaticInputSettings,
    ReviewOptionalSpeechPreview,
    ChangeShortcut,
    ReviewShortcutSettings,
    FinishShortcutEditing,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Readiness {
    pub instance: Instance,
    pub microphone: Permission,
    pub speech: Permission,
    pub accessibility: Permission,
    pub hotkey: Hotkey,
    pub next_steps: Vec<NextStep>,
}

/// Match the absolute bundle path from status (no path lookup). When supplied,
/// the expected PID must also match. Foreign or incomplete status snapshots
/// cannot supply grants or registration evidence for the intended instance.
pub fn assess(expected_bundle: &Path, expected_pid: Option<u32>, status: &Response) -> Readiness {
    let instance = instance(expected_bundle, expected_pid, status);
    let mut result = Readiness {
        instance,
        microphone: Permission::Unknown,
        speech: Permission::Unknown,
        accessibility: Permission::Unknown,
        hotkey: Hotkey::Unknown,
        next_steps: Vec::new(),
    };
    if instance != Instance::Matched {
        result.next_steps.push(match instance {
            Instance::NotRunning => NextStep::LaunchExpectedApp,
            Instance::Mismatch => NextStep::SelectExpectedInstance,
            _ => NextStep::CheckAppStatus,
        });
        return result;
    }

    let data = &status.data;
    let permissions = &data["permissions"];
    result.microphone = permission(&permissions["microphone"]);
    result.speech = permission(&permissions["speech"]);
    result.accessibility = match permissions["accessibility"].as_bool() {
        Some(true) => Permission::Granted,
        Some(false) => Permission::Denied,
        None => Permission::Unknown,
    };
    result.hotkey = match data["hotkey"]["registration"].as_str() {
        Some("registered") => Hotkey::Registered,
        Some("conflict") => Hotkey::Conflict,
        Some("registration_failed") => Hotkey::RegistrationFailed,
        Some("suspended") => Hotkey::Suspended,
        _ => Hotkey::Unknown,
    };

    match result.microphone {
        Permission::Granted => {}
        Permission::Unknown | Permission::Unavailable => {
            result.next_steps.push(NextStep::CheckMicrophoneStatus);
        }
        _ => result.next_steps.push(NextStep::ReviewMicrophonePermission),
    }
    match (data["input"]["automatic"].as_bool(), result.accessibility) {
        (Some(false), _) | (Some(true), Permission::Granted) => {}
        (Some(true), Permission::Denied) => result
            .next_steps
            .push(NextStep::ReviewAccessibilityOrDisableAutomaticInput),
        _ => result
            .next_steps
            .push(NextStep::CheckAutomaticInputSettings),
    }
    // ChatGPT is the only source using the optional system Speech preview.
    if data["source"].as_str() == Some("chatgpt")
        && matches!(
            result.speech,
            Permission::Denied | Permission::NotDetermined | Permission::Restricted
        )
    {
        result
            .next_steps
            .push(NextStep::ReviewOptionalSpeechPreview);
    }
    match result.hotkey {
        Hotkey::Registered => {}
        Hotkey::Conflict => result.next_steps.push(NextStep::ChangeShortcut),
        Hotkey::RegistrationFailed | Hotkey::Unknown => {
            result.next_steps.push(NextStep::ReviewShortcutSettings);
        }
        Hotkey::Suspended => result.next_steps.push(NextStep::FinishShortcutEditing),
    }
    result
}

fn instance(expected_bundle: &Path, expected_pid: Option<u32>, status: &Response) -> Instance {
    if status.schema_version != VERSION || !expected_bundle.is_absolute() || expected_pid == Some(0)
    {
        return Instance::Unknown;
    }
    if status.status == "error"
        && status
            .error
            .as_ref()
            .is_some_and(|error| error.code == "app_not_running")
    {
        return Instance::NotRunning;
    }
    if status.status != "ok" || status.error.is_some() {
        return Instance::Unknown;
    }
    let actual_bundle = status.data["bundle_path"].as_str().map(Path::new);
    let actual_pid = status.data["pid"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .filter(|pid| *pid != 0);
    if actual_bundle.is_some_and(|path| path.is_absolute() && path != expected_bundle)
        || matches!((expected_pid, actual_pid), (Some(expected), Some(actual)) if expected != actual)
    {
        Instance::Mismatch
    } else if actual_bundle == Some(expected_bundle) && actual_pid.is_some() {
        Instance::Matched
    } else {
        Instance::Unknown
    }
}

fn permission(value: &Value) -> Permission {
    match value.as_str() {
        Some("authorized") => Permission::Granted,
        Some("denied") => Permission::Denied,
        Some("restricted") => Permission::Restricted,
        Some("notDetermined") => Permission::NotDetermined,
        Some("unavailable") => Permission::Unavailable,
        _ => Permission::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const EXPECTED: &str = "/synthetic/VoicePort.app";

    fn snapshot() -> Response {
        Response::ok(json!({
            "bundle_path": EXPECTED, "pid": 1919, "source": "chatgpt",
            "permissions": {
                "microphone": "notDetermined", "speech": "restricted", "accessibility": false
            },
            "input": {"automatic": true}
        }))
    }

    #[test]
    fn readiness_does_not_trust_unmatched_or_incomplete_identity() {
        let mut missing_pid = snapshot();
        missing_pid.data["pid"] = Value::Null;
        let mut future = snapshot();
        future.schema_version += 1;
        for (path, pid, status, expected, step) in [
            (
                EXPECTED,
                None,
                Response::error("app_not_running", "synthetic"),
                Instance::NotRunning,
                NextStep::LaunchExpectedApp,
            ),
            (
                "/other/VoicePort.app",
                None,
                snapshot(),
                Instance::Mismatch,
                NextStep::SelectExpectedInstance,
            ),
            (
                EXPECTED,
                Some(42),
                snapshot(),
                Instance::Mismatch,
                NextStep::SelectExpectedInstance,
            ),
            (
                EXPECTED,
                Some(1919),
                missing_pid,
                Instance::Unknown,
                NextStep::CheckAppStatus,
            ),
            (
                EXPECTED,
                None,
                Response::error("ipc_unavailable", "synthetic"),
                Instance::Unknown,
                NextStep::CheckAppStatus,
            ),
            (
                EXPECTED,
                None,
                future,
                Instance::Unknown,
                NextStep::CheckAppStatus,
            ),
        ] {
            let result = assess(Path::new(path), pid, &status);
            assert_eq!(result.instance, expected);
            assert_eq!(result.next_steps, [step]);
            assert_eq!(result.microphone, Permission::Unknown);
            assert_eq!(result.speech, Permission::Unknown);
            assert_eq!(result.accessibility, Permission::Unknown);
            assert_eq!(result.hotkey, Hotkey::Unknown);
        }
    }

    #[test]
    fn readiness_projects_permissions_without_requesting_any_grant() {
        let status = snapshot();
        let before = serde_json::to_value(&status).unwrap();
        let result = assess(Path::new(EXPECTED), Some(1919), &status);
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "instance": "matched", "microphone": "not_determined", "speech": "restricted",
                "accessibility": "denied", "hotkey": "unknown", "next_steps": [
                    "review_microphone_permission", "review_accessibility_or_disable_automatic_input",
                    "review_optional_speech_preview", "review_shortcut_settings"
                ]
            })
        );
        assert_eq!(serde_json::to_value(status).unwrap(), before);
    }

    #[test]
    fn readiness_requires_structured_hotkey_evidence_and_respects_input_settings() {
        let mut status = snapshot();
        status.data["permissions"]["microphone"] = json!("authorized");
        status.data["input"]["automatic"] = json!(false);
        status.data["source"] = json!("local.synthetic");
        status.data["hotkey"] = json!({"error": "may be taken"});
        for (registration, hotkey, steps) in [
            (
                Value::Null,
                Hotkey::Unknown,
                vec![NextStep::ReviewShortcutSettings],
            ),
            (
                json!("registration_failed"),
                Hotkey::RegistrationFailed,
                vec![NextStep::ReviewShortcutSettings],
            ),
            (
                json!("conflict"),
                Hotkey::Conflict,
                vec![NextStep::ChangeShortcut],
            ),
            (json!("registered"), Hotkey::Registered, vec![]),
            (
                json!("suspended"),
                Hotkey::Suspended,
                vec![NextStep::FinishShortcutEditing],
            ),
        ] {
            status.data["hotkey"]["registration"] = registration;
            let result = assess(Path::new(EXPECTED), None, &status);
            assert_eq!(result.hotkey, hotkey);
            assert_eq!(result.next_steps, steps);
        }
        status.data["permissions"] = Value::Null;
        status.data["input"] = Value::Null;
        let result = assess(Path::new(EXPECTED), None, &status);
        assert_eq!(result.microphone, Permission::Unknown);
        assert_eq!(result.accessibility, Permission::Unknown);
        assert!(result.next_steps.contains(&NextStep::CheckMicrophoneStatus));
        assert!(
            result
                .next_steps
                .contains(&NextStep::CheckAutomaticInputSettings)
        );
    }
}
