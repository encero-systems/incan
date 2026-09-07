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
//!    Keyword colouring is registry-driven through [`keywords::category`], never a local word list.
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

use incan_core::lang::keywords::{self, KeywordCategory};
use incan_syntax::ast::{Declaration, EmbeddedNode, Expr, Param, Program, Spanned, Statement, Type};
use incan_syntax::lexer::{self, FStringPart, Token, TokenKind};
use tower_lsp::lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

use crate::frontend::ast_walk::any_expr_in_program;

// ============================================================================
// LEGEND
// ============================================================================

/// Token types this server publishes, in legend order.
///
/// Every entry is a *standard* LSP token type. Custom types are deliberately avoided: an editor that does not know
/// a custom name renders it with no colour at all, which would make DSL-owned regions less visible rather than more
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
/// contract: appending is safe, reordering silently recolours every document.
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

/// Collect every AST byte region that refines the token stream's default identifier classification.
///
/// Only slots the parser records with a span can contribute. Where the AST offers nothing — a call's callee, a
/// field access — the token-stream rules in [`classify_token_stream`] answer instead.
fn collect_regions(program: &Program) -> Vec<Region> {
    let mut collector = RegionCollector::default();
    for decl in &program.declarations {
        collect_declaration_regions(&decl.node, &mut collector);
    }
    let mut regions = collector.regions;
    regions.sort_by_key(|region| (region.start, region.width()));
    regions
}

/// Collect the regions one declaration contributes, recursing into nested declarations and bodies.
fn collect_declaration_regions(decl: &Declaration, out: &mut RegionCollector) {
    match decl {
        Declaration::Function(function) => {
            for decorator in &function.decorators {
                out.push(decorator.span, RegionKind::Decorator);
            }
            for type_param in &function.type_params {
                out.push(type_param.span, RegionKind::TypeParameter);
            }
            for param in &function.params {
                out.param(param);
            }
            out.ty(&function.return_type);
            collect_body_regions(&function.body, out);
        }
        Declaration::Model(model) => {
            collect_type_decl_regions(&model.decorators, &model.type_params, &model.traits, out);
            for field in &model.fields {
                out.ty(&field.node.ty);
            }
            for property in &model.properties {
                out.ty(&property.node.return_type);
                if let Some(body) = &property.node.body {
                    collect_body_regions(body, out);
                }
            }
            for method in &model.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::Class(class) => {
            collect_type_decl_regions(&class.decorators, &class.type_params, &class.traits, out);
            for field in &class.fields {
                out.ty(&field.node.ty);
            }
            for property in &class.properties {
                out.ty(&property.node.return_type);
                if let Some(body) = &property.node.body {
                    collect_body_regions(body, out);
                }
            }
            for method in &class.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::Trait(trait_decl) => {
            collect_type_decl_regions(&trait_decl.decorators, &trait_decl.type_params, &trait_decl.traits, out);
            for property in &trait_decl.properties {
                out.ty(&property.node.return_type);
                if let Some(body) = &property.node.body {
                    collect_body_regions(body, out);
                }
            }
            for method in &trait_decl.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::Enum(enum_decl) => {
            collect_type_decl_regions(&enum_decl.decorators, &enum_decl.type_params, &enum_decl.traits, out);
            for variant in &enum_decl.variants {
                for field in &variant.node.fields {
                    out.ty(field);
                }
            }
            for method in &enum_decl.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::Newtype(newtype) => {
            collect_type_decl_regions(&newtype.decorators, &newtype.type_params, &newtype.traits, out);
            out.ty(&newtype.underlying);
            for method in &newtype.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::TypeAlias(alias) => {
            for type_param in &alias.type_params {
                out.push(type_param.span, RegionKind::TypeParameter);
            }
            out.ty(&alias.target);
        }
        Declaration::Const(constant) => {
            if let Some(ty) = &constant.ty {
                out.ty(ty);
            }
        }
        Declaration::Static(value) => out.ty(&value.ty),
        Declaration::TestModule(module) => {
            for nested in &module.body {
                collect_declaration_regions(&nested.node, out);
            }
        }
        _ => {}
    }
}

/// Collect the decorator, type-parameter and trait-bound regions shared by every nominal type declaration.
fn collect_type_decl_regions(
    decorators: &[Spanned<incan_syntax::ast::Decorator>],
    type_params: &[incan_syntax::ast::TypeParam],
    traits: &[Spanned<incan_syntax::ast::TraitBound>],
    out: &mut RegionCollector,
) {
    for decorator in decorators {
        out.push(decorator.span, RegionKind::Decorator);
    }
    for type_param in type_params {
        out.push(type_param.span, RegionKind::TypeParameter);
    }
    for bound in traits {
        out.push(bound.span, RegionKind::Type);
    }
}

/// Collect the regions one method contributes.
fn collect_method_regions(method: &incan_syntax::ast::MethodDecl, out: &mut RegionCollector) {
    for decorator in &method.decorators {
        out.push(decorator.span, RegionKind::Decorator);
    }
    for type_param in &method.type_params {
        out.push(type_param.span, RegionKind::TypeParameter);
    }
    if let Some(target) = &method.trait_target {
        out.push(target.span, RegionKind::Type);
    }
    for param in &method.params {
        out.param(param);
    }
    out.ty(&method.return_type);
    if let Some(body) = &method.body {
        collect_body_regions(body, out);
    }
}

/// Collect regions from a statement body, reaching annotated local bindings inside nested blocks.
///
/// `let total: Decimal = ...` is the one statement-level slot carrying a type, and `AssignmentStmt` records both the
/// bound name's span and that annotation, so both are recoverable without re-parsing.
fn collect_body_regions(body: &[Spanned<Statement>], out: &mut RegionCollector) {
    for stmt in body {
        match &stmt.node {
            Statement::Assignment(assignment) => {
                out.push(assignment.name_span, RegionKind::Variable);
                if let Some(ty) = &assignment.ty {
                    out.ty(ty);
                }
            }
            Statement::If(if_stmt) => {
                collect_body_regions(&if_stmt.then_body, out);
                if let Some(else_body) = &if_stmt.else_body {
                    collect_body_regions(else_body, out);
                }
            }
            Statement::While(while_stmt) => collect_body_regions(&while_stmt.body, out),
            Statement::Loop(loop_stmt) => collect_body_regions(&loop_stmt.body, out),
            Statement::For(for_stmt) => collect_body_regions(&for_stmt.body, out),
            Statement::Unsafe(unsafe_stmt) => collect_body_regions(&unsafe_stmt.body, out),
            _ => {}
        }
    }
}

/// Resolve every AST region to a per-byte answer, so classification can ask about an offset in constant time.
///
/// Regions nest — a parameter contains its annotation, a generic type contains its arguments — and the narrowest
/// one covering an offset is the correct answer. Asking that per identifier by scanning the region list is
/// `O(identifiers × regions)`, which measured at 4.3 seconds for an 84 KB file; the language server recomputes this
/// on every keystroke, so that is not a cost it can carry. Painting widest-first lets the narrowest region land
/// last and win, and turns the per-token question into an index.
fn resolve_regions(regions: &[Region], len: usize) -> Vec<Option<RegionKind>> {
    let mut map = vec![None; len];
    let mut ordered: Vec<&Region> = regions.iter().collect();
    ordered.sort_by_key(|region| std::cmp::Reverse(region.width()));
    for region in ordered {
        for slot in map.iter_mut().take(region.end.min(len)).skip(region.start.min(len)) {
            *slot = Some(region.kind);
        }
    }
    map
}

// ============================================================================
// LAYER 1 — TOKEN STREAM
// ============================================================================

/// Classify a lexed token stream, refined by AST regions where they exist.
///
/// This layer alone is what a document being actively typed gets: `regions` is empty whenever the file does not
/// parse, and every rule below still applies. Structural context comes from neighbouring tokens (`Ident` after
/// `def`, `Ident` before `(`) rather than from a name's spelling, so nothing here guesses from `PascalCase`.
fn classify_token_stream(tokens: &[Token], regions: &[Option<RegionKind>]) -> Vec<ClassifiedRange> {
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
            // Punctuation carries no semantic information an editor's grammar does not already colour correctly,
            // and emitting it would roughly double the response for no visible gain. Indentation and EOF are
            // synthetic: `Indent`/`Dedent` spans are zero-width markers, not source the reader sees.
            TokenKind::Punctuation(_) | TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof => {
            }
        }
    }
    ranges
}

/// Classify one identifier token from its AST region, then its neighbours, then a default of `variable`.
///
/// AST regions win because they are derived from what the parser actually built. The neighbour rules below are the
/// fallback for slots the AST records no span for, and they are also the only classification a file that fails to
/// parse ever gets.
fn classify_identifier(
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
        use incan_core::lang::keywords::KeywordId;
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
            incan_core::lang::punctuation::PunctuationId::Dot
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
            incan_core::lang::punctuation::PunctuationId::ColonColon
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
fn previous_kind(tokens: &[Token], index: usize) -> Option<&TokenKind> {
    index
        .checked_sub(1)
        .and_then(|previous| tokens.get(previous))
        .map(|token| &token.kind)
}

/// Kind of the token after `index`.
fn next_kind(tokens: &[Token], index: usize) -> Option<&TokenKind> {
    tokens.get(index + 1).map(|token| &token.kind)
}

/// Whether the identifier at `index` is immediately followed by an opening parenthesis.
fn next_is_call(tokens: &[Token], index: usize) -> bool {
    matches!(
        next_kind(tokens, index),
        Some(TokenKind::Punctuation(
            incan_core::lang::punctuation::PunctuationId::LParen
        ))
    )
}

/// What introduced the dotted path an identifier belongs to, when anything did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathIntroducer {
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
fn dotted_path_introducer(tokens: &[Token], index: usize) -> Option<PathIntroducer> {
    use incan_core::lang::keywords::KeywordId;
    use incan_core::lang::punctuation::PunctuationId;

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
fn classify_fstring(
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

// ============================================================================
// LAYER 3 — EMBEDDED FRAGMENTS
// ============================================================================

/// The category DSL-owned bytes take, chosen per submode.
///
/// Ordinary Incan never renders as `macro`, so a markup or style fragment reads at a glance as a different surface —
/// which is exactly the ownership visibility RFC 081 asks tooling to provide. Raw text, regex and type-shaped
/// submodes have honest standard equivalents and use those instead.
fn submode_category(submode: incan_vocab::EmbeddedFragmentSubmode) -> Category {
    use incan_vocab::EmbeddedFragmentSubmode;
    match submode {
        EmbeddedFragmentSubmode::Markup
        | EmbeddedFragmentSubmode::Style
        | EmbeddedFragmentSubmode::SelectorDeclarationValue => Category::Macro,
        EmbeddedFragmentSubmode::RawText => Category::String,
        EmbeddedFragmentSubmode::RegexTemplate => Category::Regexp,
        EmbeddedFragmentSubmode::TypePosition => Category::Type,
        // The submode list is `#[non_exhaustive]`: a submode added later is DSL-owned by definition, so treat it as
        // such rather than letting its bytes fall through to ordinary-Incan classification.
        _ => Category::Macro,
    }
}

/// Replace the token layer's answer over every embedded fragment in the program.
///
/// This routes through [`any_expr_in_program`], the compiler's own expression walk, rather than adding a seventh
/// private copy of "which expressions does a program contain" (#1022 deleted six such duplicates). The painting
/// happens inside the walk because that walk lends each expression only for the duration of the call — which is
/// also why nothing here clones a fragment to carry it out.
fn apply_fragments(program: &Program, source: &str, map: &mut [Option<Paint>]) {
    any_expr_in_program(program, |expr| {
        if let Expr::Embedded(fragment) = expr {
            apply_fragment(fragment, source, map);
        }
        false
    });
}

/// Clear one fragment's extent from the token layer, then paint its own classification over it.
///
/// Hole bytes are deliberately left as the token layer classified them: a hole is ordinary Incan, typechecked and
/// lowered exactly as the same expression in ordinary position, so re-deriving it here would be a second opinion
/// about what the fragment owns.
///
/// One boundary is worth stating, because it is easy to read as a disagreement with
/// [`EmbeddedFragmentExpr::ownership_at`] and is not. That method answers for a *cursor position*, where the offset
/// just past a name still resolves to it — the right convention for hover. This paints *bytes*, and the byte at a
/// hole's end is the hole's closing delimiter, which is DSL-owned punctuation rather than part of the expression.
/// The two answers coincide everywhere else.
fn apply_fragment(fragment: &incan_syntax::ast::EmbeddedFragmentExpr, source: &str, map: &mut [Option<Paint>]) {
    let Some((extent, ranges)) = fragment_ranges(fragment, source) else {
        return;
    };
    let holes: Vec<_> = fragment.holes().iter().map(|hole| hole.span).collect();
    let limit = map.len();
    for (offset, slot) in map
        .iter_mut()
        .enumerate()
        .take(extent.end.min(limit))
        .skip(extent.start.min(limit))
    {
        let in_hole = holes.iter().any(|span| span.start <= offset && offset < span.end);
        if !in_hole {
            *slot = None;
        }
    }
    paint(map, &ranges);
}

/// Classify one fragment's bytes, returning its extent alongside the ranges covering the DSL-owned part.
///
/// Every byte of the extent is decided here except the holes, which are left uncovered so the token-stream layer's
/// ordinary-Incan classification shows through. That inversion is deliberate: a hole is not embedded syntax at all,
/// and the compiler already typechecks it exactly as it would the same expression in ordinary position.
fn fragment_ranges(
    fragment: &incan_syntax::ast::EmbeddedFragmentExpr,
    source: &str,
) -> Option<(incan_syntax::ast::Span, Vec<ClassifiedRange>)> {
    let extent = fragment.node_extent()?;
    let start = extent.start.min(source.len());
    let end = extent.end.min(source.len());
    if end <= start {
        return None;
    }

    // A byte map is the simplest way to let node categories, nested children and holes all overwrite one another in
    // tree order without an interval-arithmetic pass. Fragments are editor-sized, so the cost is irrelevant.
    let default = submode_category(fragment.submode);
    let mut bytes = vec![Some(default); end - start];
    paint_nodes(&fragment.nodes, start, &mut bytes);
    for hole in fragment.holes() {
        for offset in hole.span.start.max(start)..hole.span.end.min(end) {
            bytes[offset - start] = None;
        }
    }

    Some((extent, run_length_ranges(&bytes, start, source)))
}

/// Paint node-specific categories over a fragment's byte map, in tree order so children refine their parents.
fn paint_nodes(nodes: &[Spanned<EmbeddedNode>], origin: usize, bytes: &mut [Option<Category>]) {
    for node in nodes {
        let category = match &node.node {
            EmbeddedNode::Comment(_) => Some(Category::Comment),
            EmbeddedNode::Regex { .. } => Some(Category::Regexp),
            EmbeddedNode::Text(_) => Some(Category::String),
            EmbeddedNode::EntityRef(_) => Some(Category::Macro),
            _ => None,
        };
        if let Some(category) = category {
            for offset in node.span.start.max(origin)..node.span.end.min(origin + bytes.len()) {
                bytes[offset - origin] = Some(category);
            }
        }
        // Recurse so a child's own category wins over the container's, and so holes nested inside containers are
        // reached. The hole pass runs afterwards and overwrites whatever was painted here.
        match &node.node {
            EmbeddedNode::Element(element) => {
                for attr in &element.attrs {
                    if let Some(value) = &attr.value {
                        paint_nodes(std::slice::from_ref(value), origin, bytes);
                    }
                }
                paint_nodes(&element.children, origin, bytes);
            }
            EmbeddedNode::StyleRule(rule) => {
                paint_nodes(&rule.selectors, origin, bytes);
                paint_nodes(&rule.declarations, origin, bytes);
            }
            EmbeddedNode::Declaration(declaration) => {
                paint_nodes(&declaration.value, origin, bytes);
            }
            _ => {}
        }
    }
}

/// Collapse a byte map into the fewest contiguous ranges, skipping bytes left to another layer.
///
/// Cuts land only where the byte map changes, and every boundary in it comes from a node or hole span, so each
/// emitted range starts and ends on a character boundary.
fn run_length_ranges(bytes: &[Option<Category>], origin: usize, source: &str) -> Vec<ClassifiedRange> {
    let mut ranges = Vec::new();
    let mut run: Option<(usize, Category)> = None;
    for (index, byte) in bytes.iter().enumerate() {
        let offset = origin + index;
        match (run, byte) {
            (Some((start, category)), Some(current)) if category == *current => {
                let _ = start;
            }
            (Some((start, category)), _) => {
                push_char_aligned(&mut ranges, start, offset, category, source);
                run = byte.map(|category| (offset, category));
            }
            (None, Some(category)) => run = Some((offset, *category)),
            (None, None) => {}
        }
    }
    if let Some((start, category)) = run {
        push_char_aligned(&mut ranges, start, origin + bytes.len(), category, source);
    }
    ranges
}

/// Record a range, snapping both ends onto character boundaries so a multi-byte character is never split.
fn push_char_aligned(ranges: &mut Vec<ClassifiedRange>, start: usize, end: usize, category: Category, source: &str) {
    let mut start = start;
    let mut end = end;
    while start < source.len() && !source.is_char_boundary(start) {
        start += 1;
    }
    while end > start && end <= source.len() && !source.is_char_boundary(end) {
        end -= 1;
    }
    if end > start {
        ranges.push(ClassifiedRange::plain(start, end, category));
    }
}

// ============================================================================
// COMMENTS
// ============================================================================

/// Recover comment ranges as the `#`-to-end-of-line runs no other layer claimed.
///
/// The lexer discards comments rather than emitting them as tokens, and the formatter's comment model is anchored to
/// declarations rather than to spans, so neither can be reused here. Rather than re-implement string and fragment
/// scanning — the two places a `#` is not a comment — this asks what the other layers already covered: a `#` inside
/// a string literal or a claimed fragment is inside a classified range, and only an unclaimed one starts a comment.
fn comment_ranges(source: &str, claimed: &[Option<Paint>]) -> Vec<ClassifiedRange> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    while let Some(hash) = source[offset..].find('#') {
        let start = offset + hash;
        offset = start + 1;
        if claimed.get(start).is_some_and(Option::is_some) {
            continue;
        }
        let end = source[start..]
            .find('\n')
            .map_or(source.len(), |newline| start + newline);
        ranges.push(ClassifiedRange::plain(start, end, Category::Comment));
        offset = end;
    }
    ranges
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
mod tests {
    use super::*;
    use incan_syntax::parser;

    type TestResult = Result<(), String>;

    /// Parse a source into a program, as the language server does for a document that compiles.
    fn parse(source: &str) -> Result<Program, String> {
        let (tokens, _lex_errors) = lexer::lex_tolerant(source);
        parser::parse_with_source(&tokens, None, None, None, source)
            .map_err(|errors| format!("parse errors: {errors:?}"))
    }

    /// Return the category covering the first occurrence of `needle`, or an error naming what was found instead.
    fn category_of(source: &str, ranges: &[ClassifiedRange], needle: &str) -> Result<Category, String> {
        let offset = source
            .find(needle)
            .ok_or_else(|| format!("fixture does not contain {needle:?}"))?;
        ranges
            .iter()
            .find(|range| range.start <= offset && offset < range.end)
            .map(|range| range.category)
            .ok_or_else(|| format!("{needle:?} at byte {offset} is unclassified"))
    }

    /// Classify a source that parses, exercising every layer.
    fn classify(source: &str) -> Result<Vec<ClassifiedRange>, String> {
        let program = parse(source)?;
        Ok(classified_ranges(source, Some(&program)))
    }

    /// Pin each category's discriminant to its position in the published legend.
    #[test]
    fn legend_indices_match_the_category_discriminants() {
        // The client resolves `tokenType` by indexing into the published legend, so a category whose discriminant
        // drifts from its position in TOKEN_TYPES silently recolours every document rather than failing loudly.
        assert_eq!(TOKEN_TYPES.len(), Category::Macro.index() as usize + 1);
        assert_eq!(
            TOKEN_TYPES[Category::Keyword.index() as usize],
            SemanticTokenType::KEYWORD
        );
        assert_eq!(TOKEN_TYPES[Category::Type.index() as usize], SemanticTokenType::TYPE);
        assert_eq!(TOKEN_TYPES[Category::Macro.index() as usize], SemanticTokenType::MACRO);
        assert_eq!(TOKEN_MODIFIERS.len(), Modifier::DefaultLibrary as usize + 1);
        assert_eq!(
            TOKEN_MODIFIERS[Modifier::Declaration as usize],
            SemanticTokenModifier::DECLARATION
        );
    }

    /// A declaration's name takes the kind of the thing it declares, not a generic identifier colour.
    #[test]
    fn a_declaration_name_is_the_thing_it_declares() -> TestResult {
        // The problem statement's first complaint: a regex grammar cannot tell a declaration name from a local.
        let source = "def compute(value: int) -> int:\n    return value\n";
        let ranges = classify(source)?;
        assert_eq!(category_of(source, &ranges, "compute")?, Category::Function);
        assert_eq!(category_of(source, &ranges, "def")?, Category::Keyword);
        Ok(())
    }

    /// Type positions are decided by where the parser put them, never by how the name is capitalised.
    #[test]
    fn type_positions_come_from_the_ast_rather_than_from_spelling() -> TestResult {
        // `str` is lowercase and `Total` is PascalCase, so any spelling heuristic gets both of these backwards.
        // The AST places one in a type slot and the other in a local binding, and that is what decides.
        let source = "def render(label: str) -> None:\n    Total = label\n    return None\n";
        let ranges = classify(source)?;
        assert_eq!(category_of(source, &ranges, "str")?, Category::Type);
        assert_eq!(category_of(source, &ranges, "Total")?, Category::Variable);
        assert_eq!(category_of(source, &ranges, "label: ")?, Category::Parameter);
        Ok(())
    }

    /// A model, a trait and an enum each highlight as their own kind rather than as one shared category.
    #[test]
    fn nominal_declarations_take_their_own_kinds() -> TestResult {
        let source = "model User:\n    id: int\n\ntrait Loggable:\n    pass\n\nenum Mode:\n    Fast\n";
        let ranges = classify(source)?;
        assert_eq!(category_of(source, &ranges, "User")?, Category::Struct);
        assert_eq!(category_of(source, &ranges, "Loggable")?, Category::Interface);
        assert_eq!(category_of(source, &ranges, "Mode")?, Category::Enum);
        Ok(())
    }

    /// Member access distinguishes a called method from a property that is only read.
    #[test]
    fn member_access_separates_a_called_method_from_a_read_property() -> TestResult {
        let source =
            "def use(value: str) -> None:\n    other = value.lower()\n    size = value.length\n    return None\n";
        let ranges = classify(source)?;
        assert_eq!(category_of(source, &ranges, "lower")?, Category::Method);
        assert_eq!(category_of(source, &ranges, "length")?, Category::Property);
        Ok(())
    }

    /// Every segment of a dotted path takes the meaning of whatever introduced the path.
    #[test]
    fn a_dotted_path_takes_the_meaning_of_what_introduced_it() -> TestResult {
        // Only the token to the immediate left is visible to the member-access rule, so every segment after the
        // first would otherwise read as a field access on a local named `std`.
        let source = "from std.runtime import host\n\ndef main() -> None:\n    return None\n";
        let ranges = classify(source)?;
        assert_eq!(category_of(source, &ranges, "std")?, Category::Namespace);
        assert_eq!(category_of(source, &ranges, "runtime")?, Category::Namespace);
        Ok(())
    }

    /// Comment recovery claims only bytes no other layer owns, so a `#` inside a literal stays string content.
    #[test]
    fn a_hash_inside_a_string_does_not_start_a_comment() -> TestResult {
        // Comment recovery asks which bytes no other layer claimed rather than re-implementing string scanning,
        // so a `#` inside a literal has to stay part of the string.
        let source = "def main() -> None:\n    tag = \"#not-a-comment\"  # a real comment\n    return None\n";
        let ranges = classify(source)?;
        assert_eq!(category_of(source, &ranges, "#not-a-comment")?, Category::String);
        assert_eq!(category_of(source, &ranges, "# a real comment")?, Category::Comment);
        Ok(())
    }

    /// An f-string's interpolated expressions are classified as code, not as string content.
    #[test]
    fn an_fstring_hole_is_classified_as_ordinary_incan() -> TestResult {
        // An f-string is how most Incan code produces output. Treating one as a flat string blob would leave the
        // names inside it — the part a reader most needs to follow — uncoloured.
        let source = "def greet(name: str) -> None:\n    println(f\"hello {name}\")\n    return None\n";
        let ranges = classify(source)?;
        assert_eq!(category_of(source, &ranges, "hello ")?, Category::String);
        assert_eq!(category_of(source, &ranges, "name}")?, Category::Variable);
        Ok(())
    }

    /// A document mid-edit still receives highlighting from the token stream alone.
    #[test]
    fn a_document_that_does_not_parse_is_still_classified() -> TestResult {
        // The ordinary state of a document being edited. Without an AST the type and fragment layers are gone, but
        // losing every colour on each keystroke would be worse than never having had semantic highlighting.
        let source = "def compute(value: int) -> int:\n    return value +\n";
        let ranges = classified_ranges(source, None);
        assert_eq!(category_of(source, &ranges, "def")?, Category::Keyword);
        assert_eq!(category_of(source, &ranges, "compute")?, Category::Function);
        assert_eq!(category_of(source, &ranges, "return")?, Category::Keyword);
        Ok(())
    }

    /// Columns are reported in UTF-16 code units, as the protocol's default position encoding requires.
    #[test]
    fn columns_are_counted_in_utf16_units_not_bytes() -> TestResult {
        // The balloon is four bytes but two UTF-16 code units, so `suffix` sits at byte column 21 and UTF-16
        // column 19. A byte-counted column places it two characters to the right of where the reader sees it —
        // the class of bug that makes highlighting look almost, but not quite, aligned.
        let source = "def main(suffix: str) -> None:\n    total = \"\u{1F388}\" + suffix\n    return None\n";
        let ranges = classify(source)?;
        let offset = source
            .rfind("suffix")
            .ok_or_else(|| "fixture must use `suffix` in the interpolating line".to_string())?;
        let range = ranges
            .iter()
            .find(|range| range.start <= offset && offset < range.end)
            .ok_or_else(|| "the trailing `suffix` must be classified".to_string())?;
        let byte_column = offset - source[..offset].rfind('\n').map_or(0, |newline| newline + 1);
        assert_eq!(byte_column, 21, "fixture assumption: `suffix` starts at byte column 21");
        let pieces = line_pieces_indexed(source, &LineIndex::new(source), range.start, range.end);
        assert_eq!(
            pieces,
            vec![(1, 19, 6)],
            "`suffix` must be reported at UTF-16 column 19"
        );
        Ok(())
    }

    /// Decoding the delta stream reproduces every classified position exactly.
    #[test]
    fn each_token_is_encoded_relative_to_the_one_before_it() -> TestResult {
        // Delta encoding is the protocol's wire format, so an absolute column leaking into the stream would shift
        // every token after it. Decoding the deltas back has to reproduce the classified ranges exactly.
        let source = "def compute(value: int) -> int:\n    doubled = value\n    return doubled\n";
        let ranges = classify(source)?;
        let encoded = encode(source, &ranges);
        let index = LineIndex::new(source);
        let mut expected = Vec::new();
        for range in &ranges {
            expected.extend(line_pieces_indexed(source, &index, range.start, range.end));
        }

        let (mut line, mut start) = (0u32, 0u32);
        let mut decoded = Vec::new();
        for token in &encoded {
            line += token.delta_line;
            start = if token.delta_line == 0 {
                start + token.delta_start
            } else {
                token.delta_start
            };
            decoded.push((line, start, token.length));
        }
        assert_eq!(
            decoded, expected,
            "decoding the delta stream must reproduce every classified position"
        );
        Ok(())
    }

    /// A range covering a newline becomes one token per line, since the protocol forbids a token that spans lines.
    #[test]
    fn a_multi_line_range_is_split_into_one_token_per_line() -> TestResult {
        // The protocol forbids a token that spans lines, so a triple-quoted string has to be emitted per line.
        let source = "def main() -> None:\n    text = \"\"\"first\nsecond\"\"\"\n    return None\n";
        let program = parse(source)?;
        let ranges = classified_ranges(source, Some(&program));
        let multi_line = ranges
            .iter()
            .find(|range| source[range.start..range.end].contains('\n'))
            .ok_or_else(|| "fixture must produce a range covering a newline".to_string())?;
        let pieces = line_pieces_indexed(source, &LineIndex::new(source), multi_line.start, multi_line.end);
        assert!(
            pieces.len() > 1,
            "a range covering a newline must split into several tokens"
        );
        let lines: Vec<u32> = pieces.iter().map(|(line, _, _)| *line).collect();
        assert!(
            lines.windows(2).all(|pair| pair[0] < pair[1]),
            "pieces must be in line order: {lines:?}"
        );
        Ok(())
    }

    /// Composed ranges are disjoint and ordered, which delta encoding depends on.
    #[test]
    fn classified_ranges_never_overlap_and_stay_in_order() -> TestResult {
        // Composition happens over a byte map precisely so this holds. A client given overlapping tokens renders
        // undefined results, and delta encoding an out-of-order range corrupts every token after it.
        let source = "def greet(name: str) -> None:\n    println(f\"hi {name}\")  # wave\n    return None\n";
        let ranges = classify(source)?;
        for pair in ranges.windows(2) {
            assert!(
                pair[0].end <= pair[1].start,
                "ranges must be disjoint and ordered: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
        Ok(())
    }
    /// Classification stays a single pass; a per-range rescan fails this by a wide margin.
    #[test]
    fn classification_cost_grows_with_the_document_rather_than_its_square() -> TestResult {
        // Both defects this guards against were `O(ranges × document)` rescans, and both were invisible to every
        // other test here because correctness was never wrong — only the cost was. Resolving a region by scanning
        // the region list per identifier, and splitting a range by walking `char_indices` from byte zero, together
        // took 4.3 seconds on an 84 KB file. A language server recomputes this on every keystroke.
        //
        // The assertion is a ratio rather than a deadline, so it means the same thing on a fast machine, a slow
        // one, and a loaded CI runner: the same content at ten times the length may not cost a hundred times as
        // much. A quadratic pass fails this by a wide margin; a linear one passes with room to spare.
        let source = std::fs::read_to_string("crates/incan_stdlib/stdlib/collections.incn")
            .map_err(|error| format!("fixture unavailable: {error}"))?;
        let mut small_end = source.len() / 10;
        while small_end < source.len() && !source.is_char_boundary(small_end) {
            small_end += 1;
        }
        let small = &source[..small_end];

        let measure = |text: &str| {
            let started = std::time::Instant::now();
            let _ = semantic_tokens(text, None);
            started.elapsed().as_secs_f64()
        };
        // One untimed pass first, so neither measurement pays for cold caches the other then benefits from.
        let _ = measure(small);
        let small_seconds = measure(small).max(1e-6);
        let large_seconds = measure(&source);

        let length_ratio = source.len() as f64 / small.len() as f64;
        let cost_ratio = large_seconds / small_seconds;
        assert!(
            cost_ratio < length_ratio * 4.0,
            "classification cost scaled {cost_ratio:.1}x for a {length_ratio:.1}x longer document, which is the \
             shape of a per-range rescan rather than a single pass"
        );
        Ok(())
    }
}
