//! Semantic token classification for `textDocument/semanticTokens/full` (RFC 081, #1400).
//!
//! ## Why this exists
//!
//! Without a `semanticTokens` provider an editor falls back to whatever regex grammar it happens to ship, which
//! cannot tell a type from a local, a soft keyword from an ordinary identifier, or DSL-owned bytes from Incan. RFC
//! 081 makes that last distinction a stated obligation: *"Tooling must make ownership visible enough that readers
//! can tell where ordinary Incan ends and the DSL-owned surface begins."* Highlighting is the surface a reader
//! actually looks at, so this module is where that obligation is met.
//!
//! ## Why the whole language, and not just fragments
//!
//! The protocol admits no partial answer. A server publishes a legend — a fixed ordered list of token types — and
//! replies with a flat integer array covering *every* token in the file. There is no way to classify one region and
//! leave the rest alone, so embedded fragments are necessarily one category among many rather than the feature
//! itself.
//!
//! ## Three layers, in increasing authority
//!
//! 1. **Token stream** ([`classify_token_stream`]). The lexer's `Token { kind, span }` output is the base layer, and it
//!    is the only layer that survives a file that does not parse — the ordinary state of a document being typed.
//!    Keyword coloring is registry-driven through [`keywords::category`], never a local word list.
//! 2. **AST regions** ([`collect_regions`]). Names in this AST are `Ident = String` and carry no span of their own, but
//!    the slots that *hold* types, parameters and decorators are `Spanned`. So the AST contributes byte *regions* —
//!    "this range is a type position" — and each identifier token takes the category of the smallest region containing
//!    it. This is precise where a spelling heuristic (`PascalCase` means type) would only guess.
//! 3. **Embedded fragments** ([`fragment_ranges`]). Inside a claimed fragment the token stream is explicitly
//!    unreliable: `Lexer::tokenize_tolerant` documents that a submode's raw content is re-tokenized by the fragment's
//!    own grammar and leaves gaps in the whole-file pass. So a fragment's bytes are dropped from the token layer
//!    entirely and re-derived from the typed node tree, with holes recursing back into ordinary Incan.
//!
//! Later layers overwrite earlier ones over the same bytes; the result is sorted, de-overlapped, split at line
//! boundaries, and delta-encoded by [`encode`].

use incan_lang::lang::keywords::{self, KeywordCategory};
use incan_syntax::ast::{Declaration, EmbeddedNode, Expr, Param, Program, Spanned, Statement, Type};
use incan_syntax::lexer::{self, FStringPart, Token, TokenKind};
use tower_lsp::lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

use incan_frontend::ast_walk::any_expr_in_program;

mod fragments_and_comments;
mod regions;
mod token_classification;

use fragments_and_comments::*;
use regions::*;
use token_classification::*;

// ============================================================================
// LEGEND
// ============================================================================

/// Token types this server publishes, in legend order.
///
/// Every entry is a *standard* LSP token type. Custom types are deliberately avoided: an editor that does not know
/// a custom name renders it with no color at all, which would make DSL-owned regions less visible rather than more
/// — the opposite of what RFC 081 asks for.
pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,
    SemanticTokenType::OPERATOR,
    SemanticTokenType::STRING,
    SemanticTokenType::NUMBER,
    SemanticTokenType::COMMENT,
    SemanticTokenType::DECORATOR,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::METHOD,
    SemanticTokenType::CLASS,
    SemanticTokenType::STRUCT,
    SemanticTokenType::ENUM,
    SemanticTokenType::INTERFACE,
    SemanticTokenType::TYPE,
    SemanticTokenType::TYPE_PARAMETER,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::ENUM_MEMBER,
    SemanticTokenType::REGEXP,
    SemanticTokenType::MACRO,
];

/// Token modifiers this server publishes, in legend order.
pub const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,
    SemanticTokenModifier::DEFINITION,
    SemanticTokenModifier::READONLY,
    SemanticTokenModifier::DEFAULT_LIBRARY,
];

/// The legend advertised in `ServerCapabilities`, and the index basis for every encoded token.
///
/// The client resolves a token's `tokenType` field by indexing into this list, so its order is part of the wire
/// contract: appending is safe, reordering silently recolors every document.
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: TOKEN_MODIFIERS.to_vec(),
    }
}

/// A classification, named rather than written as a bare legend index at each call site.
///
/// Discriminants are the legend indices, so [`Category::index`] is the identity mapping and the two lists cannot
/// drift apart without the compiler noticing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Keyword = 0,
    Operator = 1,
    String = 2,
    Number = 3,
    Comment = 4,
    Decorator = 5,
    Function = 6,
    Method = 7,
    Class = 8,
    Struct = 9,
    Enum = 10,
    Interface = 11,
    Type = 12,
    TypeParameter = 13,
    Parameter = 14,
    Property = 15,
    Variable = 16,
    Namespace = 17,
    EnumMember = 18,
    Regexp = 19,
    Macro = 20,
}

impl Category {
    /// Index of this category in [`TOKEN_TYPES`].
    pub fn index(self) -> u32 {
        self as u32
    }
}

/// Bit position of a modifier in [`TOKEN_MODIFIERS`], as the protocol's modifier bitset expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Declaration = 0,
    Definition = 1,
    Readonly = 2,
    DefaultLibrary = 3,
}

impl Modifier {
    /// This modifier as a one-bit mask, ready to be `|`-combined into a token's modifier bitset.
    pub fn mask(self) -> u32 {
        1 << (self as u32)
    }
}

// ============================================================================
// CLASSIFIED RANGES
// ============================================================================

/// One classified byte range, before line splitting and delta encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassifiedRange {
    /// Byte offset of the first classified byte.
    pub start: usize,
    /// Byte offset one past the last classified byte.
    pub end: usize,
    /// What the range is.
    pub category: Category,
    /// Combined [`Modifier`] masks.
    pub modifiers: u32,
}

impl ClassifiedRange {
    /// Build a range carrying no modifiers.
    fn plain(start: usize, end: usize, category: Category) -> Self {
        Self {
            start,
            end,
            category,
            modifiers: 0,
        }
    }
}

// ============================================================================
// LAYER 2 — AST REGIONS
// ============================================================================

/// What an AST-derived byte region means for the identifiers inside it.
///
/// Regions exist because names in this AST are `Ident = String` and carry no span, while the slots holding them are
/// `Spanned`. A region says "identifiers in this range are types" without needing a span on the name itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionKind {
    /// A type position: annotations, return types, bounds, type arguments.
    Type,
    /// A type parameter's own declaration.
    TypeParameter,
    /// A decorator, including its path and arguments.
    Decorator,
    /// A parameter list entry, outside its own type annotation.
    Parameter,
    /// A local binding's declared name.
    Variable,
}

impl RegionKind {
    /// The category an identifier inside this region takes.
    fn category(self) -> Category {
        match self {
            RegionKind::Type => Category::Type,
            RegionKind::TypeParameter => Category::TypeParameter,
            RegionKind::Decorator => Category::Decorator,
            RegionKind::Parameter => Category::Parameter,
            RegionKind::Variable => Category::Variable,
        }
    }
}

/// One AST-derived byte region.
#[derive(Debug, Clone, Copy)]
struct Region {
    start: usize,
    end: usize,
    kind: RegionKind,
}

impl Region {
    /// Byte width, used to prefer the most specific region covering an offset.
    fn width(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

/// Accumulator for the AST region walk.
#[derive(Default)]
struct RegionCollector {
    regions: Vec<Region>,
}

impl RegionCollector {
    /// Record one region.
    fn push(&mut self, span: incan_syntax::ast::Span, kind: RegionKind) {
        if span.end > span.start {
            self.regions.push(Region {
                start: span.start,
                end: span.end,
                kind,
            });
        }
    }

    /// Record a type slot and its nested type arguments.
    ///
    /// The outer span already covers the arguments, so nesting adds nothing here; the single outer region is what
    /// every identifier in `List[Dict[str, User]]` needs.
    fn ty(&mut self, ty: &Spanned<Type>) {
        self.push(ty.span, RegionKind::Type);
    }

    /// Record a parameter: the entry itself, then its annotation as the more specific region inside it.
    fn param(&mut self, param: &Spanned<Param>) {
        self.push(param.span, RegionKind::Parameter);
        self.ty(&param.node.ty);
    }
}

// ============================================================================
// COMPOSITION
// ============================================================================

/// One byte's classification while the layers are being composed.
type Paint = (Category, u32);

/// Paint ranges onto the byte map, later ranges overwriting earlier ones.
fn paint(map: &mut [Option<Paint>], ranges: &[ClassifiedRange]) {
    let limit = map.len();
    for range in ranges {
        for slot in map.iter_mut().take(range.end.min(limit)).skip(range.start) {
            *slot = Some((range.category, range.modifiers));
        }
    }
}

/// Compose every layer into the final, non-overlapping classification of a document.
///
/// The layers are composed over a byte map rather than merged as intervals. Overlap is the normal case, not an edge
/// one — an f-string's holes sit inside its own string span, and a fragment's holes sit inside the fragment — and a
/// byte map answers "who owns this byte" once, in layer order, instead of at each pairwise intersection.
pub fn classified_ranges(source: &str, ast: Option<&Program>) -> Vec<ClassifiedRange> {
    let regions = resolve_regions(&ast.map(collect_regions).unwrap_or_default(), source.len());
    let (tokens, _lex_errors) = lexer::lex_tolerant(source);

    let mut map: Vec<Option<Paint>> = vec![None; source.len()];
    paint(&mut map, &classify_token_stream(&tokens, &regions));

    // ---- Embedded fragments replace the token layer over their own extent ----
    if let Some(program) = ast {
        apply_fragments(program, source, &mut map);
    }

    // ---- Comments claim what nothing else did ----
    let comments = comment_ranges(source, &map);
    paint(&mut map, &comments);

    collapse(&map, source)
}

/// Collapse the composed byte map into the fewest contiguous, character-aligned ranges.
fn collapse(map: &[Option<Paint>], source: &str) -> Vec<ClassifiedRange> {
    let mut ranges: Vec<ClassifiedRange> = Vec::new();
    let mut run: Option<(usize, Paint)> = None;
    for (offset, slot) in map.iter().enumerate() {
        match (run, slot) {
            (Some((_, current)), Some(next)) if current == *next => {}
            (Some((start, (category, modifiers))), _) => {
                push_run(&mut ranges, start, offset, category, modifiers, source);
                run = slot.map(|paint| (offset, paint));
            }
            (None, Some(paint)) => run = Some((offset, *paint)),
            (None, None) => {}
        }
    }
    if let Some((start, (category, modifiers))) = run {
        push_run(&mut ranges, start, map.len(), category, modifiers, source);
    }
    ranges
}

/// Record one collapsed run, snapped to character boundaries.
fn push_run(
    ranges: &mut Vec<ClassifiedRange>,
    start: usize,
    end: usize,
    category: Category,
    modifiers: u32,
    source: &str,
) {
    let mut start = start;
    let mut end = end;
    while start < source.len() && !source.is_char_boundary(start) {
        start += 1;
    }
    while end > start && end <= source.len() && !source.is_char_boundary(end) {
        end -= 1;
    }
    if end > start {
        ranges.push(ClassifiedRange {
            start,
            end,
            category,
            modifiers,
        });
    }
}

// ============================================================================
// ENCODING
// ============================================================================

/// Encode classified ranges as the protocol's flat `[deltaLine, deltaStartChar, length, type, modifiers]` array.
///
/// Two protocol constraints shape this. Positions are counted in UTF-16 code units, not bytes, so a document with
/// any non-ASCII character above a token would otherwise highlight at the wrong column. And a token may not span
/// lines, so a multi-line range — a triple-quoted string, a markup fragment — is emitted as one token per line.
pub fn encode(source: &str, ranges: &[ClassifiedRange]) -> Vec<SemanticToken> {
    let mut encoded = Vec::new();
    let index = LineIndex::new(source);
    let (mut previous_line, mut previous_start) = (0u32, 0u32);

    for range in ranges {
        for (line, start_char, length) in line_pieces_indexed(source, &index, range.start, range.end) {
            if length == 0 {
                continue;
            }
            let delta_line = line - previous_line;
            let delta_start = if delta_line == 0 {
                start_char - previous_start
            } else {
                start_char
            };
            encoded.push(SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type: range.category.index(),
                token_modifiers_bitset: range.modifiers,
            });
            previous_line = line;
            previous_start = start_char;
        }
    }

    encoded
}

/// Byte offsets of every line start, so a position can be resolved without rescanning the document.
///
/// Resolving each range by walking `char_indices` from the beginning is `O(ranges × document)`, which measured at
/// 4.2 seconds for an 84 KB file — the dominant cost of the whole pass, and one the language server would pay on
/// every keystroke. Locating the line by search and counting UTF-16 units only from that line's start makes it
/// proportional to the lines a range actually covers.
struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    /// Index every line start in `source`.
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(source.match_indices('\n').map(|(offset, _)| offset + 1));
        Self { starts }
    }

    /// Return the 0-based line containing `offset`.
    fn line_of(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next.saturating_sub(1),
        }
    }

    /// Count UTF-16 code units between a line's start and `offset`.
    fn utf16_column(&self, source: &str, line: usize, offset: usize) -> u32 {
        let line_start = self.starts.get(line).copied().unwrap_or(0);
        source[line_start..offset.min(source.len())]
            .chars()
            .map(|character| character.len_utf16() as u32)
            .sum()
    }

    /// Byte offset one past the end of a line's content, excluding its newline.
    fn line_end(&self, source: &str, line: usize) -> usize {
        self.starts
            .get(line + 1)
            .map(|next| next.saturating_sub(1))
            .unwrap_or(source.len())
    }
}

/// Split a byte range into one `(line, startChar, length)` triple per line it covers, in UTF-16 code units.
fn line_pieces_indexed(source: &str, index: &LineIndex, start: usize, end: usize) -> Vec<(u32, u32, u32)> {
    let mut pieces = Vec::new();
    let mut cursor = start;
    while cursor < end {
        let line = index.line_of(cursor);
        let line_end = index.line_end(source, line).min(end);
        if line_end > cursor {
            let column = index.utf16_column(source, line, cursor);
            let width: u32 = source[cursor..line_end]
                .chars()
                .map(|character| character.len_utf16() as u32)
                .sum();
            if width > 0 {
                pieces.push((line as u32, column, width));
            }
        }
        // Step past this line's newline; a range ending exactly at the newline stops here.
        cursor = index.line_end(source, line) + 1;
    }
    pieces
}

/// Classify a document and encode it for `textDocument/semanticTokens/full`.
///
/// `ast` is optional on purpose. A document mid-edit frequently does not parse, and an editor that loses all
/// highlighting on every keystroke is worse than one that never had semantic highlighting at all. Without an AST the
/// token-stream layer still classifies keywords, literals, comments, calls and member access; type positions and
/// embedded-fragment ownership are the two things that degrade.
pub fn semantic_tokens(source: &str, ast: Option<&Program>) -> Vec<SemanticToken> {
    let ranges = classified_ranges(source, ast);
    encode(source, &ranges)
}

#[cfg(test)]
mod tests;
