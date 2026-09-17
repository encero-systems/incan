//! Canonical, span-free hashing of Rust token streams.
//!
//! The Rust-side declaration digest answers one question: did anything that can change compiled output move? Token
//! streams are the cheapest representation that answers it honestly. They survive reformatting, they carry no byte
//! offsets, and they retain every construct the compiler will actually see, including macro invocations that this
//! crate cannot expand. This module turns a [`proc_macro2::TokenStream`] into a stable hexadecimal digest.
//!
//! Two things are excluded because they provably cannot reach compiled output. Comments are dropped by the tokenizer
//! before this module ever sees them, so `//` and `/* */` cost nothing. Doc attributes survive tokenization as
//! `#[doc = …]` (that is what `///` and `//!` lex to), so they are dropped here explicitly, at every nesting level.
//!
//! # Why spacing is part of the digest
//!
//! [`proc_macro2::Spacing`] records whether a punctuation token was written flush against the next one, and it is
//! semantically load-bearing: `a && b` is a logical conjunction while `a & &b` is a bitwise `and` against a
//! reference, and the two differ in nothing else. Discarding spacing would hash those equal, which is exactly the
//! under-invalidation this digest exists to prevent, so it is retained.
//!
//! Retaining it does not cost formatting stability, because callers hash the stream `syn` re-emits from a parsed
//! item rather than the stream the lexer produced. That round trip normalizes spacing onto meaning: a double
//! reference emits two `Alone` ampersands whether it was written `&&u8` or `& &u8`, while the `&&` operator emits a
//! `Joint` one. Layout is normalized, meaning is preserved. The exception is a macro invocation's argument tokens,
//! which `syn` stores exactly as lexed, so spacing inside macro soup remains source-faithful and over-invalidates
//! in the same direction as everything else in that soup.
//!
//! # Why macro bodies are absorbed verbatim
//!
//! Outside a macro invocation, a `#` followed by a bracket group is always an attribute, so recognizing doc
//! attributes by shape is exact. Inside macro token soup that guarantee is gone: `#[doc = $d]` may be a matcher
//! fragment or a substitution whose edit really does change what the macro expands to. Macro bodies are therefore
//! hashed exactly as written, doc attributes included. The failure direction is over-invalidation, and detecting a
//! macro invocation by shape can only ever over-trigger (`return !(x)` looks like one), so both halves of the
//! heuristic fail safe.

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use sha2::{Digest, Sha256};

// ============================================================================
// Encoding tags
// ============================================================================
//
// Every token contributes a one-byte kind tag so that a token stream cannot be confused with a differently shaped one
// that happens to concatenate to the same bytes. Variable-length payloads are length-delimited for the same reason.

/// Tag introducing an identifier's text.
const TAG_IDENT: u8 = b'I';
/// Tag introducing a punctuation character.
const TAG_PUNCT: u8 = b'P';
/// Tag introducing a literal's source text.
const TAG_LITERAL: u8 = b'L';
/// Tag closing a delimited group, so nesting depth cannot be forged by a flat token run.
const TAG_GROUP_END: u8 = b'}';

/// Hash a token stream into this crate's canonical digest, dropping doc attributes at every nesting level.
///
/// The returned string is lowercase hexadecimal SHA-256. It depends only on the token sequence, never on where those
/// tokens sat in a file.
pub(crate) fn digest_tokens(tokens: TokenStream) -> String {
    let mut hasher = Sha256::new();
    absorb_stripped(&mut hasher, tokens);
    hex::encode(hasher.finalize())
}

/// Absorb a token stream with doc attributes removed and macro bodies left verbatim.
///
/// The stream is materialized into a slice because recognizing an attribute needs lookahead across two or three
/// token trees, which an iterator alone cannot offer without buffering anyway.
fn absorb_stripped(hasher: &mut Sha256, tokens: TokenStream) {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    let mut index = 0;
    while index < trees.len() {
        // ---- Doc attributes contribute nothing ----
        if let Some(resume) = doc_attribute_end(&trees, index) {
            index = resume;
            continue;
        }

        // ---- Macro invocations are hashed exactly as written ----
        if let Some(resume) = macro_invocation_end(&trees, index) {
            for tree in trees.iter().take(resume).skip(index) {
                absorb_verbatim(hasher, tree);
            }
            index = resume;
            continue;
        }

        // ---- Ordinary tokens, recursing into groups so nested attributes are reached ----
        match &trees[index] {
            TokenTree::Group(group) => {
                absorb_delimiter(hasher, group.delimiter());
                absorb_stripped(hasher, group.stream());
                hasher.update([TAG_GROUP_END]);
            }
            leaf => absorb_verbatim(hasher, leaf),
        }
        index += 1;
    }
}

/// Absorb one token tree exactly as written, including any doc attributes nested inside it.
///
/// This is the encoding that defines the digest. It is deliberately structural rather than textual: it never calls
/// `TokenStream::to_string`, so no rendering decision of an upstream crate can silently change what a digest means,
/// and it never touches [`proc_macro2::Span`], so an item's position in its file cannot reach the hash.
fn absorb_verbatim(hasher: &mut Sha256, tree: &TokenTree) {
    match tree {
        TokenTree::Group(group) => {
            absorb_delimiter(hasher, group.delimiter());
            for inner in group.stream() {
                absorb_verbatim(hasher, &inner);
            }
            hasher.update([TAG_GROUP_END]);
        }
        TokenTree::Ident(ident) => absorb_field(hasher, TAG_IDENT, ident.to_string().as_bytes()),
        TokenTree::Punct(punct) => {
            absorb_field(hasher, TAG_PUNCT, punct.as_char().to_string().as_bytes());
            hasher.update([spacing_tag(punct.spacing())]);
        }
        TokenTree::Literal(literal) => absorb_field(hasher, TAG_LITERAL, literal.to_string().as_bytes()),
    }
}

/// Absorb one length-delimited, tagged field so adjacent payloads cannot be mistaken for one another.
///
/// The length is written as decimal digits followed by `:` rather than as fixed-width native-endian bytes, so a digest
/// computed on a 32-bit host matches one computed on a 64-bit host.
///
/// This is the crate's single delimiting primitive, shared with [`crate::mir_digest`]. A second implementation would
/// be a second chance to get the guard wrong, and a digest whose delimiting is wrong fails silently: it maps two
/// different inputs onto one key, which is the direction that ships a stale artifact. Tag spaces are per-digest; the
/// encoding is not.
pub(crate) fn absorb_field(hasher: &mut Sha256, tag: u8, bytes: &[u8]) {
    hasher.update([tag]);
    hasher.update(bytes.len().to_string().as_bytes());
    hasher.update(b":");
    hasher.update(bytes);
}

/// Absorb a group's opening delimiter.
///
/// An invisible (`Delimiter::None`) group is real grouping produced by macro expansion and changes how the tokens
/// parse, so it gets its own tag rather than being treated as absent.
fn absorb_delimiter(hasher: &mut Sha256, delimiter: Delimiter) {
    let tag = match delimiter {
        Delimiter::Parenthesis => b'(',
        Delimiter::Brace => b'{',
        Delimiter::Bracket => b'[',
        Delimiter::None => b'N',
    };
    hasher.update([tag]);
}

/// Map punctuation spacing onto its encoding tag.
fn spacing_tag(spacing: Spacing) -> u8 {
    match spacing {
        Spacing::Joint => b'J',
        Spacing::Alone => b'A',
    }
}

/// Report where a doc attribute starting at `index` ends, or `None` when no doc attribute starts there.
///
/// Both the outer form (`#[doc = "…"]`, which `///` lexes to) and the inner form (`#![doc = "…"]`, which `//!` lexes
/// to) are recognized, as is the call form `#[doc(hidden)]`. Every `doc` attribute is rustdoc-only: none of them can
/// change compiled output, so all of them are excluded rather than only the comment-derived spellings.
fn doc_attribute_end(trees: &[TokenTree], index: usize) -> Option<usize> {
    let TokenTree::Punct(pound) = trees.get(index)? else {
        return None;
    };
    if pound.as_char() != '#' {
        return None;
    }
    let mut cursor = index + 1;
    if let Some(TokenTree::Punct(bang)) = trees.get(cursor)
        && bang.as_char() == '!'
    {
        cursor += 1;
    }
    let TokenTree::Group(group) = trees.get(cursor)? else {
        return None;
    };
    if group.delimiter() != Delimiter::Bracket {
        return None;
    }
    let Some(TokenTree::Ident(name)) = group.stream().into_iter().next() else {
        return None;
    };
    if name != "doc" {
        return None;
    }
    Some(cursor + 1)
}

/// Report where a macro invocation starting at `index` ends, or `None` when none starts there.
///
/// Two shapes count: `name ! <group>` for an ordinary invocation, and `macro_rules ! name <group>` for a definition,
/// whose body is token soup for the same reason. The check is intentionally shape-based and therefore approximate;
/// because a false positive only suppresses doc-stripping inside the group, it costs an extra invalidation and never
/// a missed one.
fn macro_invocation_end(trees: &[TokenTree], index: usize) -> Option<usize> {
    let TokenTree::Ident(name) = trees.get(index)? else {
        return None;
    };
    let TokenTree::Punct(bang) = trees.get(index + 1)? else {
        return None;
    };
    if bang.as_char() != '!' {
        return None;
    }
    let body = if name == "macro_rules" {
        if !matches!(trees.get(index + 2), Some(TokenTree::Ident(_))) {
            return None;
        }
        index + 3
    } else {
        index + 2
    };
    match trees.get(body) {
        Some(TokenTree::Group(_)) => Some(body + 1),
        _ => None,
    }
}
