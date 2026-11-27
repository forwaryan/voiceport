//! Local, deterministic processing of final dictation text.
use crate::dictation_dictionary::{Dictionary, Rule};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct OutputRules {
    pub(crate) trim_edges: bool,
    pub(crate) normalize_newlines: bool,
}

#[derive(Default)]
pub(crate) struct OutputPipeline {
    dictionary: Dictionary,
    rules: OutputRules,
}

pub(crate) struct OutputStages<'a> {
    pub(crate) original: &'a str,
    pub(crate) after_dictionary: Cow<'a, str>,
    formatted: Option<String>,
}

impl OutputStages<'_> {
    pub(crate) fn final_text(&self) -> &str {
        self.formatted.as_deref().unwrap_or(&self.after_dictionary)
    }
}

impl<'a> OutputStages<'a> {
    pub(crate) fn into_final(self) -> Cow<'a, str> {
        self.formatted.map_or(self.after_dictionary, Cow::Owned)
    }
}

impl OutputPipeline {
    pub(crate) fn compile(dictionary: &[Rule], rules: OutputRules) -> Result<Self, String> {
        Ok(Self {
            dictionary: Dictionary::compile(dictionary)?,
            rules,
        })
    }

    pub(crate) fn is_identity(&self) -> bool {
        self.dictionary.is_empty() && self.rules == OutputRules::default()
    }

    pub(crate) fn process<'a>(&self, original: &'a str) -> OutputStages<'a> {
        let after_dictionary = self.dictionary.apply(original);
        let trimmed = if self.rules.trim_edges {
            after_dictionary.trim()
        } else {
            &after_dictionary
        };
        let formatted = if self.rules.normalize_newlines && trimmed.contains('\r') {
            Some(trimmed.replace("\r\n", "\n").replace('\r', "\n"))
        } else if trimmed.len() != after_dictionary.len() {
            Some(trimmed.to_owned())
        } else {
            None
        };
        OutputStages {
            original,
            after_dictionary,
            formatted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictation_dictionary::parse_rules;

    #[test]
    fn dictation_output_preserves_default_bytes_and_applies_rules_independently() {
        let input = "\u{3000}e\u{301}\r\n👩🏽‍💻\r🙂  \n";
        let identity = OutputPipeline::default();
        assert!(identity.is_identity());
        assert!(
            matches!(identity.process(input).into_final(), Cow::Borrowed(text) if text == input)
        );
        for (rules, expected) in [
            (
                OutputRules {
                    trim_edges: true,
                    ..Default::default()
                },
                "e\u{301}\r\n👩🏽‍💻\r🙂",
            ),
            (
                OutputRules {
                    normalize_newlines: true,
                    ..Default::default()
                },
                "\u{3000}e\u{301}\n👩🏽‍💻\n🙂  \n",
            ),
        ] {
            let pipeline = OutputPipeline::compile(&[], rules).unwrap();
            assert!(!pipeline.is_identity());
            let stages = pipeline.process(input);
            assert_eq!(stages.original, input);
            assert_eq!(stages.after_dictionary, input);
            assert_eq!(stages.final_text(), expected);
            assert_eq!(stages.into_final(), expected);
        }
    }

    #[test]
    fn dictation_output_exposes_dictionary_result_before_formatting() {
        let dictionary = parse_rules("vp → VoicePort").unwrap();
        let pipeline = OutputPipeline::compile(
            &dictionary,
            OutputRules {
                trim_edges: true,
                normalize_newlines: true,
            },
        )
        .unwrap();
        let stages = pipeline.process(" vp\r\n🙂 ");
        assert_eq!(stages.original, " vp\r\n🙂 ");
        assert_eq!(stages.after_dictionary, " VoicePort\r\n🙂 ");
        assert_eq!(stages.final_text(), "VoicePort\n🙂");
        assert_eq!(pipeline.process("\r\n \t\u{3000}").final_text(), "");
    }
}
