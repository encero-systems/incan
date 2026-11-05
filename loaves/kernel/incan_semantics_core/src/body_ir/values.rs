//! Constants, numeric newtype constants, ownership facts, rvalues and variant targets.

use super::*;

/// A literal constant operand value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Constant {
    /// A checked primitive type used as a value-level SDK marker; its represented type is retained without source-name
    /// inference.
    TypeToken(IncanType),
    /// Static collection or tuple data with its checked carrier and source-ordered elements. A frozen dictionary
    /// stores alternating keys and values; consumers must validate its arity before constructing storage.
    FrozenAggregate(IncanType, Vec<Constant>),
    Int(i64),
    Float(String),
    /// A compiler-typed numeric constant whose canonical kind and full-width payload survived lowering.
    TypedNumeric(TypedNumericConstant),
    Bool(bool),
    Str(String),
    /// Static frozen text retained by the checker, including source-local text constants.
    ///
    /// This carries the Copy frozen identity through union injection; an ordinary owned string must not be
    /// reclassified as frozen merely because its bytes happen to equal a literal.
    FrozenStr(String),
    /// A `b"..."` byte-string literal, represented as an **owned buffer** rather than a borrowed slice.
    ///
    /// This is deliberately its own variant instead of a reuse of [`Self::Str`]. `bytes` and `str` are distinct
    /// source types with distinct equality, and the compiler-owned string helpers ([`HelperOp::StrConcat`] and its
    /// siblings) assume their operands are text; a byte string arriving as a string constant would be handed to
    /// them silently.
    ///
    /// The owned-versus-borrowed choice is stated rather than left implicit because it decides the
    /// [`OwnershipFact`] every later read of a `bytes` value gets. `IncanType::Primitive(IncanPrimitiveType::Bytes)`
    /// reports [`crate::AbiV0Ownership::Owned`], so re-reading a `bytes` local must select
    /// [`OwnershipFact::Clone`] or [`OwnershipFact::Move`] — exactly what [`Self::Str`] already gets, and the
    /// opposite of what a borrowed-slice representation would have claimed. Holding the literal as an owned buffer
    /// keeps the constant and its type's own ownership fact agreeing.
    ///
    /// Like [`Self::Str`], this records no runtime requirement of its own: it is an immutable literal buffer, not a
    /// helper-constructed value. Operations *on* bytes keep whatever refusal they already had.
    Bytes(Vec<u8>),
    Unit,
    None,
    /// A checked immutable sized-integer newtype value with its exact constructor and complete numeric payload.
    NumericNewtype(Box<NumericNewtypeConstant>),
    /// A metadata-selected public Rust constant, evaluated by rustc with its exact checked carrier.
    RustItem(String, IncanType),
}

/// Pure newtype data accepted by the declaring const checker, without a runtime call or module initialization.
/// This fact covers one sized-integer tuple slot, not model construction, defaults, hooks, or generic instances.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NumericNewtypeConstant {
    /// Exact checked owning declaration, physical layout, and sole positional argument binding.
    pub constructor: ConstructorTarget,
    /// Full-width integer value already checked against the underlying numeric kind.
    pub value: TypedNumericConstant,
}

impl NumericNewtypeConstant {
    /// Validate the complete immutable value against one retained nominal before projection or materialization.
    /// A same-spelled owner, changed address, shifted field, open instance, or mismatched numeric width grants no
    /// value.
    pub fn matches_layout(&self, layout: &NominalDeclaration) -> bool {
        let Some(ty) = self.numeric_type() else {
            return false;
        };
        let binding_matches = matches!(&self.constructor.binding,
            ArgumentBinding::Resolved { arguments, defaulted_slots }
                if arguments.len() == 1 && defaulted_slots.is_empty()
                    && arguments.first().is_some_and(|bound| bound.slot == 0 && bound.written_position == 0));
        layout.canonical.kind == crate::SemanticSourceTargetKind::Newtype
            && layout.type_parameters.is_empty()
            && layout.type_parameter_count == 0
            && layout.class_inheritance.is_none()
            && !layout.has_field_defaults
            && layout.fields.as_slice() == ["0"]
            && layout.field_identities.len() == 1
            && layout.field_identities.first() == Some(&layout.canonical)
            && layout.field_types.len() == 1
            && layout.field_types.first() == Some(&ty)
            && self.constructor.canonical.as_ref() == Some(&layout.canonical)
            && self.constructor.direct_declaration_id.as_ref() == Some(&layout.direct_declaration_id)
            && self.constructor.canonical_field_layout.as_deref() == Some(layout.fields.as_slice())
            && binding_matches
    }

    /// Require a matching signed/unsigned kind and an in-range payload; other numeric families are absent.
    pub(super) fn numeric_type(&self) -> Option<IncanType> {
        use incan_lang::numeric_values::{IntegerBounds, integer_bounds};
        let kind = match &self.value {
            TypedNumericConstant::Signed { kind, value } => {
                let IntegerBounds::Signed { minimum, maximum } = integer_bounds(*kind)? else {
                    return None;
                };
                if *value < minimum || *value > maximum {
                    return None;
                }
                *kind
            }
            TypedNumericConstant::Unsigned { kind, value } => {
                let IntegerBounds::Unsigned { maximum } = integer_bounds(*kind)? else {
                    return None;
                };
                if *value > maximum {
                    return None;
                }
                *kind
            }
            _ => return None,
        };
        Some(IncanType::Primitive(crate::IncanPrimitiveType::Numeric(kind)))
    }
}

/// One exact numeric constant retained in Body IR without collapsing its checked type or value domain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypedNumericConstant {
    /// An exact signed integer constant with its canonical checked identity.
    Signed {
        /// Canonical signed integer kind selected by the typechecker.
        kind: NumericTypeId,
        /// Exact signed payload, already checked against `kind`'s range.
        value: i128,
    },
    /// An exact unsigned integer constant with its canonical checked identity.
    Unsigned {
        /// Canonical unsigned integer kind selected by the typechecker.
        kind: NumericTypeId,
        /// Exact unsigned payload, already checked against `kind`'s range.
        value: u128,
    },
    /// An exact IEEE-754 binary32 constant retained by its bit representation.
    F32 {
        /// Raw binary32 bits after source literal parsing and f32 rounding.
        bits: u32,
    },
    /// An exact IEEE-754 binary64 constant retained by its bit representation.
    F64 {
        /// Raw binary64 bits after source literal parsing.
        bits: u64,
    },
    /// A fixed-scale decimal constant retaining its checked type and written scale.
    Decimal {
        /// Checked decimal precision in the supported range `1..=38`.
        precision: u8,
        /// Checked maximum fractional scale, no greater than `precision`.
        scale: u8,
        /// Signed literal digits with the decimal point removed.
        coefficient: i128,
        /// Fractional digits written by the source literal, no greater than `scale`.
        literal_scale: u8,
    },
}

impl TypedNumericConstant {
    /// Return the canonical checked kind retained by this constant.
    pub fn type_name(&self) -> String {
        match self {
            Self::Signed { kind, .. } | Self::Unsigned { kind, .. } => {
                incan_lang::lang::types::numerics::as_str(*kind).to_string()
            }
            Self::F32 { .. } => "f32".to_string(),
            Self::F64 { .. } => "f64".to_string(),
            Self::Decimal { precision, scale, .. } => format!("decimal[{precision}, {scale}]"),
        }
    }
}

impl Constant {
    /// Render a deterministic maintainer-facing spelling for this constant.
    ///
    /// Frozen text carries an explicit type marker. A byte string renders every byte as a `\xNN` escape rather than as
    /// text, so a snapshot can never read a `bytes` constant as the `str` constant it is not.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::RustItem(path, ty) => format!("const<rust::{path}: {ty}>"),
            Self::FrozenAggregate(ty, values) => format!("const<{ty}>({values:?})"),
            Self::TypeToken(ty) => format!("const<Type[{ty}]>"),
            Self::Int(v) => format!("const({v})"),
            Self::Float(v) => format!("const({v})"),
            Self::TypedNumeric(value) => format!("const<{}>({value:?})", value.type_name()),
            Self::NumericNewtype(value) => format!("const<newtype>({value:?})"),
            Self::Bool(v) => format!("const({v})"),
            Self::Str(v) => format!("const({v:?})"),
            Self::FrozenStr(v) => format!("const<FrozenStr>({v:?})"),
            Self::Bytes(bytes) => {
                let mut rendered = String::from("const(b\"");
                for byte in bytes {
                    let _ = write!(&mut rendered, "\\x{byte:02x}");
                }
                rendered.push_str("\")");
                rendered
            }
            Self::Unit => "const(())".to_string(),
            Self::None => "const(none)".to_string(),
        }
    }
}

/// Duckborrower ownership decision for one place-read.
///
/// This refines [`crate::types::AbiV0Ownership`] (which only distinguishes "trivially copy" from "owned") with the
/// move/clone split that a real Rust emission boundary needs, plus an explicit [`OwnershipFact::Unknown`] escape
/// hatch for reads Body IR v0 cannot yet classify — per #653, ownership decisions must be represented "as
/// Duckborrower facts or explicit unknowns," not silently defaulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnershipFact {
    /// Trivial bitwise copy of a `Copy`-shaped type.
    Copy,
    /// Ownership of the place is transferred out (its last use, non-Copy type).
    Move,
    /// The place is cloned because it is read again later and its type is not trivially copyable.
    Clone,
    /// A shared borrow of the place is taken without transferring or duplicating ownership.
    Borrow,
    /// A mutable borrow of the place is taken.
    MutBorrow,
    /// Ownership could not yet be classified by v0's lowering (explicit unknown, not a silent default).
    Unknown,
}

impl OwnershipFact {
    /// Compact snapshot spelling for this fact.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
            Self::Clone => "clone",
            Self::Borrow => "borrow",
            Self::MutBorrow => "mut_borrow",
            Self::Unknown => "unknown",
        }
    }
}

// ============================================================================
// Rvalues
// ============================================================================

/// The right-hand side of an [`StatementKind::Assign`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Rvalue {
    /// Use an operand's value directly.
    Use(Operand),
    UnaryOp(UnOp, Operand),
    BinaryOp(BinOp, Operand, Operand),
    /// Test one runtime value against a type target resolved and retained by the source checker.
    ///
    /// The target is not a runtime operand. Keeping this operation distinct prevents an executor from reparsing a
    /// source type name or treating arbitrary type expressions as first-class runtime values.
    IsInstance {
        value: Operand,
        /// Alias-expanded checked type of the tested value.
        ///
        /// This lets a bounded executor validate the whole admitted type-test profile before effects instead of
        /// accidentally accepting every runtime carrier that happens to produce `false` for one primitive target.
        value_ty: IncanType,
        target: IsInstanceTarget,
    },
    /// Build a tuple, list, or nominal-constructor value from its element/field operands.
    Aggregate(AggregateKind, Vec<ArgumentElement>),
    /// `{k: v, ...}` dict literal, as an ordered list of entries.
    ///
    /// Separate from [`Self::Aggregate`] because a dict entry is a *pair*, not a single element, and a `**source`
    /// spread is neither. Flattening keys and values into one element list would make a mis-paired list
    /// representable and leave the pairing enforceable only by convention; this shape makes it unrepresentable.
    ///
    /// Entries take effect in order, and **a later entry overwrites an earlier one with the same key**. That
    /// precedence is a property of dict construction rather than of any one call site, which is why it is stated
    /// here rather than recorded per call site.
    Dict(Vec<DictEntry>),
    /// Materialize one exact source-local RFC 032 value-enum member.
    ///
    /// This stores declaration identities rather than a raw scalar so a direct runtime can revalidate enum/member
    /// membership against [`BodyIrModule::value_enum_declarations`] before the compiler-provided `.value()` method
    /// exposes the literal. It never represents an ordinary enum, payload variant, alias, import, or Result value.
    ValueEnumVariant(ValueEnumVariantTarget),
    /// Materialize one exact source-local fieldless normal-enum member.
    ///
    /// This stores declaration identities rather than a source spelling so a direct runtime can revalidate the
    /// unit-variant membership against [`BodyIrModule::fieldless_enum_declarations`]. It never represents payload
    /// construction, aliases, imports, or Result values; matching is represented separately by
    /// [`Pattern::FieldlessEnumVariant`].
    FieldlessEnumVariant(FieldlessEnumVariantTarget),
    /// Construct one compiler-owned `Result` variant from one already-lowered payload.
    ///
    /// The variant kind and checked payload/error types are explicit Body-IR facts. A direct runtime may therefore
    /// construct only `Ok` or `Err` without treating an arbitrary same-spelled callable as a Result constructor;
    /// imported conversions, unresolved types, and cross-error-type propagation remain visible refusals.
    ResultVariant(ResultVariant),
    /// An f-string interpolation, built from a sequence of literal text chunks and already-lowered embedded
    /// expressions. It is a dedicated node rather than a helper-call desugar: an f-string is a compiler-owned
    /// structured value, not something a consumer infers later from a call shape (#653 criterion 3).
    Format(Vec<FormatPart>),
    /// A closure literal (`(params) => expr`), or a partial callable's synthesized forwarding closure (`partial
    /// Target(presets)`) -- see `loaves/compiler/incan_frontend/src/body_ir.rs`'s `BodyBuilder::lower_partial`.
    ///
    /// Body IR represents every capture explicitly rather than leaving by-value or by-reference capture to rustc:
    /// representing Duckborrower ownership facts is this IR's reason to exist (#653), so a closure capturing an outer
    /// variable is exactly the kind of copy/move/borrow decision this model must carry, not omit.
    Closure {
        /// The closure's own declared parameters, in order.
        params: Vec<CallableParam>,
        /// Every free variable the closure reads from its enclosing scope (or, for a partial callable's
        /// synthesized closure, every preset value), each lowered exactly once at the point this closure literal is
        /// constructed -- in first-occurrence source order, each carrying its own [`OwnershipFact`]/last-use marker
        /// via the same machinery any other read in this body uses.
        captured_operands: Vec<Operand>,
        /// The closure's own body.
        body: Box<ClosureBody>,
    },
    /// A lazy generator expression (`(element for ... if ...)`).
    ///
    /// `source` is the first `for` clause's source value, evaluated exactly once before this rvalue is constructed.
    /// Every later clause source, filter, and element evaluation belongs to `body` and runs only when a consumer
    /// polls the generator. `captured_operands` snapshots the remaining free variables needed by that deferred body
    /// at construction time, in the same first-occurrence order as [`GeneratorBody::capture_locals`]; a consumer
    /// must bind each captured value to that local before executing `body`. The initial `source` binds separately to
    /// [`GeneratorBody::source_local`] because it is an eagerly evaluated source value rather than a lexical name.
    ///
    /// This is deliberately neither an eager [`AggregateKind::List`] nor a named function call. It makes the
    /// construction-versus-poll boundary, source-order clause evaluation, and capture ownership visible to every
    /// backend instead of asking a target language's closure inference to reconstruct them.
    Generator {
        /// The first `for` clause's source, evaluated once at generator construction.
        source: Operand,
        /// Values captured for deferred clauses, filters, and the element expression.
        captured_operands: Vec<Operand>,
        /// The suspendable body that consumes `source` and yields accepted elements.
        body: Box<GeneratorBody>,
    },
    /// A `match` expression, evaluated by testing the scrutinee against each arm's [`Pattern`] in order and running
    /// the first arm whose pattern matches (and whose optional [`MatchArm::guard`], if present, evaluates truthy).
    ///
    /// Arms use the small, closed [`Pattern`] vocabulary, which a consumer can translate directly into native `match`
    /// arms and leave exhaustiveness checking and destructuring to rustc. Matching the same #653-criterion-3
    /// "compiler-owned semantic gets its own explicit node" treatment as
    /// [`Self::Format`]/[`StatementKind::TryPropagate`]/[`StatementKind::IterNext`], `match` stays a single structured
    /// `Rvalue` rather than being decomposed into a chain of `If` statements, which would re-derive the
    /// destructuring and dispatch a native `match` already provides.
    Match {
        /// The value being matched. Always read as [`OwnershipFact::Borrow`]: more than one arm's pattern bindings
        /// may read from the scrutinee across the arm list (only one arm actually runs at a time, but see
        /// `BodyBuilder::lower_match` in `loaves/compiler/incan_frontend/src/body_ir.rs` for why this model's last-use
        /// approximation does not attempt per-arm-exclusive dataflow), so treating this top-level read as an
        /// unconditional move would risk being unsound for whichever arm ends up executing. Each pattern
        /// binding computes its own, more precise ownership fact separately (see
        /// [`Pattern::Var`]/[`PatternBinding`]) -- a *nested* (`Tuple`/`Struct`/`Enum`-projected) binding can
        /// never disagree with this field, since a projected read is never a move (see
        /// [`PatternBinding::fact`]'s own docs), but a *root-level* `Pattern::Var`/wildcard binding that
        /// captures the scrutinee's whole value can legitimately select [`OwnershipFact::Move`]/
        /// [`OwnershipFact::Clone`] for that one arm. This field's own `Borrow` and such a root binding's
        /// `Move` are not reconciled against each other here -- a target backend that sees a root-level
        /// `Move`/`Clone` binding in some arm must match the scrutinee by value (or clone it) rather
        /// than by the reference this field's `Borrow` would otherwise suggest, the same kind of cross-fact
        /// reconciliation v0 already leaves to later work elsewhere (see the module-level docs).
        scrutinee: Operand,
        /// Every arm, in source order. The first arm whose pattern matches and whose guard (if any) is truthy runs.
        /// Incan's typechecker enforces match exhaustiveness ahead of lowering (`check_match_exhaustiveness` in
        /// `loaves/compiler/incan_frontend/src/typechecker/check_expr/match_.rs`), so Body IR itself does not need to
        /// model a fallthrough "no arm matched" case.
        arms: Vec<MatchArm>,
    },
}

impl Rvalue {
    /// Render a deterministic maintainer-facing spelling for this rvalue.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Use(op) => op.render_snapshot(),
            Self::UnaryOp(op, operand) => format!("{}{}", op.as_str(), operand.render_snapshot()),
            Self::BinaryOp(op, lhs, rhs) => {
                format!("{} {} {}", lhs.render_snapshot(), op.as_str(), rhs.render_snapshot())
            }
            Self::IsInstance {
                value,
                value_ty,
                target,
            } => {
                format!(
                    "isinstance({}: {}, {})",
                    value.render_snapshot(),
                    value_ty,
                    target.render_snapshot()
                )
            }
            Self::Dict(entries) => {
                let rendered: Vec<String> = entries.iter().map(DictEntry::render_snapshot).collect();
                format!("dict[{}]", rendered.join(", "))
            }
            Self::Aggregate(kind, elements) => {
                let items: Vec<String> = elements.iter().map(ArgumentElement::render_snapshot).collect();
                format!("{}[{}]", kind.as_str(), items.join(", "))
            }
            Self::ValueEnumVariant(target) => target.render_snapshot(),
            Self::FieldlessEnumVariant(target) => target.render_snapshot(),
            Self::ResultVariant(variant) => variant.render_snapshot(),
            Self::Format(parts) => {
                let items: Vec<String> = parts.iter().map(FormatPart::render_snapshot).collect();
                format!("fstring({})", items.join(", "))
            }
            Self::Closure {
                params,
                captured_operands,
                body,
            } => {
                let params_str: Vec<String> = params.iter().map(CallableParam::render_snapshot).collect();
                let captures_str: Vec<String> = captured_operands.iter().map(Operand::render_snapshot).collect();
                format!(
                    "closure(params=[{}], captures=[{}]) {{ {} }}",
                    params_str.join(", "),
                    captures_str.join(", "),
                    body.render_snapshot()
                )
            }
            Self::Generator {
                source,
                captured_operands,
                body,
            } => {
                let captures_str: Vec<String> = captured_operands.iter().map(Operand::render_snapshot).collect();
                format!(
                    "generator(source={}, captures=[{}]) {{ {} }}",
                    source.render_snapshot(),
                    captures_str.join(", "),
                    body.render_snapshot()
                )
            }
            Self::Match { scrutinee, arms } => {
                let arms_str: Vec<String> = arms.iter().map(MatchArm::render_snapshot).collect();
                format!("match {} {{ {} }}", scrutinee.render_snapshot(), arms_str.join(", "))
            }
        }
    }
}

/// Exact checked target carried by a Body-IR `isinstance` operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsInstanceTarget {
    /// Alias-expanded semantic target type.
    pub ty: IncanType,
    /// Canonical declaration identity for a nominal target, when the typechecker proved one.
    pub canonical: Option<CanonicalSymbolId>,
    /// Original source range of the target expression.
    pub span: HirSourceSpan,
}

impl IsInstanceTarget {
    /// Render the retained target type, source span, and optional nominal identity for deterministic Body-IR evidence.
    pub(super) fn render_snapshot(&self) -> String {
        let canonical = self.canonical.as_ref().map_or_else(String::new, |identity| {
            let origin = identity
                .module_path()
                .map(|path| path.join("::"))
                .unwrap_or_else(|| "<non-module>".to_string());
            format!(
                ", canonical={origin}::{}:{}@{}..{}",
                identity.declaration_name,
                identity.kind,
                identity.declaration_span.start,
                identity.declaration_span.end
            )
        });
        format!("target={}@{}..{}{}", self.ty, self.span.start, self.span.end, canonical)
    }
}

/// Exact source-local enum/member identity selected for one value-enum member expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueEnumVariantTarget {
    /// Exact source-local owner enum declaration identity.
    pub enum_declaration_id: CompilerNodeId,
    /// Exact checker-minted owner identity selected at this reference site.
    pub enum_canonical: CanonicalSymbolId,
    /// Exact source-local member declaration identity.
    pub variant_declaration_id: CompilerNodeId,
    /// Exact checker-minted member identity selected at this reference site.
    pub variant_canonical: CanonicalSymbolId,
    /// Canonical owner name, retained for malformed-Body-IR cross-checks and diagnostics.
    pub enum_name: String,
    /// Canonical member name, retained for malformed-Body-IR cross-checks and diagnostics.
    pub variant_name: String,
}

impl ValueEnumVariantTarget {
    /// Render both retained identities so snapshots cannot mistake a member spelling for its direct target fact.
    pub(super) fn render_snapshot(&self) -> String {
        format!(
            "value_enum_variant({}::{} enum_id={} enum_canonical={} variant_id={} variant_canonical={})",
            self.enum_name,
            self.variant_name,
            self.enum_declaration_id,
            self.enum_canonical.render_compact(),
            self.variant_declaration_id,
            self.variant_canonical.render_compact()
        )
    }
}

/// Exact source-local enum/member identity selected for one fieldless normal-enum member expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldlessEnumVariantTarget {
    /// Exact source-local owner enum declaration identity.
    pub enum_declaration_id: CompilerNodeId,
    /// Exact checker-minted owner identity selected at this reference site.
    pub enum_canonical: CanonicalSymbolId,
    /// Exact source-local member declaration identity.
    pub variant_declaration_id: CompilerNodeId,
    /// Exact checker-minted member identity selected at this reference site.
    pub variant_canonical: CanonicalSymbolId,
    /// Canonical owner name, retained for malformed-Body-IR cross-checks and diagnostics.
    pub enum_name: String,
    /// Canonical member name, retained for malformed-Body-IR cross-checks and diagnostics.
    pub variant_name: String,
}

impl FieldlessEnumVariantTarget {
    /// Render both retained identities so snapshots cannot mistake a unit-variant spelling for a direct target fact.
    pub(super) fn render_snapshot(&self) -> String {
        format!(
            "fieldless_enum_variant({}::{} enum_id={} enum_canonical={} variant_id={} variant_canonical={})",
            self.enum_name,
            self.variant_name,
            self.enum_declaration_id,
            self.enum_canonical.render_compact(),
            self.variant_declaration_id,
            self.variant_canonical.render_compact()
        )
    }
}
