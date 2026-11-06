//! Token stream and identifier classification, dotted path introducers, and f-string parts.

use super::*;

// ============================================================================
// LAYER 1 — TOKEN STREAM
// ============================================================================

/// Classify a lexed token stream, refined by AST regions where they exist.
///
/// This layer alone is what a document being actively typed gets: `regions` is empty whenever the file does not
/// parse, and every rule below still applies. Structural context comes from neighboring tokens (`Ident` after
/// `def`, `Ident` before `(`) rather than from a name's spelling, so nothing here guesses from `PascalCase`.
pub(super) fn classify_token_stream(tokens: &[Token], regions: &[Option<RegionKind>]) -> Vec<ClassifiedRange> {
    let mut ranges = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let (start, end) = (token.span.start, token.span.end);
        match &token.kind {
            TokenKind::Keyword(id) => {
                let category = match keywords::category(*id) {
                    KeywordCategory::Operator => Category::Operator,
                    _ => Category::Keyword,
                };
                ranges.push(ClassifiedRange::plain(start, end, category));
            }
            TokenKind::Operator(_) | TokenKind::Ellipsis => {
                ranges.push(ClassifiedRange::plain(start, end, Category::Operator));
            }
            TokenKind::Int(_) | TokenKind::Float(_) | TokenKind::Decimal(_) => {
                ranges.push(ClassifiedRange::plain(start, end, Category::Number));
            }
            TokenKind::String(_) | TokenKind::Bytes(_) => {
                ranges.push(ClassifiedRange::plain(start, end, Category::String));
            }
            TokenKind::FString(parts) => {
                ranges.extend(classify_fstring(parts, start, end, regions));
            }
            TokenKind::Ident(_) => {
                ranges.push(classify_identifier(tokens, index, start, end, regions));
            }
            // Punctuation carries no semantic information an editor's grammar does not already color correctly,
            // and emitting it would roughly double the response for no visible gain. Indentation and EOF are
            // synthetic: `Indent`/`Dedent` spans are zero-width markers, not source the reader sees.
            TokenKind::Punctuation(_) | TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof => {
            }
        }
    }
    ranges
}

/// Classify one identifier token from its AST region, then its neighbors, then a default of `variable`.
///
/// AST regions win because they are derived from what the parser actually built. The neighbor rules below are the
/// fallback for slots the AST records no span for, and they are also the only classification a file that fails to
/// parse ever gets.
pub(super) fn classify_identifier(
    tokens: &[Token],
    index: usize,
    start: usize,
    end: usize,
    regions: &[Option<RegionKind>],
) -> ClassifiedRange {
    if let Some(kind) = regions.get(start).copied().flatten() {
        let modifiers = match kind {
            RegionKind::TypeParameter | RegionKind::Variable => Modifier::Declaration.mask(),
            _ => 0,
        };
        return ClassifiedRange {
            start,
            end,
            category: kind.category(),
            modifiers,
        };
    }

    // ---- Declaration names: the identifier introduced by a declaring keyword ----
    if let Some(TokenKind::Keyword(id)) = previous_kind(tokens, index) {
        use incan_lang::lang::keywords::KeywordId;
        let declared = match id {
            KeywordId::Def => Some(Category::Function),
            KeywordId::Class => Some(Category::Class),
            KeywordId::Model => Some(Category::Struct),
            KeywordId::Trait | KeywordId::Capability => Some(Category::Interface),
            KeywordId::Enum => Some(Category::Enum),
            KeywordId::Type | KeywordId::Newtype => Some(Category::Type),
            KeywordId::Const | KeywordId::Static => Some(Category::Variable),
            KeywordId::Let | KeywordId::Mut => Some(Category::Variable),
            _ => None,
        };
        if let Some(category) = declared {
            let modifiers = match id {
                KeywordId::Const | KeywordId::Static => Modifier::Declaration.mask() | Modifier::Readonly.mask(),
                _ => Modifier::Declaration.mask(),
            };
            return ClassifiedRange {
                start,
                end,
                category,
                modifiers,
            };
        }
    }

    // ---- Dotted paths take the meaning of what introduced them ----
    // `@a.b.c` is decorator syntax throughout, and `from std.runtime import host` names modules throughout. Both
    // read as a field access on a local if only the token immediately to the left is consulted.
    match dotted_path_introducer(tokens, index) {
        Some(PathIntroducer::Decorator) => {
            return ClassifiedRange::plain(start, end, Category::Decorator);
        }
        Some(PathIntroducer::Module) => {
            return ClassifiedRange::plain(start, end, Category::Namespace);
        }
        None => {}
    }

    // ---- Member access: `recv.name`, a method when called and a property otherwise ----
    if matches!(
        previous_kind(tokens, index),
        Some(TokenKind::Punctuation(
            incan_lang::lang::punctuation::PunctuationId::Dot
        ))
    ) {
        let category = if next_is_call(tokens, index) {
            Category::Method
        } else {
            Category::Property
        };
        return ClassifiedRange::plain(start, end, category);
    }

    // ---- Module paths: `std::hash`, `rust::serde` ----
    if matches!(
        next_kind(tokens, index),
        Some(TokenKind::Punctuation(
            incan_lang::lang::punctuation::PunctuationId::ColonColon
        ))
    ) {
        return ClassifiedRange::plain(start, end, Category::Namespace);
    }

    // ---- Ordinary calls: `name(...)` ----
    if next_is_call(tokens, index) {
        return ClassifiedRange::plain(start, end, Category::Function);
    }

    ClassifiedRange::plain(start, end, Category::Variable)
}

/// Kind of the token before `index`, skipping nothing: adjacency is the point of these rules.
pub(super) fn previous_kind(tokens: &[Token], index: usize) -> Option<&TokenKind> {
    index
        .checked_sub(1)
        .and_then(|previous| tokens.get(previous))
        .map(|token| &token.kind)
}

/// Kind of the token after `index`.
pub(super) fn next_kind(tokens: &[Token], index: usize) -> Option<&TokenKind> {
    tokens.get(index + 1).map(|token| &token.kind)
}

/// Whether the identifier at `index` is immediately followed by an opening parenthesis.
pub(super) fn next_is_call(tokens: &[Token], index: usize) -> bool {
    matches!(
        next_kind(tokens, index),
        Some(TokenKind::Punctuation(
            incan_lang::lang::punctuation::PunctuationId::LParen
        ))
    )
}

/// What introduced the dotted path an identifier belongs to, when anything did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PathIntroducer {
    /// An `@`, so the whole path is decorator syntax.
    Decorator,
    /// An `import` or `from`, so the whole path names modules.
    Module,
}

/// Walk back through a dotted path from the identifier at `index` and report what introduced it.
///
/// Only the token immediately to the left is visible to the ordinary member-access rule, which is enough for
/// `value.field` and wrong for anything reached through a path: in `@policy.audit` the `audit` segment is decorator
/// syntax, and in `from std.runtime import host` the `runtime` segment names a module. Both would otherwise read as
/// a field access on a local named `policy` or `std`.
pub(super) fn dotted_path_introducer(tokens: &[Token], index: usize) -> Option<PathIntroducer> {
    use incan_lang::lang::keywords::KeywordId;
    use incan_lang::lang::punctuation::PunctuationId;

    let mut cursor = index;
    loop {
        match previous_kind(tokens, cursor)? {
            TokenKind::Punctuation(PunctuationId::At) => return Some(PathIntroducer::Decorator),
            TokenKind::Keyword(KeywordId::Import | KeywordId::From) => {
                return Some(PathIntroducer::Module);
            }
            TokenKind::Punctuation(PunctuationId::Dot) => {
                // Step over the `.ident` to the left and keep looking for whatever started the path.
                let previous = cursor.checked_sub(2)?;
                if !matches!(tokens.get(previous).map(|token| &token.kind), Some(TokenKind::Ident(_))) {
                    return None;
                }
                cursor = previous;
            }
            _ => return None,
        }
    }
}

/// Split an f-string into its literal runs and its interpolated expressions.
///
/// The literal parts are string content; each `{expr}` hole is ordinary Incan and is re-lexed so the names inside it
/// are classified like any other expression. Without this an f-string — which is how most Incan code formats output
/// — would be one flat string blob.
pub(super) fn classify_fstring(
    parts: &[FStringPart],
    start: usize,
    end: usize,
    regions: &[Option<RegionKind>],
) -> Vec<ClassifiedRange> {
    let mut ranges = vec![ClassifiedRange::plain(start, end, Category::String)];
    for part in parts {
        let FStringPart::Expr { text, offset } = part else {
            continue;
        };
        // `offset` is the opening brace; the expression's own bytes start just after it.
        let inner_start = offset + 1;
        let (inner_tokens, _) = lexer::lex_tolerant(text);
        for range in classify_token_stream(&inner_tokens, regions) {
            let shifted_start = inner_start + range.start;
            let shifted_end = inner_start + range.end;
            if shifted_end <= end {
                ranges.push(ClassifiedRange {
                    start: shifted_start,
                    end: shifted_end,
                    ..range
                });
            }
        }
    }
    ranges
}
