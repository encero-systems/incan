//! Snapshot helpers for the source-defined `std.regex` module.
//!
//! `stdlib/regex.incn` imports `regex::Regex` and `regex::RegexBuilder`
//! directly. This file only snapshots borrowed engine matches/captures into
//! owned tuple/list/dict data that generated Incan code can safely store.

use std::collections::HashMap;

/// Owned match data: `(text, start, end)`.
pub type RawMatch = (String, i64, i64);

/// Owned capture data: `(indexed_groups, named_groups)`.
pub type RawCaptures = (Vec<Option<RawMatch>>, HashMap<String, Option<RawMatch>>);

/// Return the first match anywhere in `text`.
#[must_use]
pub fn find(regex: &regex::Regex, text: &str) -> Option<RawMatch> {
    regex.find(text).map(raw_match)
}

/// Return all left-to-right non-overlapping matches in `text`.
#[must_use]
pub fn find_all(regex: &regex::Regex, text: &str) -> Vec<RawMatch> {
    regex.find_iter(text).map(raw_match).collect()
}

/// Return captures for the first match anywhere in `text`.
#[must_use]
pub fn captures(regex: &regex::Regex, text: &str) -> Option<RawCaptures> {
    regex.captures(text).map(|captures| raw_captures(regex, &captures))
}

/// Return all left-to-right non-overlapping capture results in `text`.
#[must_use]
pub fn captures_all(regex: &regex::Regex, text: &str) -> Vec<RawCaptures> {
    regex
        .captures_iter(text)
        .map(|captures| raw_captures(regex, &captures))
        .collect()
}

/// Convert one borrowed engine match into owned data.
fn raw_match(value: regex::Match<'_>) -> RawMatch {
    (
        value.as_str().to_string(),
        i64::try_from(value.start()).unwrap_or(i64::MAX),
        i64::try_from(value.end()).unwrap_or(i64::MAX),
    )
}

/// Convert borrowed engine captures into owned indexed and named groups.
fn raw_captures(regex: &regex::Regex, captures: &regex::Captures<'_>) -> RawCaptures {
    let groups = (0..captures.len())
        .map(|index| captures.get(index).map(raw_match))
        .collect();
    let names = regex
        .capture_names()
        .flatten()
        .map(|name| (name.to_string(), captures.name(name).map(raw_match)))
        .collect();
    (groups, names)
}
