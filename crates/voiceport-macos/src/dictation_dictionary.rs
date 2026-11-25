//! Deterministic local substitutions for final dictation and the settings preview.
use objc2_foundation::NSString;
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};
use unicode_segmentation::UnicodeSegmentation;

const RULE_LIMIT: usize = 256;
const TERM_LIMIT: usize = 128;
const EXPANSION_LIMIT: usize = 512;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct Rule {
    input: String,
    output: String,
}

pub(crate) fn parse_rules(text: &str) -> Result<Vec<Rule>, String> {
    let rules = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let (input, output) = line
                .split_once('→')
                .ok_or_else(|| format!("第 {} 行需要用 → 分隔原词与新文字。", index + 1))?;
            Ok(Rule {
                input: input.trim().into(),
                output: output.trim().into(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Dictionary::compile(&rules)?;
    Ok(rules)
}

pub(crate) fn format_rules(rules: &[Rule]) -> String {
    rules
        .iter()
        .map(|rule| format!("{} → {}", rule.input, rule.output))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Default)]
pub(crate) struct Dictionary(Node);

#[derive(Default)]
struct Node {
    edges: HashMap<String, Node>,
    replacement: Option<String>,
}

fn canonical(grapheme: &str) -> Cow<'_, str> {
    if grapheme.is_ascii() {
        Cow::Borrowed(grapheme)
    } else {
        Cow::Owned(
            NSString::from_str(grapheme)
                .precomposedStringWithCanonicalMapping()
                .to_string(),
        )
    }
}

impl Dictionary {
    pub(crate) fn compile(rules: &[Rule]) -> Result<Self, String> {
        if rules.len() > RULE_LIMIT {
            return Err(format!("词典最多可保存 {RULE_LIMIT} 条规则。"));
        }
        let mut dictionary = Self::default();
        for (index, rule) in rules.iter().enumerate() {
            if rule.input.trim().is_empty()
                || rule.output.trim().is_empty()
                || rule.input.contains('→')
                || rule.input.chars().any(char::is_control)
                || rule.output.chars().any(char::is_control)
                || rule.input.chars().take(TERM_LIMIT + 1).count() > TERM_LIMIT
                || rule.output.chars().take(EXPANSION_LIMIT + 1).count() > EXPANSION_LIMIT
            {
                return Err(format!("第 {} 条规则为空、过长或包含控制字符。", index + 1));
            }
            let mut node = &mut dictionary.0;
            for grapheme in rule.input.graphemes(true) {
                node = node
                    .edges
                    .entry(canonical(grapheme).into_owned())
                    .or_default();
            }
            if node.replacement.is_some() {
                return Err(format!("第 {} 条规则与前面的原词重复。", index + 1));
            }
            node.replacement = Some(rule.output.clone());
        }
        Ok(dictionary)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.edges.is_empty()
    }

    pub(crate) fn apply<'a>(&self, original: &'a str) -> Cow<'a, str> {
        if self.is_empty() || original.is_empty() {
            return Cow::Borrowed(original);
        }
        let boundaries: HashSet<_> = original
            .split_word_bound_indices()
            .map(|(at, _)| at)
            .collect();
        let parts: Vec<_> = original
            .grapheme_indices(true)
            .map(|(offset, grapheme)| (offset, canonical(grapheme), boundaries.contains(&offset)))
            .collect();
        let mut result = String::new();
        let mut copied = 0;
        let mut start = 0;
        while start < parts.len() {
            if !parts[start].2 {
                start += 1;
                continue;
            }
            let mut best = None;
            let mut node = &self.0;
            for (end, (_, grapheme, _)) in parts.iter().enumerate().skip(start) {
                let Some(next) = node.edges.get(grapheme.as_ref()) else {
                    break;
                };
                node = next;
                if let Some(replacement) = &node.replacement
                    && parts.get(end + 1).is_none_or(|part| part.2)
                {
                    best = Some((end + 1, replacement));
                }
            }
            if let Some((end, replacement)) = best {
                result.push_str(&original[copied..parts[start].0]);
                result.push_str(replacement);
                copied = parts.get(end).map_or(original.len(), |part| part.0);
                start = end;
            } else {
                start += 1;
            }
        }
        if copied == 0 {
            return Cow::Borrowed(original);
        }
        result.push_str(&original[copied..]);
        Cow::Owned(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary(rules: &str) -> Dictionary {
        Dictionary::compile(&parse_rules(rules).unwrap()).unwrap()
    }

    #[test]
    fn dictionary_uses_canonical_graphemes_and_preserves_unmatched_bytes() {
        let d = dictionary("café → Café™\n👩🏽‍💻 → 开发\n👍 → 好");
        let original = "cafe\u{301} 👩🏽‍💻 👍🏽 👍 e\u{301}";
        assert_eq!(d.apply(original), "Café™ 开发 👍🏽 好 e\u{301}");
        assert!(matches!(d.apply("☕ e\u{301}"), Cow::Borrowed(_)));
        assert!(parse_rules("cafe\u{301} → first\ncafé → second").is_err());
    }

    #[test]
    fn dictionary_respects_words_and_concatenated_chinese_english() {
        let d = dictionary("app → App\n北经 → 北京\nVoice Port → VoicePort\nλ → lambda");
        assert_eq!(
            d.apply("在北经用app和Voice Port。apple my_app app2 app.example app’s αλ λ (app)!"),
            "在北京用App和VoicePort。apple my_app app2 app.example app’s αλ lambda (App)!"
        );
    }

    #[test]
    fn dictionary_takes_longest_match_and_never_reprocesses_output() {
        let d = dictionary("你好 → 哈喽\n你好呀 → 欢迎\n欢迎 → 再见\n签名 → 谢谢，稍后跟进");
        assert_eq!(d.apply("你好呀！你好！签名"), "欢迎！哈喽！谢谢，稍后跟进");
        let rules = parse_rules("签名 → 谢谢 → 下一步\n\nvp → VoicePort").unwrap();
        assert_eq!(parse_rules(&format_rules(&rules)).unwrap(), rules);
        assert!(parse_rules("→ missing").is_err());
        assert!(parse_rules("empty →").is_err());
    }
}
