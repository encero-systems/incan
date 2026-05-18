//! Runtime primitives for the source-defined `std.regex` module.
//!
//! The public Incan API is declared in `stdlib/regex.incn`. This module owns the Rust engine boundary: it converts
//! `regex` crate matches and captures into owned values so generated Incan code never has to carry regex lifetimes.

use std::collections::HashMap;

use regex::RegexBuilder;

/// Pattern compilation failure surfaced by `std.regex`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegexError {
    /// Stable error category.
    pub kind: String,
    /// Human-readable compiler detail from the safe regex engine.
    pub detail: String,
}

impl RegexError {
    /// Return the stable error category.
    #[must_use]
    pub fn kind(&self) -> String {
        self.kind.clone()
    }

    /// Return the human-readable detail string.
    #[must_use]
    pub fn message(&self) -> String {
        self.detail.clone()
    }
}

/// Compiled safe-default regular expression.
#[derive(Clone, Debug)]
pub struct RawRegex {
    inner: regex::Regex,
}

impl RawRegex {
    /// Compile a pattern with the RFC 059 constructor flag set.
    pub fn compile(
        pattern: String,
        ignore_case: bool,
        multiline: bool,
        dotall: bool,
        verbose: bool,
    ) -> Result<Self, RegexError> {
        RegexBuilder::new(&pattern)
            .case_insensitive(ignore_case)
            .multi_line(multiline)
            .dot_matches_new_line(dotall)
            .ignore_whitespace(verbose)
            .build()
            .map(|inner| Self { inner })
            .map_err(|err| RegexError {
                kind: "compile_error".to_string(),
                detail: err.to_string(),
            })
    }

    /// Return whether the pattern matches anywhere in `text`.
    #[must_use]
    pub fn is_match(&self, text: String) -> bool {
        self.inner.is_match(&text)
    }

    /// Return the first match anywhere in `text`.
    #[must_use]
    pub fn find(&self, text: String) -> Option<RawMatch> {
        self.inner.find(&text).map(RawMatch::from_match)
    }

    /// Return a left-to-right non-overlapping match iterator.
    #[must_use]
    pub fn find_iter(&self, text: String) -> RawMatchIterator {
        RawMatchIterator {
            items: self.inner.find_iter(&text).map(RawMatch::from_match).collect(),
            index: 0,
        }
    }

    /// Return captures for the first match anywhere in `text`.
    #[must_use]
    pub fn captures(&self, text: String) -> Option<RawCaptures> {
        self.inner
            .captures(&text)
            .map(|captures| RawCaptures::from_captures(&self.inner, &captures))
    }

    /// Return a left-to-right non-overlapping captures iterator.
    #[must_use]
    pub fn captures_iter(&self, text: String) -> RawCapturesIterator {
        RawCapturesIterator {
            items: self
                .inner
                .captures_iter(&text)
                .map(|captures| RawCaptures::from_captures(&self.inner, &captures))
                .collect(),
            index: 0,
        }
    }

    /// Return captures only when the match spans the full input.
    #[must_use]
    pub fn full_match(&self, text: String) -> Option<RawCaptures> {
        let captures = self.inner.captures(&text)?;
        let full = captures.get(0)?;
        if full.start() == 0 && full.end() == text.len() {
            Some(RawCaptures::from_captures(&self.inner, &captures))
        } else {
            None
        }
    }

    /// Return a left-to-right split iterator over all separators.
    #[must_use]
    pub fn split(&self, text: String) -> RawSplitIterator {
        RawSplitIterator {
            items: self.inner.split(&text).map(ToString::to_string).collect(),
            index: 0,
        }
    }

    /// Return a left-to-right split iterator with at most `limit` fields.
    #[must_use]
    pub fn splitn(&self, text: String, limit: i64) -> RawSplitIterator {
        let normalized = usize::try_from(limit).unwrap_or(0);
        RawSplitIterator {
            items: self.inner.splitn(&text, normalized).map(ToString::to_string).collect(),
            index: 0,
        }
    }

    /// Replace the first match using Rust-style capture interpolation in `replacement`.
    #[must_use]
    pub fn replace(&self, text: String, replacement: String) -> String {
        self.inner.replace(&text, replacement.as_str()).to_string()
    }

    /// Replace every match using Rust-style capture interpolation in `replacement`.
    #[must_use]
    pub fn replace_all(&self, text: String, replacement: String) -> String {
        self.inner.replace_all(&text, replacement.as_str()).to_string()
    }

    /// Replace at most `limit` matches using Rust-style capture interpolation in `replacement`.
    #[must_use]
    pub fn replacen(&self, text: String, limit: i64, replacement: String) -> String {
        let normalized = usize::try_from(limit).unwrap_or(0);
        self.inner.replacen(&text, normalized, replacement.as_str()).to_string()
    }

    /// Replace the first match literally, without capture interpolation.
    #[must_use]
    pub fn replace_literal(&self, text: String, replacement: String) -> String {
        self.inner
            .replace(&text, regex::NoExpand(replacement.as_str()))
            .to_string()
    }

    /// Replace every match literally, without capture interpolation.
    #[must_use]
    pub fn replace_all_literal(&self, text: String, replacement: String) -> String {
        self.inner
            .replace_all(&text, regex::NoExpand(replacement.as_str()))
            .to_string()
    }

    /// Replace at most `limit` matches literally, without capture interpolation.
    #[must_use]
    pub fn replacen_literal(&self, text: String, limit: i64, replacement: String) -> String {
        let normalized = usize::try_from(limit).unwrap_or(0);
        self.inner
            .replacen(&text, normalized, regex::NoExpand(replacement.as_str()))
            .to_string()
    }
}

/// Owned match span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawMatch {
    text: String,
    start: i64,
    end: i64,
}

impl RawMatch {
    /// Convert a borrowed regex match into an owned value.
    fn from_match(value: regex::Match<'_>) -> Self {
        Self {
            text: value.as_str().to_string(),
            start: i64::try_from(value.start()).unwrap_or(i64::MAX),
            end: i64::try_from(value.end()).unwrap_or(i64::MAX),
        }
    }

    /// Return the matched substring.
    #[must_use]
    pub fn as_str(&self) -> String {
        self.text.clone()
    }

    /// Return the start byte offset.
    #[must_use]
    pub fn start(&self) -> i64 {
        self.start
    }

    /// Return the exclusive end byte offset.
    #[must_use]
    pub fn end(&self) -> i64 {
        self.end
    }

    /// Return `(start, end)`.
    #[must_use]
    pub fn span(&self) -> (i64, i64) {
        (self.start, self.end)
    }
}

/// Owned capture result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawCaptures {
    groups: Vec<Option<RawMatch>>,
    names: HashMap<String, Option<RawMatch>>,
}

impl RawCaptures {
    /// Convert borrowed captures into owned capture groups and named-group mapping.
    fn from_captures(regex: &regex::Regex, captures: &regex::Captures<'_>) -> Self {
        let groups = (0..captures.len())
            .map(|index| captures.get(index).map(RawMatch::from_match))
            .collect();
        let names = regex
            .capture_names()
            .flatten()
            .map(|name| (name.to_string(), captures.name(name).map(RawMatch::from_match)))
            .collect();
        Self { groups, names }
    }

    /// Return the full match as group 0.
    #[must_use]
    pub fn full_match(&self) -> Option<RawMatch> {
        self.match_index(0)
    }

    /// Return capture group text by numeric index.
    #[must_use]
    pub fn group_index(&self, index: i64) -> Option<String> {
        self.match_index(index).map(|m| m.as_str())
    }

    /// Return capture group text by name.
    #[must_use]
    pub fn group_name(&self, name: String) -> Option<String> {
        self.names.get(&name).cloned().flatten().map(|m| m.as_str())
    }

    /// Return capture span by numeric index.
    #[must_use]
    pub fn span_index(&self, index: i64) -> Option<(i64, i64)> {
        self.match_index(index).map(|m| m.span())
    }

    /// Return capture span by name.
    #[must_use]
    pub fn span_name(&self, name: String) -> Option<(i64, i64)> {
        self.names.get(&name).cloned().flatten().map(|m| m.span())
    }

    /// Return indexed capture values, preserving unmatched optional groups as `None`.
    #[must_use]
    pub fn groups(&self) -> Vec<Option<String>> {
        self.groups
            .iter()
            .skip(1)
            .map(|group| group.as_ref().map(RawMatch::as_str))
            .collect()
    }

    /// Return named capture values, preserving unmatched optional groups as `None`.
    #[must_use]
    pub fn groupdict(&self) -> HashMap<String, Option<String>> {
        self.names
            .iter()
            .map(|(name, group)| (name.clone(), group.as_ref().map(RawMatch::as_str)))
            .collect()
    }

    /// Return the owned match for a numeric capture group.
    #[must_use]
    pub fn match_index(&self, index: i64) -> Option<RawMatch> {
        let Ok(index) = usize::try_from(index) else {
            return None;
        };
        self.groups.get(index).cloned().flatten()
    }
}

/// Owned iterator over regex matches.
#[derive(Clone, Debug)]
pub struct RawMatchIterator {
    items: Vec<RawMatch>,
    index: usize,
}

impl RawMatchIterator {
    /// Return the next match or `None` at exhaustion.
    pub fn __next__(&mut self) -> Option<RawMatch> {
        let item = self.items.get(self.index).cloned();
        if item.is_some() {
            self.index += 1;
        }
        item
    }
}

/// Owned iterator over capture results.
#[derive(Clone, Debug)]
pub struct RawCapturesIterator {
    items: Vec<RawCaptures>,
    index: usize,
}

impl RawCapturesIterator {
    /// Return the next capture result or `None` at exhaustion.
    pub fn __next__(&mut self) -> Option<RawCaptures> {
        let item = self.items.get(self.index).cloned();
        if item.is_some() {
            self.index += 1;
        }
        item
    }
}

/// Owned iterator over split fields.
#[derive(Clone, Debug)]
pub struct RawSplitIterator {
    items: Vec<String>,
    index: usize,
}

impl RawSplitIterator {
    /// Return the next split field or `None` at exhaustion.
    pub fn __next__(&mut self) -> Option<String> {
        let item = self.items.get(self.index).cloned();
        if item.is_some() {
            self.index += 1;
        }
        item
    }
}
