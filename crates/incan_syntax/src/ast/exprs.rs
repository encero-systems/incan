//! Expression AST types: literals, operators, calls, match/if expressions, comprehensions, and surface expressions.

use std::fmt;

use incan_semantics_core::SurfaceFeatureKey;

use super::{Ident, Param, Span, Spanned, Statement, Type, VocabBlockStmt};

// ============================================================================
// Expressions
// ============================================================================

/// Slice expression: represents `start:end` or `start:end:step`
/// All components are optional, e.g., `[:5]`, `[2:]`, `[::2]`
#[derive(Debug, Clone, PartialEq)]
pub struct SliceExpr {
    pub start: Option<Box<Spanned<Expr>>>,
    pub end: Option<Box<Spanned<Expr>>>,
    pub step: Option<Box<Spanned<Expr>>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Identifier
    Ident(Ident),
    /// Literal
    Literal(Literal),
    /// `self`
    SelfExpr,
    /// Binary operation: `a + b`
    Binary(Box<Spanned<Expr>>, BinaryOp, Box<Spanned<Expr>>),
    /// Unary operation: `-x`, `not x`
    Unary(UnaryOp, Box<Spanned<Expr>>),
    /// Function/method call: `f(a, b)` or `f[T](a, b)`
    Call(Box<Spanned<Expr>>, Vec<Spanned<Type>>, Vec<CallArg>),
    /// Index: `x[i]`
    Index(Box<Spanned<Expr>>, Box<Spanned<Expr>>),
    /// Slice: `x[start:end]` or `x[start:end:step]`
    Slice(Box<Spanned<Expr>>, SliceExpr),
    /// Field access: `x.field`
    Field(Box<Spanned<Expr>>, Ident),
    /// Method call: `x.method(args)` or `x.method[T](args)`
    MethodCall(Box<Spanned<Expr>>, Ident, Vec<Spanned<Type>>, Vec<CallArg>),
    /// Partial callable preset expression: `partial Target(name=value)`.
    Partial(Box<PartialExpr>),
    /// `expr?` (try/propagate)
    Try(Box<Spanned<Expr>>),
    /// Match expression
    Match(Box<Spanned<Expr>>, Vec<Spanned<MatchArm>>),
    /// If expression
    If(Box<IfExpr>),
    /// Loop expression
    Loop(Box<LoopExpr>),
    /// List comprehension: `[expr for x in iter if cond]`
    ListComp(Box<ListComp>),
    /// Dict comprehension: `{k: v for x in iter if cond}`
    DictComp(Box<DictComp>),
    /// Generator expression: `(expr for x in iter if cond)`
    Generator(Box<GeneratorExpr>),
    /// Closure: `(x, y) => expr` (a lot like python's lambda)
    Closure(Vec<Spanned<Param>>, Box<Spanned<Expr>>),
    /// Tuple: `(a, b)`
    Tuple(Vec<Spanned<Expr>>),
    /// List literal: `[a, *b, c]`
    List(Vec<ListEntry>),
    /// Dict literal: `{k: v, **other}`
    Dict(Vec<DictEntry>),
    /// Set literal: `{a, b, c}`
    Set(Vec<Spanned<Expr>>),
    /// Parenthesized expression
    Paren(Box<Spanned<Expr>>),
    /// Type constructor: `Some(x)`, `Ok(x)`, `User(id=1, name="Ada")`
    Constructor(Ident, Vec<CallArg>),
    /// f-string: `f"Hello {name}"`
    FString(Vec<FStringPart>),
    /// `yield expr` (for fixtures/generators)
    Yield(Option<Box<Spanned<Expr>>>),
    /// Range expression: `start..end` (exclusive) or `start..=end` (inclusive)
    Range {
        start: Box<Spanned<Expr>>,
        end: Box<Spanned<Expr>>,
        inclusive: bool,
    },
    /// Generic surface expression routed to semantics handlers.
    Surface(Box<SurfaceExpr>),
    /// Raw library vocab declaration used as an expression before vocab desugaring.
    VocabBlock(Box<VocabBlockStmt>),
    /// Descriptor-gated embedded-fragment artifact (RFC 081): a language-shaped lexical submode claimed by a DSL
    /// descriptor, with real expression holes that flow through ordinary typecheck/lowering.
    ///
    /// Unlike [`Expr::Surface`], this variant is not eliminated by the pre-typecheck vocab desugar pass — its
    /// [`EmbeddedNode::Hole`] sub-expressions are genuine Incan expressions that must be typechecked and lowered
    /// like any other expression, so the container must survive into `check_expr`/`lower/expr` as itself.
    Embedded(Box<EmbeddedFragmentExpr>),
}

/// One entry in a list literal.
#[derive(Debug, Clone, PartialEq)]
pub enum ListEntry {
    /// Direct element expression.
    Element(Spanned<Expr>),
    /// Spread another list into the literal at this position.
    Spread(Spanned<Expr>),
}

/// One entry in a dict literal.
#[derive(Debug, Clone, PartialEq)]
pub enum DictEntry {
    /// Direct key/value pair.
    Pair(Spanned<Expr>, Spanned<Expr>),
    /// Spread another dict into the literal at this position.
    Spread(Spanned<Expr>),
}

/// A keyword preset supplied to a partial callable template.
#[derive(Debug, Clone, PartialEq)]
pub struct PartialArg {
    pub name: Ident,
    pub value: Spanned<Expr>,
}

/// Local partial callable preset expression payload.
#[derive(Debug, Clone, PartialEq)]
pub struct PartialExpr {
    pub target: Box<Spanned<Expr>>,
    pub type_args: Vec<Spanned<Type>>,
    pub args: Vec<PartialArg>,
}

/// Generic surface expression node emitted by parser handoff.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceExpr {
    pub key: SurfaceFeatureKey,
    pub payload: SurfaceExprPayload,
}

/// Surface expression payload variants.
#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceExprPayload {
    /// Prefix unary keyword expression: `kw expr`.
    PrefixUnary(Box<Spanned<Expr>>),
    /// Import-activated `std.async` race block: `race for value: ...`.
    RaceFor(Box<RaceForExpr>),
    /// DSL-owned leading-dot path with an implicit receiver: `.field` or `.relation.field`.
    LeadingDotPath {
        segments: Vec<Ident>,
        receiver: incan_vocab::ScopedSurfaceReceiver,
        owner: ScopedSurfaceOwner,
    },
    /// DSL-owned binary glyph with local block semantics.
    ScopedGlyph {
        glyph: String,
        left: Box<Spanned<Expr>>,
        right: Box<Spanned<Expr>>,
        owner: ScopedSurfaceOwner,
    },
    /// DSL-owned identifier call accepted in an eligible name-resolution position.
    ScopedSymbolCall {
        symbol: Ident,
        args: Vec<CallArg>,
        owner: ScopedSurfaceOwner,
    },
}

/// Expression-position `race for value:` surface syntax.
#[derive(Debug, Clone, PartialEq)]
pub struct RaceForExpr {
    /// The one source-authored winner binding shared by every arm, including its exact header token span.
    pub binding: Spanned<Ident>,
    pub arms: Vec<RaceForArm>,
}

/// One `await expr => body` arm in a race expression.
#[derive(Debug, Clone, PartialEq)]
pub struct RaceForArm {
    pub awaitable: Spanned<Expr>,
    pub body: RaceForBody,
}

/// Body form for a race arm.
#[derive(Debug, Clone, PartialEq)]
pub enum RaceForBody {
    Expr(Spanned<Expr>),
    Block(Vec<Spanned<Statement>>),
}

/// Source DSL context that accepted a scoped surface expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedSurfaceOwner {
    pub declaration: String,
    pub clause: Option<String>,
    pub call: Option<String>,
}

// ============================================================================
// RFC 081 — descriptor-gated embedded-fragment artifact
// ============================================================================

/// Typed artifact produced when a descriptor claims a lexical submode for an eligible position (RFC 081, `#1023`).
///
/// This is the parser's committed answer to "what does this embedded source mean": `submode` names the fixed
/// grammar family the descriptor claimed (see `incan_vocab::EmbeddedFragmentSubmode`), `nodes` is the structural
/// tree produced by that grammar, and `source_text` preserves the original fragment text verbatim so a formatter
/// that does not understand this descriptor's grammar (or one it declares layout-sensitive) can still render it
/// faithfully. `key` mirrors `SurfaceExpr::key`'s identity shape so tooling can correlate an embedded fragment back
/// to the descriptor and dependency that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedFragmentExpr {
    /// Descriptor identity that claimed this fragment, for diagnostics and tooling correlation.
    pub key: SurfaceFeatureKey,
    /// Fixed lexical submode kind this fragment was parsed under.
    pub submode: incan_vocab::EmbeddedFragmentSubmode,
    /// Structural node tree produced by the submode's grammar, in source order.
    pub nodes: Vec<Spanned<EmbeddedNode>>,
    /// Verbatim original source text of the whole fragment, for the formatter's layout-preserving mode and for
    /// tooling that needs the untouched source rather than the structural tree.
    pub source_text: String,
    /// Whether the claiming descriptor declared itself layout-sensitive.
    ///
    /// RFC 081 gives the formatter exactly two modes: render from this structural tree, or preserve `source_text`
    /// verbatim for a DSL that declares itself layout-sensitive. The descriptor owns that choice, and the parser
    /// resolves it here so the formatter never has to rediscover the descriptor to answer it.
    pub layout_sensitive: bool,
}

impl EmbeddedFragmentExpr {
    /// Collect every ordinary-Incan expression hole in this fragment, in source order.
    ///
    /// A hole is genuine Incan syntax: the typechecker and lowering already treat it exactly as they would the same
    /// expression written in ordinary position. Tooling that walks expressions has to reach holes the same way, or a
    /// fragment becomes an opaque region where hover, signature help, and scoped-symbol resolution stop working for
    /// code that is not embedded at all. This is the one traversal that answers "which expressions does this
    /// fragment own", so the compiler and its tooling cannot drift into different answers (RFC 081, #1022).
    pub fn holes(&self) -> Vec<&Spanned<Expr>> {
        let mut holes = Vec::new();
        collect_embedded_holes(&self.nodes, &mut holes);
        holes
    }

    /// Collect every ordinary-Incan expression hole in this fragment mutably, in source order.
    ///
    /// The mutable counterpart to [`EmbeddedFragmentExpr::holes`], for the passes that rewrite a hole in place
    /// rather than only reading it — the pre-typecheck vocab desugar pass is the one such caller, because a hole
    /// may itself contain another DSL's vocab block or scoped surface. It answers the same ownership question as
    /// `holes`, over the same node kinds in the same order, so the two cannot disagree about what a fragment owns.
    pub fn holes_mut(&mut self) -> Vec<&mut Spanned<Expr>> {
        let mut holes = Vec::new();
        collect_embedded_holes_mut(&mut self.nodes, &mut holes);
        holes
    }

    /// Return the source extent this fragment's structural nodes cover, or `None` when it parsed to no nodes.
    ///
    /// [`EmbeddedFragmentExpr`] deliberately carries no span field: the fragment's own span lives on the enclosing
    /// [`Spanned<Expr>`], and duplicating it here would create a second thing to keep in sync. Tooling that has the
    /// fragment but not its wrapper — an [`Expr`]-level predicate walk, for instance — recovers the extent from the
    /// nodes themselves, which carry absolute source offsets. An empty fragment has no DSL-owned content to locate,
    /// so `None` is the honest answer rather than a zero-width guess.
    pub fn node_extent(&self) -> Option<Span> {
        let mut extent: Option<Span> = None;
        for node in &self.nodes {
            extent = Some(match extent {
                Some(current) => current.merge(node.span),
                None => node.span,
            });
        }
        extent
    }

    /// Answer which side of the ownership boundary `offset` falls on inside this fragment.
    ///
    /// RFC 081 draws exactly one boundary through a fragment: an expression hole is ordinary Incan and everything
    /// around it is DSL-owned syntax whose meaning belongs to the claiming descriptor. Tooling that resolves a
    /// position — hover above all — has to ask that question before it interprets anything, because a tag name, a
    /// selector, or a declaration property is spelled like an Incan identifier and would otherwise resolve as one.
    ///
    /// Returns `None` when `offset` lies outside the fragment entirely. The hole side is decided by
    /// [`EmbeddedFragmentExpr::holes`], so this never becomes a second opinion about what a fragment owns.
    pub fn ownership_at(&self, offset: usize) -> Option<EmbeddedOwnership<'_>> {
        let extent = self.node_extent()?;
        if offset < extent.start || offset > extent.end {
            return None;
        }
        let hole = self
            .holes()
            .into_iter()
            .find(|hole| hole.span.start <= offset && offset <= hole.span.end);
        Some(match hole {
            Some(hole) => EmbeddedOwnership::Hole(hole),
            None => EmbeddedOwnership::DslOwned,
        })
    }
}

/// Which side of an embedded fragment's ownership boundary a source offset falls on (RFC 081, #1022).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmbeddedOwnership<'a> {
    /// Ordinary Incan: the offset is inside an expression hole, which is typechecked and lowered exactly as the
    /// same expression written in ordinary position. Tooling should treat it as ordinary Incan, not as DSL syntax.
    Hole(&'a Spanned<Expr>),
    /// DSL-owned syntax: tags, selectors, declaration properties, regex patterns, type shapes, raw text. It carries
    /// no ordinary Incan meaning, so resolving a name here against ordinary Incan scope would be wrong.
    DslOwned,
}

/// Walk embedded nodes in source order, collecting the expression holes reachable from each.
///
/// `Hole` is the one leaf carrying an ordinary expression. Container kinds recurse into their own children; every
/// remaining kind is DSL-owned syntax that cannot contain one.
fn collect_embedded_holes<'a>(nodes: &'a [Spanned<EmbeddedNode>], holes: &mut Vec<&'a Spanned<Expr>>) {
    for node in nodes {
        match &node.node {
            EmbeddedNode::Hole(expr) => holes.push(expr.as_ref()),
            EmbeddedNode::Element(element) => {
                for attr in &element.attrs {
                    if let Some(value) = &attr.value {
                        collect_embedded_holes(std::slice::from_ref(value), holes);
                    }
                }
                collect_embedded_holes(&element.children, holes);
            }
            EmbeddedNode::StyleRule(rule) => {
                collect_embedded_holes(&rule.selectors, holes);
                collect_embedded_holes(&rule.declarations, holes);
            }
            EmbeddedNode::Declaration(declaration) => collect_embedded_holes(&declaration.value, holes),
            EmbeddedNode::Text(_)
            | EmbeddedNode::EntityRef(_)
            | EmbeddedNode::Comment(_)
            | EmbeddedNode::Value(_)
            | EmbeddedNode::Regex { .. }
            | EmbeddedNode::TypeShape(_) => {}
        }
    }
}

/// Walk embedded nodes mutably in source order, collecting the expression holes reachable from each.
///
/// Kept structurally identical to [`collect_embedded_holes`] so the mutable and shared answers to "which
/// expressions does this fragment own" cannot diverge.
fn collect_embedded_holes_mut<'a>(nodes: &'a mut [Spanned<EmbeddedNode>], holes: &mut Vec<&'a mut Spanned<Expr>>) {
    for node in nodes {
        match &mut node.node {
            EmbeddedNode::Hole(expr) => holes.push(expr.as_mut()),
            EmbeddedNode::Element(element) => {
                for attr in &mut element.attrs {
                    if let Some(value) = &mut attr.value {
                        collect_embedded_holes_mut(std::slice::from_mut(value), holes);
                    }
                }
                collect_embedded_holes_mut(&mut element.children, holes);
            }
            EmbeddedNode::StyleRule(rule) => {
                collect_embedded_holes_mut(&mut rule.selectors, holes);
                collect_embedded_holes_mut(&mut rule.declarations, holes);
            }
            EmbeddedNode::Declaration(declaration) => collect_embedded_holes_mut(&mut declaration.value, holes),
            EmbeddedNode::Text(_)
            | EmbeddedNode::EntityRef(_)
            | EmbeddedNode::Comment(_)
            | EmbeddedNode::Value(_)
            | EmbeddedNode::Regex { .. }
            | EmbeddedNode::TypeShape(_) => {}
        }
    }
}

/// One structural node inside a descriptor-gated embedded fragment.
///
/// Node kinds are shared across submodes where the shape is genuinely the same construct (raw text runs and
/// expression holes appear in every submode); submode-specific shapes (markup elements, style rules, regex
/// literals, type shapes) are only ever produced by their owning submode's grammar. Every variant carries enough
/// structure to typecheck the expression holes inside it and to report a diagnostic anchored at the exact
/// sub-region rather than the whole fragment.
#[derive(Debug, Clone, PartialEq)]
pub enum EmbeddedNode {
    /// Raw literal text run, preserved verbatim within this submode's own grammar (whitespace, text-node content,
    /// comment bodies, raw-text/comment submode content).
    Text(String),
    /// Expression hole re-entering ordinary Incan parsing (`{expr}` or `${expr}`).
    ///
    /// This sub-expression is genuine Incan syntax: it is typechecked with `check_expr` and lowered with
    /// `lower_expr` exactly as if it appeared in ordinary expression position, never erased pre-typecheck.
    Hole(Box<Spanned<Expr>>),
    /// Markup element: `<name attr=...>children</name>` or a self-closing `<name .../>`.
    Element(EmbeddedElement),
    /// Markup entity reference, for example `&amp;`.
    EntityRef(String),
    /// Comment content, stored verbatim without its delimiters (`<!-- ... -->` or `/* ... */`).
    Comment(String),
    /// Style rule: a selector list followed by a declaration block.
    StyleRule(EmbeddedStyleRule),
    /// One `property: value;` declaration, used both inside a style rule and as a bare declaration-value fragment.
    Declaration(EmbeddedDeclaration),
    /// A single declaration-value shape (dimension, color, custom-property reference, literal, or selector token).
    Value(EmbeddedValue),
    /// A regex literal: `/pattern/flags`.
    Regex { pattern: String, flags: String },
    /// A minimal representative type-shaped grammar node.
    TypeShape(EmbeddedTypeShape),
}

/// Markup element node: a tag, its attributes, and its children.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedElement {
    /// Tag name, for example `section`.
    pub name: String,
    /// Attributes in source order.
    pub attrs: Vec<EmbeddedAttr>,
    /// Child nodes in source order (text, elements, entity references, comments, holes).
    pub children: Vec<Spanned<EmbeddedNode>>,
    /// Whether the element was written in self-closing form (`<name .../>`), so it has no children/close tag.
    pub self_closing: bool,
}

/// One markup attribute: a name and an optional value (`Text` for a quoted literal, `Hole` for `{expr}`).
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedAttr {
    /// Attribute name, exactly as written (for example `class`, `src`).
    pub name: String,
    /// Attribute value, or `None` for a bare boolean-style attribute (`name` with no `=value`).
    pub value: Option<Spanned<EmbeddedNode>>,
}

/// Style rule: a selector list and its declaration block.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedStyleRule {
    /// Selector list entries, in source order.
    pub selectors: Vec<Spanned<EmbeddedNode>>,
    /// Declarations inside the rule's `{ ... }` block, in source order.
    pub declarations: Vec<Spanned<EmbeddedNode>>,
}

/// One declaration-value production: `property: value1 value2 ...;`.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedDeclaration {
    /// Declared property name, including a leading `--` for custom properties.
    pub property: String,
    /// Declaration value, as a sequence of value/hole nodes (most declarations have exactly one).
    pub value: Vec<Spanned<EmbeddedNode>>,
}

/// A single declaration-value or selector-position literal shape.
#[derive(Debug, Clone, PartialEq)]
pub enum EmbeddedValue {
    /// A numeric dimension, for example `16px` or `2rem` (unit may be empty for a bare number).
    Dimension { number: String, unit: String },
    /// A color literal, for example `#1166ff` (stored including the leading `#`).
    Color(String),
    /// A custom-property reference, for example `var(--accent-color)`.
    CustomPropertyRef(String),
    /// A bare identifier value.
    Ident(String),
    /// A quoted string literal value.
    StringLit(String),
    /// A bare numeric literal (no unit).
    Number(String),
    /// A selector-list entry, for example `.card:hover` or `> #title` (stored as one flat token run).
    Selector(String),
}

/// A minimal, representative type-shaped grammar node (RFC 081's `type-position` submode).
///
/// This intentionally covers only the constructs #1023's acceptance criteria names — namespace-qualified names,
/// generics, nullable, array, and union — not a full external type-system grammar.
#[derive(Debug, Clone, PartialEq)]
pub enum EmbeddedTypeShape {
    /// Namespace-qualified name, for example `a.b.C` (segments in source order).
    Name(Vec<String>),
    /// Generic application, for example `Foo<Bar, Baz>`.
    Generic(Box<EmbeddedTypeShape>, Vec<EmbeddedTypeShape>),
    /// Nullable type, for example `T?`.
    Nullable(Box<EmbeddedTypeShape>),
    /// Array type, for example `T[]`.
    Array(Box<EmbeddedTypeShape>),
    /// Union type, for example `A | B`.
    Union(Vec<EmbeddedTypeShape>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FStringFormat {
    Display,
    Debug,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    Literal(String),
    Expr { expr: Spanned<Expr>, format: FStringFormat },
}

/// Parsed integer literal with the **source substring** used for formatting.
///
/// [`IntLiteral::repr`] is the exact `source[start..end]` span from the lexer (including `_` numeric separators).
///
/// [`PartialEq`] ignores only [`IntLiteral::repr`] so AST equality tests do not depend on source spelling.
#[derive(Debug, Clone)]
pub struct IntLiteral {
    pub value: i64,
    pub magnitude: u128,
    pub repr: String,
}

impl PartialEq for IntLiteral {
    /// Compare numeric meaning while ignoring spelling differences such as separators.
    fn eq(&self, other: &Self) -> bool {
        self.magnitude == other.magnitude
    }
}

impl IntLiteral {
    /// Canonical decimal spelling — for AST nodes built without source text (tests, vocab bridge).
    pub fn synthetic(value: i64) -> Self {
        Self {
            value,
            magnitude: value.unsigned_abs().into(),
            repr: value.to_string(),
        }
    }

    /// Returns `true` when the literal fit in the default signed integer representation at lex time.
    pub fn fits_i64(&self) -> bool {
        self.magnitude <= i64::MAX as u128
    }
}

/// Parsed floating-point literal with the **source substring** used for formatting.
///
/// [`FloatLiteral::repr`] is the exact `source[start..end]` span from the lexer (including `_` numeric separators and
/// the author’s `e` / `E` exponent spelling). It avoids `f64` `Display` shortening (for example `120.0` vs `120`) and
/// keeps formatter output aligned with comment reattachment anchors.
///
/// [`PartialEq`] compares only [`FloatLiteral::value`] (IEEE bits) so AST equality tests do not depend on `repr`.
#[derive(Debug, Clone)]
pub struct FloatLiteral {
    pub value: f64,
    pub repr: String,
}

impl PartialEq for FloatLiteral {
    fn eq(&self, other: &Self) -> bool {
        self.value.to_bits() == other.value.to_bits()
    }
}

/// Parsed decimal literal with the **source substring** used for formatting.
///
/// The `body` field is the numeric spelling without `_` separators and without the trailing `d` suffix. Semantic
/// validation of precision and scale belongs to the typechecker.
#[derive(Debug, Clone)]
pub struct DecimalLiteral {
    pub body: String,
    pub repr: String,
}

impl PartialEq for DecimalLiteral {
    /// Compare semantic decimal literal bodies while ignoring source formatting.
    fn eq(&self, other: &Self) -> bool {
        self.body == other.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(IntLiteral),
    Float(FloatLiteral),
    Decimal(DecimalLiteral),
    String(String),
    Bytes(Vec<u8>),
    Bool(bool),
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv, // // (Python-style floor division)
    Mod,
    Pow,
    MatMul,
    PipeForward,
    PipeBackward,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
    In,
    NotIn,
    Is,
    IsNot,
}

impl fmt::Display for BinaryOp {
    /// Format a binary operator using its source-level spelling.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinaryOp::Add => write!(f, "+"),
            BinaryOp::Sub => write!(f, "-"),
            BinaryOp::Mul => write!(f, "*"),
            BinaryOp::Div => write!(f, "/"),
            BinaryOp::FloorDiv => write!(f, "//"),
            BinaryOp::Mod => write!(f, "%"),
            BinaryOp::Pow => write!(f, "**"),
            BinaryOp::MatMul => write!(f, "@"),
            BinaryOp::PipeForward => write!(f, "|>"),
            BinaryOp::PipeBackward => write!(f, "<|"),
            BinaryOp::BitAnd => write!(f, "&"),
            BinaryOp::BitOr => write!(f, "|"),
            BinaryOp::BitXor => write!(f, "^"),
            BinaryOp::Shl => write!(f, "<<"),
            BinaryOp::Shr => write!(f, ">>"),
            BinaryOp::Eq => write!(f, "=="),
            BinaryOp::NotEq => write!(f, "!="),
            BinaryOp::Lt => write!(f, "<"),
            BinaryOp::Gt => write!(f, ">"),
            BinaryOp::LtEq => write!(f, "<="),
            BinaryOp::GtEq => write!(f, ">="),
            BinaryOp::And => write!(f, "and"),
            BinaryOp::Or => write!(f, "or"),
            BinaryOp::In => write!(f, "in"),
            BinaryOp::NotIn => write!(f, "not in"),
            BinaryOp::Is => write!(f, "is"),
            BinaryOp::IsNot => write!(f, "is not"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    Invert,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallArg {
    /// Positional argument
    Positional(Spanned<Expr>),
    /// Named argument: `name=value`
    Named(Spanned<Ident>, Spanned<Expr>),
    /// Positional unpack argument: `*expr`.
    PositionalUnpack(Spanned<Expr>),
    /// Keyword unpack argument: `**expr`.
    KeywordUnpack(Spanned<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternArg {
    /// Positional pattern: `Type(x)`
    Positional(Spanned<Pattern>),
    /// Named pattern: `Type(name=pat)`
    Named(Spanned<Ident>, Spanned<Pattern>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Spanned<Pattern>,
    pub guard: Option<Spanned<Expr>>, // `if condition` guard
    pub body: MatchBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchBody {
    /// `=> expr` (single expression)
    Expr(Spanned<Expr>),
    /// Block of statements
    Block(Vec<Spanned<Statement>>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    /// Wildcard: `_`
    Wildcard,
    /// Binding: `x`
    Binding(Ident),
    /// Literal: `42`, `"hello"`, `true`
    Literal(Literal),
    /// Constructor: `Some(x)`, `Ok(value)`, `Type(name=pat)`
    Constructor(Spanned<Ident>, Vec<PatternArg>),
    /// Tuple: `(a, b)`
    Tuple(Vec<Spanned<Pattern>>),
    /// Parenthesized pattern used for grouping: `(A | B)`
    Group(Box<Spanned<Pattern>>),
    /// Alternation: `A | B`
    Or(Vec<Spanned<Pattern>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfExpr {
    /// Condition that decides whether the `then` body executes.
    pub condition: Spanned<Expr>,
    /// Statements evaluated when the condition is truthy.
    pub then_body: Vec<Spanned<Statement>>,
    /// Optional fallback statements evaluated when the condition is false.
    pub else_body: Option<Vec<Spanned<Statement>>>,
}

/// Explicit infinite-loop expression (`loop:`).
///
/// Unlike `while`, this form is allowed in expression position and may yield a value via `break <expr>`.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopExpr {
    /// Statements that execute for each loop iteration until a `break` exits the loop.
    pub body: Vec<Spanned<Statement>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListComp {
    /// Element expression produced for each accepted binding.
    pub expr: Spanned<Expr>,
    /// First `for` binding mirrored for single-clause comprehension lowering.
    pub pattern: Spanned<Pattern>,
    /// First `for` iterable mirrored for single-clause comprehension lowering.
    pub iter: Spanned<Expr>,
    /// First trailing `if` filter mirrored for single-clause comprehension lowering.
    pub filter: Option<Spanned<Expr>>,
    /// Parsed comprehension clauses in source order.
    pub clauses: Vec<ComprehensionClause>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DictComp {
    /// Key expression produced for each accepted binding.
    pub key: Spanned<Expr>,
    /// Value expression produced for each accepted binding.
    pub value: Spanned<Expr>,
    /// First `for` binding mirrored for single-clause comprehension lowering.
    pub pattern: Spanned<Pattern>,
    /// First `for` iterable mirrored for single-clause comprehension lowering.
    pub iter: Spanned<Expr>,
    /// First trailing `if` filter mirrored for single-clause comprehension lowering.
    pub filter: Option<Spanned<Expr>>,
    /// Parsed comprehension clauses in source order.
    pub clauses: Vec<ComprehensionClause>,
}

/// Generator-expression payload.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratorExpr {
    /// Expression yielded by the generator for each accepted binding.
    pub expr: Spanned<Expr>,
    /// Parsed comprehension clauses in source order.
    pub clauses: Vec<ComprehensionClause>,
}

/// One clause in a comprehension-like expression.
#[derive(Debug, Clone, PartialEq)]
pub enum ComprehensionClause {
    /// `for pattern in iter`
    For {
        /// Binding pattern introduced by the clause.
        pattern: Spanned<Pattern>,
        /// Iterable source consumed by the clause.
        iter: Spanned<Expr>,
    },
    /// `if condition`
    If(Spanned<Expr>),
}
