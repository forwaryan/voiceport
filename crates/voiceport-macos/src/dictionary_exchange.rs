//! Local exchange for the post-transcription dictionary.
use crate::{
    app_update::has_no_acl_grants,
    dictation_dictionary::{Dictionary, RULE_LIMIT, Rule, canonical, format_rules, parse_rules},
};
use std::{
    collections::HashSet,
    fs::{OpenOptions, Permissions},
    io::{Read, Write},
    iter::Peekable,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::Path,
    str::Chars,
};
use unicode_segmentation::UnicodeSegmentation;
use voiceport_control::DictionaryImportMode;

const HEADER: &str = "input,output";
const MAX_BYTES: usize = 1024 * 1024;
const FILE_ERROR: &str = "请选择可读取的 UTF-8 CSV 或 TXT 普通文件。";
const SAVE_ERROR: &str = "无法导出词典。请选择一个新的 .csv 文件名。";
const PRIVATE_SAVE_ERROR: &str = "无法确认导出文件仅当前用户可读写。请选择没有额外访问授权的目录。";

pub(crate) struct ImportPlan {
    pub(crate) rules: Vec<Rule>,
    pub(crate) imported: usize,
    pub(crate) skipped_duplicates: usize,
}

impl ImportPlan {
    pub(crate) fn new(
        current: &[Rule],
        incoming: Vec<Rule>,
        mode: DictionaryImportMode,
    ) -> Result<Self, String> {
        let imported = incoming.len();
        let mut skipped_duplicates = 0;
        let rules = match mode {
            DictionaryImportMode::Replace => incoming,
            DictionaryImportMode::Merge => {
                // Both lists already passed compilation. Use the same grapheme
                // keys as the matching trie to identify overlaps across lists.
                let key = |rule: &Rule| {
                    rule.input
                        .graphemes(true)
                        .map(|part| canonical(part).into_owned())
                        .collect::<Vec<_>>()
                };
                let seen: HashSet<_> = current.iter().map(key).collect();
                let mut merged = current.to_vec();
                for rule in incoming {
                    if seen.contains(&key(&rule)) {
                        skipped_duplicates += 1;
                    } else {
                        merged.push(rule);
                    }
                }
                merged
            }
        };
        Dictionary::compile(&rules)?;
        Ok(Self {
            rules,
            imported,
            skipped_duplicates,
        })
    }
}

fn extension(path: &Path) -> Option<&str> {
    path.extension().and_then(|value| value.to_str())
}

pub(crate) fn read(path: &Path) -> Result<Vec<Rule>, String> {
    let format = extension(path).map(str::to_ascii_lowercase);
    let Some(format @ ("csv" | "txt")) = format.as_deref().filter(|_| path.is_absolute()) else {
        return Err(FILE_ERROR.into());
    };
    let before = path.symlink_metadata().map_err(|_| FILE_ERROR)?;
    if !before.is_file() {
        return Err(FILE_ERROR.into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| FILE_ERROR)?;
    let opened = file.metadata().map_err(|_| FILE_ERROR)?;
    let fingerprint = |metadata: &std::fs::Metadata| {
        (
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    if !opened.is_file() || fingerprint(&before) != fingerprint(&opened) {
        return Err(FILE_ERROR.into());
    }
    let oversized = "词典文件最多可读取 1 MiB。";
    if opened.len() > MAX_BYTES as u64 {
        return Err(oversized.into());
    }
    let mut text = String::new();
    (&mut file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| FILE_ERROR)?;
    if text.len() > MAX_BYTES {
        return Err(oversized.into());
    }
    if text.len() as u64 != opened.len()
        || fingerprint(&file.metadata().map_err(|_| FILE_ERROR)?) != fingerprint(&opened)
    {
        return Err("词典文件在读取时发生变化，请重新选择。".into());
    }
    if format == "csv" {
        parse_csv(&text)
    } else {
        parse_rules(without_bom(&text))
    }
}

pub(crate) fn write(path: &Path, rules: &[Rule]) -> Result<(), String> {
    if !path.is_absolute() || !extension(path).is_some_and(|ext| ext.eq_ignore_ascii_case("csv")) {
        return Err(SAVE_ERROR.into());
    }
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().ok_or(SAVE_ERROR)?)
        .map_err(|_| SAVE_ERROR)?;
    temporary
        .as_file()
        .set_permissions(Permissions::from_mode(0o600))
        .map_err(|_| PRIVATE_SAVE_ERROR)?;
    // Creation can inherit an ACL that grants access despite mode 0600.
    // Refuse before placing any rules in such a file.
    verify_private_export(&temporary)?;
    temporary
        .write_all(format_csv(rules).as_bytes())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|_| SAVE_ERROR)?;
    verify_private_export(&temporary)?;
    // Publication only renames or links this inode in the same directory.
    temporary.persist_noclobber(path).map_err(|_| SAVE_ERROR)?;
    Ok(())
}

fn verify_private_export(temporary: &tempfile::NamedTempFile) -> Result<(), &'static str> {
    let file = temporary.as_file();
    let open = file.metadata().map_err(|_| PRIVATE_SAVE_ERROR)?;
    let named = temporary
        .path()
        .symlink_metadata()
        .map_err(|_| PRIVATE_SAVE_ERROR)?;
    if !named.is_file()
        || (open.dev(), open.ino()) != (named.dev(), named.ino())
        || open.nlink() != 1
        || open.uid() != unsafe { libc::geteuid() }
        || open.mode() & 0o7777 != 0o600
        || !matches!(has_no_acl_grants(file.as_raw_fd()), Ok(true))
    {
        return Err(PRIVATE_SAVE_ERROR);
    }
    Ok(())
}

fn without_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

pub(crate) fn parse_editor(text: &str) -> Result<Vec<Rule>, String> {
    if without_bom(text).lines().next() == Some(HEADER) {
        parse_csv(text)
    } else {
        parse_rules(text)
    }
}

pub(crate) fn format_editor(rules: &[Rule]) -> String {
    // The legacy editor trims both fields. Use CSV if that would change a rule.
    if rules
        .iter()
        .any(|rule| rule.input != rule.input.trim() || rule.output != rule.output.trim())
    {
        format_csv(rules)
    } else {
        format_rules(rules)
    }
}

pub(crate) fn format_csv(rules: &[Rule]) -> String {
    let mut csv = format!("{HEADER}\n");
    for rule in rules {
        for (field, separator) in [(&rule.input, ','), (&rule.output, '\n')] {
            csv.push('"');
            for ch in field.chars() {
                if ch == '"' {
                    csv.push('"');
                }
                csv.push(ch);
            }
            csv.push('"');
            csv.push(separator);
        }
    }
    csv
}

fn csv_field(chars: &mut Peekable<Chars<'_>>) -> Option<String> {
    let mut field = String::new();
    if chars.next_if_eq(&'"').is_some() {
        loop {
            let ch = chars.next()?;
            if ch == '"' && chars.next_if_eq(&'"').is_none() {
                break;
            }
            field.push(ch);
        }
    } else {
        while let Some(ch) = chars.next_if(|ch| !matches!(ch, ',' | '\n' | '\r' | '"')) {
            field.push(ch);
        }
    }
    Some(field)
}

fn csv_record(chars: &mut Peekable<Chars<'_>>) -> Option<(String, String)> {
    let input = csv_field(chars)?;
    chars.next_if_eq(&',')?;
    let output = csv_field(chars)?;
    match chars.next() {
        None | Some('\n') => Some((input, output)),
        Some('\r') if chars.next_if_eq(&'\n').is_some() => Some((input, output)),
        _ => None,
    }
}

fn parse_csv(text: &str) -> Result<Vec<Rule>, String> {
    if text.len() > MAX_BYTES {
        return Err("词典 CSV 最多可读取 1 MiB。".into());
    }
    let mut chars = without_bom(text).chars().peekable();
    if !matches!(csv_record(&mut chars), Some((input, output)) if input == "input" && output == "output")
    {
        return Err(format!("词典 CSV 的标题应为 {HEADER}。"));
    }
    let mut rules = Vec::new();
    while chars.peek().is_some() {
        if rules.len() == RULE_LIMIT {
            return Err(format!("词典最多可保存 {RULE_LIMIT} 条规则。"));
        }
        let (input, output) = csv_record(&mut chars).ok_or_else(|| {
            format!(
                "CSV 第 {} 条规则须有两列，引号使用双引号转义。",
                rules.len() + 1
            )
        })?;
        rules.push(Rule { input, output });
    }
    Dictionary::compile(&rules)?;
    Ok(rules)
}

#[cfg(test)]
mod tests;
