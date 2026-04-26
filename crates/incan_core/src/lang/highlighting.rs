//! Syntax-highlighting helpers derived from stable language registries.
//!
//! This module keeps editor-facing keyword buckets keyed by stable [`KeywordId`] values (instead of raw strings). That
//! lets generators reuse the same canonical vocabulary metadata as the compiler, formatter, and reference-doc tooling.

use std::collections::HashMap;

use super::keywords::{self, KeywordId};

/// A VS Code/TextMate regex bucket keyed by its scope name.
#[derive(Debug, Clone, Copy)]
pub struct VscodeKeywordBucket {
    /// TextMate scope name used in `incan.tmLanguage.json`.
    pub pattern_name: &'static str,
    /// Stable keyword ids that belong to this scope.
    pub keyword_ids: &'static [KeywordId],
}

const FLOW_KEYWORDS: &[KeywordId] = &[
    KeywordId::Assert,
    KeywordId::If,
    KeywordId::Else,
    KeywordId::Elif,
    KeywordId::Loop,
    KeywordId::While,
    KeywordId::For,
    KeywordId::Match,
    KeywordId::Case,
    KeywordId::Return,
    KeywordId::Yield,
    KeywordId::Break,
    KeywordId::Continue,
    KeywordId::Pass,
];

const CONST_KEYWORDS: &[KeywordId] = &[KeywordId::Const, KeywordId::Static];
const ASYNC_KEYWORDS: &[KeywordId] = &[KeywordId::Async, KeywordId::Await];
const DECLARATION_KEYWORDS: &[KeywordId] = &[
    KeywordId::Def,
    KeywordId::Class,
    KeywordId::Model,
    KeywordId::Trait,
    KeywordId::Enum,
    KeywordId::Newtype,
    KeywordId::Type,
];
const IMPORT_KEYWORDS: &[KeywordId] = &[
    KeywordId::Import,
    KeywordId::From,
    KeywordId::As,
    KeywordId::Super,
    KeywordId::Crate,
    KeywordId::Rust,
    KeywordId::Python,
];
const EXTENDS_KEYWORDS: &[KeywordId] = &[KeywordId::Extends, KeywordId::With];
const VISIBILITY_KEYWORDS: &[KeywordId] = &[KeywordId::Pub];
const STORAGE_KEYWORDS: &[KeywordId] = &[KeywordId::Let, KeywordId::Mut];
const LOGICAL_OPERATOR_KEYWORDS: &[KeywordId] = &[
    KeywordId::And,
    KeywordId::In,
    KeywordId::Is,
    KeywordId::Not,
    KeywordId::Or,
];
const TRUE_KEYWORDS: &[KeywordId] = &[KeywordId::True];
const FALSE_KEYWORDS: &[KeywordId] = &[KeywordId::False];
const NONE_KEYWORDS: &[KeywordId] = &[KeywordId::None];
const SELF_KEYWORDS: &[KeywordId] = &[KeywordId::SelfKw];

/// Registry-backed regex buckets for VS Code/TextMate grammar generation.
pub const VSCODE_KEYWORD_BUCKETS: &[VscodeKeywordBucket] = &[
    VscodeKeywordBucket {
        pattern_name: "keyword.control.flow.incan",
        keyword_ids: FLOW_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "keyword.control.const.incan",
        keyword_ids: CONST_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "keyword.control.async.incan",
        keyword_ids: ASYNC_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "keyword.declaration.incan",
        keyword_ids: DECLARATION_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "keyword.control.import.incan",
        keyword_ids: IMPORT_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "keyword.declaration.extends.incan",
        keyword_ids: EXTENDS_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "keyword.other.visibility.incan",
        keyword_ids: VISIBILITY_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "storage.modifier.incan",
        keyword_ids: STORAGE_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "keyword.operator.logical.incan",
        keyword_ids: LOGICAL_OPERATOR_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "constant.language.boolean.true.incan",
        keyword_ids: TRUE_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "constant.language.boolean.false.incan",
        keyword_ids: FALSE_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "constant.language.none.incan",
        keyword_ids: NONE_KEYWORDS,
    },
    VscodeKeywordBucket {
        pattern_name: "variable.language.self.incan",
        keyword_ids: SELF_KEYWORDS,
    },
];

/// Build the regex used for a VS Code/TextMate bucket.
pub fn vscode_regex_for_bucket(bucket: &VscodeKeywordBucket) -> String {
    regex_for_keyword_ids(bucket.keyword_ids)
}

/// Build the regex for a specific set of keyword ids.
pub fn regex_for_keyword_ids(keyword_ids: &[KeywordId]) -> String {
    let spellings = spellings_for_keyword_ids(keyword_ids);
    format!(r"\b({})\b", spellings.join("|"))
}

/// Return all canonical + alias spellings for a set of keyword ids.
pub fn spellings_for_keyword_ids(keyword_ids: &[KeywordId]) -> Vec<&'static str> {
    let mut spellings = Vec::new();
    for keyword_id in keyword_ids {
        spellings.push(keywords::as_str(*keyword_id));
        spellings.extend_from_slice(keywords::aliases(*keyword_id));
    }
    spellings.sort_unstable();
    spellings.dedup();
    spellings
}

/// Return every VS Code grammar regex bucket as `pattern name -> regex`.
pub fn vscode_pattern_regexes() -> HashMap<&'static str, String> {
    VSCODE_KEYWORD_BUCKETS
        .iter()
        .map(|bucket| (bucket.pattern_name, vscode_regex_for_bucket(bucket)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn vscode_buckets_cover_every_keyword_exactly_once() {
        let mut bucket_counts: HashMap<KeywordId, usize> = HashMap::new();
        for bucket in VSCODE_KEYWORD_BUCKETS {
            for keyword_id in bucket.keyword_ids {
                *bucket_counts.entry(*keyword_id).or_default() += 1;
            }
        }

        for keyword in keywords::KEYWORDS {
            assert_eq!(
                bucket_counts.get(&keyword.id).copied(),
                Some(1),
                "keyword {:?} should appear in exactly one VS Code bucket",
                keyword.id
            );
        }
    }

    #[test]
    fn vscode_declaration_bucket_includes_aliases() {
        let declaration_bucket = VSCODE_KEYWORD_BUCKETS
            .iter()
            .find(|bucket| bucket.pattern_name == "keyword.declaration.incan");
        assert!(declaration_bucket.is_some());
        let spellings = declaration_bucket
            .map(|bucket| spellings_for_keyword_ids(bucket.keyword_ids))
            .unwrap_or_default();
        assert!(spellings.contains(&"def"));
        assert!(spellings.contains(&"fn"));
    }

    #[test]
    fn vscode_literal_buckets_preserve_case_variants() {
        let true_regex = vscode_pattern_regexes().remove("constant.language.boolean.true.incan");
        let false_regex = vscode_pattern_regexes().remove("constant.language.boolean.false.incan");
        assert_eq!(true_regex, Some(r"\b(True|true)\b".to_string()));
        assert_eq!(false_regex, Some(r"\b(False|false)\b".to_string()));
    }

    #[test]
    fn vscode_assert_keyword_uses_flow_scope() {
        let regexes = vscode_pattern_regexes();
        let async_regex = regexes.get("keyword.control.async.incan");
        let flow_regex = regexes.get("keyword.control.flow.incan");

        assert!(async_regex.is_some_and(|regex| !regex.contains("assert")));
        assert!(flow_regex.is_some_and(|regex| regex.contains("assert")));
    }
}
