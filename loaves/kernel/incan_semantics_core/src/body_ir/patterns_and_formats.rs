//! Match arms, patterns, pattern bindings, format parts and unary operators.

use super::*;

/// One arm of a [`Rvalue::Match`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArm {
    /// The pattern tested against the match scrutinee.
    pub pattern: Pattern,
    /// Statements needed to compute `guard`, run only once this arm's `pattern` has already matched (a guard may
    /// read this arm's own pattern-bound locals -- see [`Pattern::Var`]). Empty when the arm has no `if` guard, or
    /// when its guard needs no supporting statements of its own.
    pub guard_stmts: Vec<Statement>,
    /// The arm's optional `if` guard: when present and this arm's pattern matches, the arm only runs if this also
    /// evaluates truthy; otherwise matching falls through to the next arm. `None` for an unguarded arm.
    pub guard: Option<Operand>,
    /// Statements needed to compute `result`, run once this arm is selected (its pattern matched, and its `guard`,
    /// if any, was truthy).
    pub body_stmts: Vec<Statement>,
    /// This arm's produced value. A source arm whose body is a statement block rather than a single `=> expr`
    /// always resolves to [`Constant::Unit`].
    pub result: Operand,
}

impl MatchArm {
    /// Render a deterministic maintainer-facing spelling for this arm, flattening `guard_stmts`/`body_stmts` onto
    /// one `; `-joined segment each so the whole arm nests inside [`Rvalue::render_snapshot`]'s single-line output.
    pub(super) fn render_snapshot(&self) -> String {
        let mut out = self.pattern.render_snapshot();
        if let Some(guard) = &self.guard {
            let flattened = render_flattened_stmts(&self.guard_stmts);
            if flattened.is_empty() {
                let _ = write!(&mut out, " if {}", guard.render_snapshot());
            } else {
                let _ = write!(
                    &mut out,
                    " if {{ {}; {} }}",
                    flattened.join("; "),
                    guard.render_snapshot()
                );
            }
        }
        let flattened = render_flattened_stmts(&self.body_stmts);
        if flattened.is_empty() {
            let _ = write!(&mut out, " => {}", self.result.render_snapshot());
        } else {
            let _ = write!(
                &mut out,
                " => {{ {}; {} }}",
                flattened.join("; "),
                self.result.render_snapshot()
            );
        }
        out
    }
}

/// A `match` arm's pattern.
///
/// The vocabulary is small and closed: a consumer translates each variant directly into the matching native pattern
/// syntax and leaves the destructuring and dispatch to rustc. A binding ([`Self::Var`]) carries no bare source name:
/// its [`PatternBinding`] holds an already-declared [`LocalId`] plus the Duckborrower fact/last-use marker for reading
/// that part of the scrutinee, consistent with #653's requirement that ownership decisions be represented as
/// explicit facts on the model itself, not deferred to a consumer's own name resolution.
///
/// [`Self::UnionMember`] retains the checker-selected target of an ordinary union constructor pattern. Native backends
/// can select its payload variant without resolving the written constructor name. Subset targets remain explicit types;
/// each backend must admit or refuse their reconstruction rather than silently treating them as concrete payloads. RFC
/// 021 field-alias refinement is still separate from this vocabulary. Every [`PatternBinding`]'s local carries the
/// checker's per-pattern-node type and its corresponding ownership fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Pattern {
    /// `_`: matches anything, binds nothing.
    Wildcard,
    /// A plain name pattern (`x`): matches anything and binds the matched value (or, when nested inside a
    /// [`Self::Tuple`]/[`Self::Struct`]/[`Self::Enum`], the matched sub-value) to a fresh local.
    Var(PatternBinding),
    /// A literal pattern (`42`, `"hello"`, `true`, `none`, ...): matches only a scrutinee equal to this constant.
    Literal(Constant),
    /// A tuple pattern (`(a, b)`): matches a tuple scrutinee and recursively matches/binds each element.
    Tuple(Vec<Pattern>),
    /// A named-field constructor pattern (`Point(x=a, y=b)`) outside the direct nominal profile. The optional
    /// canonical fact preserves what checking proved, but does not by itself assert a runtime field layout.
    Struct {
        /// Exact checker-selected constructor identity, when checking proved one.
        canonical: Option<CanonicalSymbolId>,
        name: String,
        fields: Vec<(String, Pattern)>,
    },
    /// A source-local plain-model pattern whose exact declaration identity is retained alongside canonical named
    /// fields. This is distinct from [`Self::Struct`], which has no admitted direct-layout target and remains a
    /// visible refusal even when its optional checked identity is present.
    Nominal {
        target: NominalPatternTarget,
        fields: Vec<(String, Pattern)>,
    },
    /// A source-local fieldless normal-enum unit pattern with exact enum/member identities.
    FieldlessEnumVariant(FieldlessEnumVariantTarget),
    /// One intrinsic `Result` constructor pattern with its recursively lowered payload patterns.
    Result {
        variant: ResultVariantKind,
        fields: Vec<Pattern>,
    },
    /// A positional constructor pattern (`Some(x)`, `Ok(value)`, `Shape::Circle(r)`): matches an enum-variant (or
    /// `Option`/`Result`) scrutinee and recursively matches/binds each positional field. `name` is the enum type
    /// name when known, or empty when v0 lowering could not resolve it (matching the existing backend's own
    /// `Pattern::Enum { name: String::new(), .. }` fallback for a bare, non-union constructor pattern) -- a target
    /// backend must not rely on `name` being populated, and must refuse rather than reconstruct a missing
    /// `canonical` identity.
    Enum {
        /// Exact checker-selected constructor/member identity, when checking proved one.
        canonical: Option<CanonicalSymbolId>,
        name: String,
        variant: String,
        fields: Vec<Pattern>,
    },
    /// An alternation pattern (`A | B`): matches if any alternative matches. Incan's typechecker (RFC 071) requires
    /// every alternative to bind an identical name/type set (`check_or_pattern` in
    /// `loaves/compiler/incan_frontend/src/typechecker/check_expr/match_.rs`), so lowering declares exactly one shared
    /// local per bound name across all alternatives rather than one per alternative -- see
    /// `BodyBuilder::lower_match_pattern` in `loaves/compiler/incan_frontend/src/body_ir.rs`.
    Or(Vec<Pattern>),
    /// A checker-selected ordinary union member constructor pattern. The retained target type, rather than the written
    /// constructor spelling, selects the native payload variant.
    UnionMember {
        /// Alias-expanded member or subset type selected by pattern checking.
        ty: IncanType,
        /// Positional payload patterns; the native scalar-member profile admits zero or one.
        fields: Vec<Pattern>,
    },
}

impl Pattern {
    /// Render a deterministic maintainer-facing spelling for this pattern.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Wildcard => "_".to_string(),
            Self::Var(binding) => binding.render_snapshot(),
            Self::Literal(constant) => constant.render_snapshot(),
            Self::Tuple(items) => {
                let items: Vec<String> = items.iter().map(Pattern::render_snapshot).collect();
                format!("({})", items.join(", "))
            }
            Self::Struct {
                canonical,
                name,
                fields,
            } => {
                let fields: Vec<String> = fields
                    .iter()
                    .map(|(field_name, pat)| format!("{field_name}: {}", pat.render_snapshot()))
                    .collect();
                let canonical = canonical
                    .as_ref()
                    .map_or_else(|| "<unresolved>".to_string(), CanonicalSymbolId::render_compact);
                format!("{name} {{ {} }} canonical={canonical}", fields.join(", "))
            }
            Self::UnionMember { ty, fields } => {
                let fields: Vec<String> = fields.iter().map(Pattern::render_snapshot).collect();
                format!("union {ty}({})", fields.join(", "))
            }
            Self::Nominal { target, fields } => {
                let fields: Vec<String> = fields
                    .iter()
                    .map(|(field_name, pat)| format!("{field_name}: {}", pat.render_snapshot()))
                    .collect();
                format!("nominal {} {{ {} }}", target.render_snapshot(), fields.join(", "))
            }
            Self::FieldlessEnumVariant(target) => {
                format!("fieldless {}", target.render_snapshot())
            }
            Self::Result { variant, fields } => {
                let fields: Vec<String> = fields.iter().map(Pattern::render_snapshot).collect();
                format!("Result::{}({})", variant.as_str(), fields.join(", "))
            }
            Self::Enum {
                canonical,
                name,
                variant,
                fields,
            } => {
                let label = if name.is_empty() {
                    variant.clone()
                } else {
                    format!("{name}::{variant}")
                };
                let canonical = canonical
                    .as_ref()
                    .map_or_else(|| "<unresolved>".to_string(), CanonicalSymbolId::render_compact);
                if fields.is_empty() {
                    format!("{label} canonical={canonical}")
                } else {
                    let fields: Vec<String> = fields.iter().map(Pattern::render_snapshot).collect();
                    format!("{label}({}) canonical={canonical}", fields.join(", "))
                }
            }
            Self::Or(items) => {
                let items: Vec<String> = items.iter().map(Pattern::render_snapshot).collect();
                items.join(" | ")
            }
        }
    }
}

/// Exact source-local plain-model declaration selected by one structural pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NominalPatternTarget {
    /// Exact source-local model declaration identity.
    pub direct_declaration_id: CompilerNodeId,
    /// RFC 120 identity of the selected model declaration.
    pub canonical: CanonicalSymbolId,
    /// Canonical model name retained only for malformed-Body-IR cross-checks and diagnostics.
    pub name: String,
}

impl NominalPatternTarget {
    /// Render the nominal identity explicitly so a snapshot cannot mistake the name for an authority.
    pub(super) fn render_snapshot(&self) -> String {
        format!("{} id={}", self.name, self.direct_declaration_id)
    }
}

/// A name bound by a [`Pattern::Var`], pairing the fresh arm-scoped [`LocalId`] lowering declared for it with the
/// Duckborrower fact/last-use marker for reading the part of the scrutinee it binds.
///
/// See [`Rvalue::Match`]'s docs for why a pattern binding is modeled as this kind of read rather than an explicit
/// `Assign` statement copying out of the scrutinee: the actual value transfer happens as a side effect of the
/// target backend's native pattern match, and this model only needs to record *how* that transfer should be
/// treated (move/clone/borrow/copy) -- the same way [`PlaceOperand`] records it for an ordinary read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternBinding {
    pub local: LocalId,
    /// The ownership decision selected for this binding, computed the same way [`PlaceOperand::fact`] is: the
    /// frontend lowering that builds this model (`loaves/compiler/incan_frontend/src/body_ir.rs`) calls its own
    /// equivalent of the same place-read ownership selection used for every other read in this file, on the
    /// scrutinee place this binding projects into.
    pub fact: OwnershipFact,
    /// Whether this is statically the last read of the underlying scrutinee place this binding was computed from
    /// (see [`PlaceOperand::last_use`] for the same caveat about `fact`/`last_use` being kept as separate facts).
    pub last_use: bool,
}

impl PatternBinding {
    /// Render a deterministic maintainer-facing spelling for this binding.
    pub(super) fn render_snapshot(&self) -> String {
        format!(
            "bind(_{}, {}{})",
            self.local.0,
            self.fact.as_str(),
            if self.last_use { ", last_use" } else { "" }
        )
    }
}

/// One part of an [`Rvalue::Format`] f-string, either a literal text chunk carried through verbatim or an
/// already-lowered embedded expression plus the formatting style its source `{expr}`/`{expr!r}` syntax requested.
/// The expression side carries an [`Operand`] rather than a full expression tree: Body IR always lowers embedded
/// expressions through the same [`Operand`]-producing path as any other read, so ownership facts and last-use tracking
/// apply to f-string interpolations exactly like any other expression use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FormatPart {
    /// Literal text between interpolations, carried through unescaped: the lexer has already collapsed `{{` and
    /// `}}` to one brace, and whether a target needs them escaped again is that target's concern, not something this
    /// target-agnostic model commits to.
    Literal(String),
    /// An interpolated `{expr}` or `{expr!r}` segment.
    Expr {
        /// The already-lowered embedded expression's value.
        operand: Box<Operand>,
        /// Which formatting style the source syntax requested for this interpolation.
        style: FormatStyle,
    },
}

impl FormatPart {
    /// Render a deterministic maintainer-facing spelling for this format part.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Literal(s) => format!("lit({s:?})"),
            Self::Expr { operand, style } => {
                format!("{}:{}", operand.render_snapshot(), style.as_str())
            }
        }
    }
}

/// Formatting style requested by one f-string interpolation (`{expr}` vs. `{expr!r}`). This model carries no
/// target-representation logic, since Body IR v0 stays target-agnostic and leaves the decision of how a given style
/// maps to a concrete formatting call to the consuming backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FormatStyle {
    /// User-facing display formatting (`{value}`).
    Display,
    /// User-facing display formatting of a value whose checked type is a type parameter (`{value}` with `value: T`).
    ///
    /// Generic code formats such a value through its type argument's own display implementation, so a closed
    /// instance must not apply the formatting a concrete value of that type receives; an integral float, for one,
    /// keeps no fractional part.
    TypeParameterDisplay,
    /// Structured debug formatting (`{value!r}`).
    Debug,
}

impl FormatStyle {
    /// Compact snapshot spelling for this formatting style.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::TypeParameterDisplay => "type_parameter_display",
            Self::Debug => "debug",
        }
    }
}

/// Unary operator kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    Neg,
    Not,
    Invert,
}

impl UnOp {
    /// Compact snapshot spelling for this unary operator.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "not ",
            Self::Invert => "~",
        }
    }
}
