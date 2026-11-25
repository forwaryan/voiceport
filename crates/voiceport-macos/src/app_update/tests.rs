use super::*;
use serde_json::{Value, json};

fn latest(tag: &str, asset: &str) -> Value {
    json!({
        "tag_name": tag, "draft": false, "prerelease": false,
        "assets": [
            {"id": 123, "name": asset, "digest": format!("sha256:{}", "ff".repeat(32)), "size": 1024, "state": "uploaded"},
            {"id": 124, "name": "SHA256SUMS.txt", "digest": null, "size": 183, "state": "uploaded"}
        ]
    })
}

fn select(metadata: &Value, current: &str) -> Result<Check> {
    release_update(
        metadata.to_string().as_bytes(),
        Version::parse(current).unwrap(),
        github::Source::Public,
    )
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn fixture_release_check(current: &str) -> Result<Check> {
    select(
        &latest("v0.3.14", "VoicePort-v0.3.14-macos-arm64.zip"),
        current,
    )
}

#[test]
#[cfg(target_arch = "aarch64")]
fn compares_the_bundle_to_the_actual_github_tag() {
    let published = latest("v0.3.14", "VoicePort-v0.3.14-macos-arm64.zip");
    assert!(
        matches!(select(&published, "0.3.13"), Ok(Check::Available(update)) if update.version == "0.3.14")
    );
    assert!(
        matches!(select(&published, "0.3.14"), Ok(Check::Current { latest, local_newer: false }) if latest == "0.3.14")
    );
    assert!(
        matches!(select(&published, "0.3.100"), Ok(Check::Current { latest, local_newer: true }) if latest == "0.3.14")
    );
    let next = latest("v0.3.15", "VoicePort-macos-arm64.zip");
    assert!(
        matches!(select(&next, "0.3.14"), Ok(Check::Available(update)) if update.version == "0.3.15")
    );
}

#[test]
fn invalid_or_unstable_responses_are_query_failures() {
    for key in ["draft", "prerelease"] {
        let mut release = latest("v0.3.14", "VoicePort-v0.3.14-macos-arm64.zip");
        release[key] = json!(true);
        assert!(select(&release, "0.3.13").is_err(), "{key}");
    }
    for tag in [
        "0.3.14",
        "v0.3.14-rc1",
        "v0.3.014",
        "../app",
        "v0.3.+14",
        "v0.3.4294967296",
    ] {
        assert!(
            select(&latest(tag, "VoicePort-macos-arm64.zip"), "0.3.13").is_err(),
            "{tag}"
        );
    }
    assert!(release_update(b"not json", Version([0, 3, 13]), github::Source::Public).is_err());
}

#[test]
#[cfg(target_arch = "x86_64")]
fn arm64_release_does_not_offer_installation_on_intel() {
    let published = latest("v0.3.14", "VoicePort-v0.3.14-macos-arm64.zip");
    assert!(
        matches!(select(&published, "0.3.13"), Ok(Check::Incompatible { latest, reason }) if latest == "0.3.14" && reason.contains("Apple 芯片"))
    );
}

#[test]
#[cfg(target_arch = "aarch64")]
fn incompatible_release_keeps_the_verified_tag_and_never_offers_installation() {
    for (key, value) in [
        ("digest", json!(null)),
        ("name", json!("VoicePort-v0.3.14-macos-x64.zip")),
        ("state", json!("new")),
        ("digest", json!("sha256:xyz")),
        ("size", json!(MAX_ZIP + 1)),
        ("id", json!(0)),
    ] {
        let mut release = latest("v0.3.14", "VoicePort-v0.3.14-macos-arm64.zip");
        release["assets"][0][key] = value;
        for current in ["0.3.13", "0.3.14"] {
            assert!(
                matches!(select(&release, current), Ok(Check::Incompatible { latest, .. }) if latest == "0.3.14"),
                "{key}, {current}"
            );
        }
    }
    let mut release = latest("v0.3.14", "VoicePort-v0.3.14-macos-arm64.zip");
    let mut duplicate = release["assets"][0].clone();
    duplicate["name"] = json!("VoicePort-macos-arm64.zip");
    release["assets"].as_array_mut().unwrap().push(duplicate);
    assert!(matches!(
        select(&release, "0.3.13"),
        Ok(Check::Incompatible { .. })
    ));
}

#[test]
fn checks_exact_bytes_and_rejects_overlong_command_output() {
    let bytes = b"test artifact";
    let update = Update {
        version: "0.3.14".into(),
        tag: "v0.3.14".into(),
        asset_name: "VoicePort-v0.3.14-macos-arm64.zip".into(),
        id: 3,
        digest: digest::digest(&digest::SHA256, bytes)
            .as_ref()
            .try_into()
            .unwrap(),
        bytes: bytes.len() as u64,
        source: github::Source::Public,
    };
    assert!(verify_download(bytes, &update).is_ok());
    assert!(verify_download(b"Test artifact", &update).is_err());
    assert!(verify_download(b"test", &update).is_err());
    assert!(
        bounded_output(
            Command::new("/usr/bin/printf").arg("1234"),
            3,
            Duration::from_secs(1)
        )
        .is_err()
    );
}
