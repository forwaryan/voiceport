use super::*;
use std::{fs, os::unix::fs::symlink};

#[test]
fn csv_preserves_allowed_characters_and_legacy_editor_text() {
    let legacy = "北经 → 北京, \"欢迎\" → 👩🏽‍💻\r\n\r\nvp → VoicePort";
    let rules = parse_rules(legacy).unwrap();
    assert_eq!(parse_editor(legacy).unwrap(), rules);
    assert_eq!(parse_csv(&format_csv(&rules)).unwrap(), rules);

    let csv = "\u{feff}input,output\r\n\" café , \"\"a\"\"\",\" 谢谢 → 👩🏽‍💻\u{2028} \"\r\n";
    let rules = parse_csv(csv).unwrap();
    assert_eq!(rules[0].input, " café , \"a\"");
    assert_eq!(rules[0].output, " 谢谢 → 👩🏽‍💻\u{2028} ");
    assert_eq!(parse_editor(&format_editor(&rules)).unwrap(), rules);
    assert_eq!(parse_csv(&format_csv(&rules)).unwrap(), rules);
    assert_eq!(parse_csv("input,output\nabc,def").unwrap().len(), 1);
    assert!(parse_csv(&format_csv(&[])).unwrap().is_empty());
}

#[test]
fn csv_rejects_malformed_rows_and_disallowed_characters() {
    for row in [
        "word",
        "word,replacement,extra",
        "word,",
        "word,\"unfinished",
        "word,un\"escaped",
        "word,\"quoted\"suffix",
        "word,\"embedded\nnewline\"",
        "\"embedded\r\nnewline\",word",
        "word,\"tab\tinside\"",
        "word,null\0inside",
        "bad→word,result",
        "word,result\r",
        "café,first\ncafe\u{301},duplicate",
    ] {
        let input = format!("{HEADER}\nvalid,rule\n{row}");
        assert!(parse_csv(&input).is_err(), "accepted {input:?}");
    }
    assert!(parse_csv("").is_err());
    assert!(parse_csv("original,replacement\nword,result").is_err());
    for row in [
        format!("{},out", "x".repeat(129)),
        format!("in,{}", "x".repeat(513)),
    ] {
        assert!(parse_csv(&format!("{HEADER}\n{row}")).is_err());
    }
}

#[test]
fn import_merges_canonical_duplicates_in_order_or_replaces_the_list() {
    let current = parse_rules("café → keep\nAPP → uppercase").unwrap();
    let incoming =
        parse_csv("input,output\ncafe\u{301},replace\napp,lowercase\n北经,北京\n").unwrap();
    let plan = ImportPlan::new(&current, incoming.clone(), DictionaryImportMode::Merge).unwrap();
    assert_eq!(plan.imported, 3);
    assert_eq!(plan.skipped_duplicates, 1);
    assert_eq!(&plan.rules[..2], &current);
    assert_eq!(&plan.rules[2..], &incoming[1..]);
    let replaced =
        ImportPlan::new(&current, incoming.clone(), DictionaryImportMode::Replace).unwrap();
    assert_eq!(replaced.rules, incoming);
    assert_eq!(replaced.skipped_duplicates, 0);
    let clear = ImportPlan::new(&current, vec![], DictionaryImportMode::Replace).unwrap();
    assert!(clear.rules.is_empty());
}

#[test]
fn exchange_round_trips_at_rule_limits_and_rejects_over_limit_merges() {
    let rules: Vec<_> = (0..RULE_LIMIT)
        .map(|index| Rule {
            input: format!("{}{:03}", "🦀".repeat(125), index),
            output: "🦀".repeat(512),
        })
        .collect();
    let csv = format_csv(&rules);
    assert!(csv.len() > voiceport_control::MAX_FRAME);
    assert_eq!(parse_csv(&csv).unwrap(), rules);
    let extra = parse_rules("extra → result").unwrap();
    assert!(ImportPlan::new(&rules, extra, DictionaryImportMode::Merge).is_err());
    assert!(parse_csv(&format!("{csv}extra,result\n")).is_err());
}

#[test]
fn file_exchange_is_bounded_private_and_never_clobbers() {
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("dictionary.csv");
    let txt = dir.path().join("legacy.txt");
    fs::write(&txt, "\u{feff}北经 → 北京, \"欢迎\"\r\n").unwrap();
    let rules = read(&txt).unwrap();
    write(&csv, &rules).unwrap();
    assert_eq!(read(&csv).unwrap(), rules);
    assert_eq!(
        fs::metadata(&csv).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let exported = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&csv)
        .unwrap();
    assert!(has_no_extended_acl(exported.as_raw_fd()).unwrap());
    assert_eq!(exported.metadata().unwrap().nlink(), 1);
    assert_eq!(write(&csv, &[]).unwrap_err(), SAVE_ERROR);
    assert_eq!(read(&csv).unwrap(), rules);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    let alias = dir.path().join("alias.csv");
    symlink(&csv, &alias).unwrap();
    assert!(read(&alias).is_err());
    assert!(write(&alias, &[]).is_err());
    assert_eq!(read(&csv).unwrap(), rules);
    assert!(read(dir.path()).is_err());
    fs::write(&txt, [0xff]).unwrap();
    assert!(read(&txt).is_err());
    fs::File::create(&txt)
        .unwrap()
        .set_len(MAX_BYTES as u64 + 1)
        .unwrap();
    assert!(read(&txt).is_err());
}

#[test]
fn export_rejects_inherited_acl_grants_and_denials_before_publication() {
    for (entry, owner_can_open) in [
        ("group:everyone allow read,file_inherit,only_inherit", true),
        ("group:everyone deny read,file_inherit,only_inherit", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let chmod = std::process::Command::new("/bin/chmod")
            .args(["+a", entry])
            .arg(dir.path())
            .output()
            .unwrap();
        assert!(
            chmod.status.success(),
            "ACL fixture failed: {:?}",
            chmod.stderr
        );
        {
            let control = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
            let file = control.as_file();
            file.set_permissions(Permissions::from_mode(0o600)).unwrap();
            assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o600);
            assert!(!has_no_extended_acl(file.as_raw_fd()).unwrap());
            assert_eq!(
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(control.path())
                    .is_ok(),
                owner_can_open,
                "fixture must exercise real access for {entry}"
            );
        }
        let target = dir.path().join("private.csv");
        let rules = parse_rules("synthetic → only owner").unwrap();
        assert_eq!(write(&target, &rules).unwrap_err(), PRIVATE_SAVE_ERROR);
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }
}

#[test]
fn failed_export_clears_the_staged_inode_even_when_unlink_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let mut staged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    let path = staged.path().to_owned();
    let rules = parse_rules("synthetic → staged content").unwrap();
    staged.write_all(format_csv(&rules).as_bytes()).unwrap();
    staged.as_file().sync_all().unwrap();
    assert_eq!(
        parse_csv(&fs::read_to_string(&path).unwrap()).unwrap(),
        rules
    );

    fs::set_permissions(dir.path(), Permissions::from_mode(0o500)).unwrap();
    let unlink = fs::remove_file(&path);
    let cleanup = discard_export(staged, true);
    fs::set_permissions(dir.path(), Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        unlink.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    cleanup.unwrap();
    assert!(fs::read(&path).unwrap().is_empty());
}
