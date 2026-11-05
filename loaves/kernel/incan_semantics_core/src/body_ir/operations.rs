//! Binary operators, argument and spread elements, aggregates and argument bindings.

use super::*;

/// Binary operator kind supported by v0 lowering (arithmetic, bitwise, shift, comparison, identity, boolean).
///
/// Every variant here is a *primitive* operation: one machine-level combination of two already-evaluated operands.
/// An operator whose meaning is a runtime call belongs in [`HelperOp`] instead, and an operator whose meaning is a
/// user-defined method belongs in a [`Callee::Method`] call. Keeping those three apart is what lets a consumer read
/// a Body IR body and know whether a source operator costs a machine instruction, a compiler-owned runtime helper,
/// or a user-visible dispatch.
///
/// [`Is`](Self::Is) and [`IsNot`](Self::IsNot) stay distinct from [`Eq`](Self::Eq)/[`Ne`](Self::Ne) even though the
/// existing Rust-emission backend currently emits both pairs the same way. Collapsing them here would discard which
/// operator the source actually wrote, and Body IR is the representation a later identity/equality split would have
/// to be decided against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Is,
    IsNot,
    And,
    Or,
}

impl BinOp {
    /// Compact snapshot spelling for this binary operator.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::FloorDiv => "//",
            Self::Mod => "%",
            Self::Pow => "**",
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::BitXor => "^",
            Self::Shl => "<<",
            Self::Shr => ">>",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Is => "is",
            Self::IsNot => "is not",
            Self::And => "and",
            Self::Or => "or",
        }
    }

    /// Whether this operator can panic at runtime, in the sense Body IR records as a [`PanicFact`].
    ///
    /// The recorded class is *unconditional* failure on a value the operator itself rejects: division and modulo by
    /// a possibly-zero divisor fail on every build profile, so the fact is a property of the operation rather than
    /// of how it is compiled.
    ///
    /// `**`, `<<`, and `>>` are deliberately **not** in that class, and this is a stated decision rather than an
    /// omission (#1160). Each of them can trap only by exceeding the result width — integer `**` overflowing, a
    /// shift distance at or beyond the operand's bit width — which is the same arithmetic-overflow class as `+`,
    /// `-`, and `*`, none of which record a panic fact either. Recording overflow for the shifts and `**` alone
    /// would claim a distinction between them and ordinary arithmetic that does not exist. The bitwise `&`, `|`,
    /// and `^` are total on every input and cannot trap at all. Should Body IR ever model arithmetic overflow, it
    /// belongs to the whole numeric operator set at once, under its own [`PanicReason`].
    pub const fn may_panic(self) -> bool {
        matches!(self, Self::Div | Self::FloorDiv | Self::Mod)
    }
}

/// One element of an aggregate's element list or a call's argument list.
///
/// Body IR's element lists were fixed-arity until #1159: a `Vec<Operand>` says "exactly these values, in these
/// positions", which cannot express "splice this sequence in here, with a length known only at runtime". This enum
/// is what makes arity a represented fact rather than an assumption.
///
/// It is deliberately a change to the element *type* rather than a parallel marker list or a sibling spread-aware
/// statement kind. A parallel list would leave the spread invisible to any consumer that reads the operand vector
/// without also reading the markers — a consumer that did nothing wrong would silently compute the wrong arity. A
/// sibling statement kind would re-fork exactly what [`ArgumentBinding`] unified. Changing the element type instead
/// makes every reader of an element list a compile error at precisely the place a wrong answer would be produced,
/// while readers that never inspect elements are untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ArgumentElement {
    /// Exactly one value at this position.
    One(Operand),
    /// A value written with an argument name, at a call whose declared parameters were not statically resolved.
    ///
    /// Only meaningful in a *call* element list. Nothing in the type prevents one appearing in an aggregate, where
    /// it would be meaningless; lowering never constructs one there.
    ///
    /// A call with a resolved signature never produces this: its named arguments are bound to declared slots and
    /// recorded in [`ArgumentBinding::Resolved`], so they appear as [`Self::One`] in slot order. This form exists
    /// for the case where a spread makes the arity a runtime fact — a callee with a rest parameter — so the name
    /// cannot be resolved to a slot here but must not be discarded either.
    Named { name: String, operand: Operand },
    /// Splice the elements of one source in at this position. Its length is a runtime fact.
    Spread(SpreadElement),
}

impl ArgumentElement {
    /// The single operand at this position, or `None` when this element splices.
    ///
    /// Consumers that only handle fixed arity should refuse on `None` rather than treating a spread as one value.
    pub fn as_one(&self) -> Option<&Operand> {
        match self {
            Self::One(operand) => Some(operand),
            Self::Named { .. } | Self::Spread(_) => None,
        }
    }

    /// Render a deterministic maintainer-facing spelling.
    ///
    /// A non-spread element renders exactly as its operand did before #1159, so every existing snapshot is
    /// unchanged and a spread stands out by its marker alone.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::One(operand) => operand.render_snapshot(),
            Self::Named { name, operand } => format!("{name}={}", operand.render_snapshot()),
            Self::Spread(spread) => spread.render_snapshot(),
        }
    }
}

/// A source whose elements are spliced into the surrounding element list at this position.
///
/// The ownership decision for reading the source lives on `source` itself, like any other operand read — a
/// [`Operand::Place`] carries its own [`OwnershipFact`] and last-use marker. This type adds no separate ownership
/// field; what it adds is that the read is *identified as a splice*, so a consumer distributing the source's
/// elements can see that its length is a runtime fact rather than one value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpreadElement {
    /// The sequence or mapping being spliced.
    pub source: Operand,
    /// Which spread form the source spelled.
    pub kind: SpreadKind,
}

impl SpreadElement {
    /// Render a deterministic maintainer-facing spelling, marked by the source spread form.
    pub(super) fn render_snapshot(&self) -> String {
        let marker = match self.kind {
            SpreadKind::Sequence => "*",
            SpreadKind::Mapping => "**",
        };
        format!("{marker}{}", self.source.render_snapshot())
    }
}

/// Which spread form a [`SpreadElement`] was spelled with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpreadKind {
    /// `*source` — splices a sequence's elements positionally.
    Sequence,
    /// `**source` — splices a mapping's entries by key.
    Mapping,
}

/// One entry of a dict literal, in written source order.
///
/// A dict is built from an ordered sequence of *effects*, not from a finished mapping: keys are expressions evaluated
/// at runtime, their evaluation order is observable, and a duplicate key is legal and meaningful (a later entry
/// overwrites an earlier one). That is why [`Rvalue::Dict`] carries this ordered list rather than a map — a map would
/// have to collapse duplicates at construction, which is exactly the override semantics that must survive, and it could
/// not hold a spread whose keys are unknown until runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DictEntry {
    /// A `key: value` pair. Both operands are evaluated, key first.
    Pair(Operand, Box<Operand>),
    /// A `**source` spread. Its entries are spliced in at this position, and its key set is a runtime fact.
    Spread(SpreadElement),
}

impl DictEntry {
    /// Render a deterministic maintainer-facing spelling.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Pair(key, value) => {
                format!("{}: {}", key.render_snapshot(), value.render_snapshot())
            }
            Self::Spread(spread) => spread.render_snapshot(),
        }
    }
}

/// Aggregate value shape built by [`Rvalue::Aggregate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AggregateKind {
    /// Intrinsic `Some(payload)` selected by checking, with its concrete Option type retained on the destination.
    OptionSome,
    Tuple,
    List,
    /// `{v, ...}` set literal. Operands are the set's elements, one per entry -- the same flat shape as
    /// [`AggregateKind::List`].
    Set,
    /// A `start..end` / `start..=end` **range value**: the same value a `for` header consumes, but usable in any
    /// operand position.
    ///
    /// A range is an aggregate rather than a [`Constant`] form or a helper-constructed value, for two reasons.
    /// Its bounds are arbitrary expressions (`lo..hi` is as legal as `0..10`), which a constant cannot hold; and
    /// it needs no runtime service to exist -- four scalars laid out side by side, allocating nothing and calling
    /// nothing -- so modeling it as a [`Callee::Helper`] call would invent a runtime dependency that the `for`
    /// header's own normalization proves is not there. That is also why range construction records no
    /// [`crate::AbiV0RuntimeRequirement`], the same as [`Self::Tuple`] and unlike [`Self::List`]/[`Self::Set`].
    ///
    /// Operands appear in exactly [`Self::RANGE_FIELDS`] order, and a consumer reading one back projects it by
    /// that name (`PlaceElem::structural_field("start")`). Inclusivity is an *operand*, not a static property of this
    /// variant: `..` versus `..=` is fixed per construction site, but the site that constructs a range and the
    /// loop that later iterates it need not be the same statement, so a consumer holding only the value must be
    /// able to read which one it is instead of having to prove where it came from.
    Range,
    Constructor(Box<ConstructorTarget>),
    /// One variant of a source `enum`, constructed with its payload: `Shape.Circle(r)`, or `Shape.Empty` for a variant
    /// without one.
    ///
    /// Operands are the payload values in declared order, none for a unit variant. This covers the enums the narrower
    /// [`Rvalue::FieldlessEnumVariant`] and [`Rvalue::ValueEnumVariant`] forms do not. A consumer identifies the
    /// variant by [`EnumVariantTarget::variant_canonical`], never by its spelling.
    EnumVariant(Box<EnumVariantTarget>),
}

/// The enum and variant one [`AggregateKind::EnumVariant`] constructs, as the checker selected them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumVariantTarget {
    /// The enum's declaration name, retained for diagnostics and malformed-Body-IR cross-checks.
    pub enum_name: String,
    /// The variant's declaration name, retained for diagnostics and malformed-Body-IR cross-checks.
    pub variant_name: String,
    /// Checker-minted identity of the owning enum, when the reference site retained one.
    pub enum_canonical: Option<CanonicalSymbolId>,
    /// Checker-minted identity of the selected variant: the dispatch key.
    pub variant_canonical: CanonicalSymbolId,
    /// Resolved binding of the aggregate's operands to the variant's declared payload positions.
    pub binding: ArgumentBinding,
}

impl AggregateKind {
    /// Declared name of an [`Self::Range`] aggregate's lower bound.
    pub const RANGE_FIELD_START: &'static str = "start";
    /// Declared name of an [`Self::Range`] aggregate's upper bound, whose own inclusivity is
    /// [`Self::RANGE_FIELD_INCLUSIVE`].
    pub const RANGE_FIELD_END: &'static str = "end";
    /// Declared name of an [`Self::Range`] aggregate's per-iteration increment.
    ///
    /// The surface has no step spelling -- `start..end` and `start..=end` are the only two forms the parser
    /// produces -- so this is always the unit step today. It is carried as a real operand rather than left
    /// implicit because a consumer otherwise has to *assume* the increment, and because it is the same `1` the
    /// `for` header's normalization already adds to its index each iteration.
    pub const RANGE_FIELD_STEP: &'static str = "step";
    /// Declared name of an [`Self::Range`] aggregate's inclusivity flag: `true` for `start..=end`, `false` for
    /// `start..end`.
    pub const RANGE_FIELD_INCLUSIVE: &'static str = "inclusive";
    /// Every [`Self::Range`] field name, in the order its operands appear in the aggregate.
    pub const RANGE_FIELDS: [&'static str; 4] = [
        Self::RANGE_FIELD_START,
        Self::RANGE_FIELD_END,
        Self::RANGE_FIELD_STEP,
        Self::RANGE_FIELD_INCLUSIVE,
    ];

    /// Compact snapshot spelling for this aggregate kind.
    pub(super) fn as_str(&self) -> String {
        match self {
            Self::Tuple => "tuple".to_string(),
            Self::OptionSome => "option_some".to_string(),
            Self::List => "list".to_string(),
            Self::Set => "set".to_string(),
            Self::Range => "range".to_string(),
            Self::Constructor(target) => format!("constructor({}){}", target.name, target.binding.render_snapshot()),
            Self::EnumVariant(target) => format!(
                "enum_variant({}::{} variant_canonical={}){}",
                target.enum_name,
                target.variant_name,
                target.variant_canonical.render_compact(),
                target.binding.render_snapshot()
            ),
        }
    }
}

// ============================================================================
// Calls and runtime helpers
// ============================================================================

/// How one call site's arguments relate to the callee's declared parameters or a nominal type's declared fields.
///
/// Body IR keeps a call's operand vector in *resolved declaration order* — declared parameter order for a callable,
/// declared field order for a nominal constructor — while the statements that compute those operands are emitted in
/// the order its binding variant records. [`Self::ResolvedDeclarationOrder`] preserves frozen dev.6 evaluation for
/// ordinary checked unqualified fixed named free calls and imported SDK type-constructor factories. The two orders
/// differ when named arguments appear out of declaration order: slots bind values, while the variant sequences effects
/// and ownership. Partial capture, iterator arguments and other written-order operations retain [`Self::Resolved`].
///
/// # Written-order ownership facts are sequenced by source position, not operand index
///
/// Each operand's [`OwnershipFact`] and last-use marker for [`Self::Resolved`] were decided while lowering the
/// arguments in written source order. Consumers must preserve that sequencing; declaration-slot placement alone
/// cannot authorize reordering their reads or moves. Earlier deferred reads can keep an owner reserved, requiring
/// a later read to clone even at its final textual use. Walk [`Self::Resolved::arguments`] by ascending
/// `written_position` when evaluating, and by operand index only when passing already-evaluated values.
///
/// This is the single binding mechanism for every call shape, replacing the per-target slot map #1124 introduced on
/// [`LocalCallableTarget`]: local callables, direct named calls, method calls, and nominal construction all record
/// their binding here, so a consumer learns the same fact the same way regardless of how the call was spelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArgumentBinding {
    /// No declared parameter slots were resolved for this call's arguments.
    ///
    /// **No declared-slot claim is made, and elements are in written source order — not slot order.** A consumer
    /// must inspect each [`ArgumentElement`] rather than assume element `i` fills parameter `i`: an element may be
    /// a single value, a value written with a name, or a spread whose length is only known at runtime. Reading this
    /// as a positional argument vector is the mistake this variant exists to prevent.
    ///
    /// A callee without a retained declared surface, or a call without a computed binding for its spread elements,
    /// makes no slot claim. A rest-bearing call with explicitly retained physical packs uses [`Self::Resolved`]
    /// instead: each owned pack occupies one declared slot.
    UnresolvedPositional,
    /// Arguments were resolved against the callee's declared parameters or the type's declared fields.
    Resolved {
        /// One record per surrounding argument operand, in the same order as those operands.
        arguments: Vec<BoundArgument>,
        /// Declared slots the call site omitted, in ascending slot order.
        ///
        /// Each names a parameter or field the call site left to a default. Body IR records the fact without
        /// materializing the value: the default's computation stays owned by whichever declaration or callable value
        /// introduced it, which is the contract [`CallableParam`] already states. For an ordinary declaration that is
        /// a source-declared [`CallableParamDefault::Source`] computation; for a partial callable it may instead be
        /// the construction-time [`CallableParamDefault::PartialPreset`] its callable parameter identifies, so a
        /// consumer reads the owning [`Rvalue::Closure`] rather than assuming the target declaration has a default of
        /// its own. Recording the omission here is still what frees a consumer from diffing the operand vector against
        /// the declaration, and it is why no defaulted slot carries an [`OwnershipFact`]: this call site evaluates
        /// nothing for it. Making those defaults evaluable is
        /// #1172's own scope, and this fact is what tells it which slots need a value.
        defaulted_slots: Vec<usize>,
    },
    /// A checked unqualified fixed named free call or imported SDK factory evaluated in frozen declaration order.
    ///
    /// The producer evaluates supplied expressions and decides ownership in ascending slot order. `written_position`
    /// retains source coordinates, but does not sequence these ownership facts. Consumers pass the already evaluated
    /// values by slot and must not reinterpret this as a written-order binding. Defaults remain owned by the
    /// selected declaration. Unchecked targets, dynamic callables and unproven spreads cannot supply this fact.
    /// The persisted wire name remains `ResolvedFactory`, preserving existing metadata and its declaration-order
    /// semantics.
    #[serde(rename = "ResolvedFactory")]
    ResolvedDeclarationOrder {
        /// One record per surrounding argument operand, in declaration order.
        arguments: Vec<BoundArgument>,
        /// Omitted declared slots, in ascending order, without an evaluated operand or ownership fact.
        defaulted_slots: Vec<usize>,
    },
}

impl ArgumentBinding {
    /// Build a resolved identity binding: operand `i` fills slot `i` and was written `i`-th, with nothing defaulted.
    ///
    /// Only for callers that actually resolved the callee's declared parameters and know the call supplies each one
    /// in order. A caller that did not resolve a signature wants [`Self::UnresolvedPositional`] instead.
    pub fn resolved_positional(count: usize) -> Self {
        Self::Resolved {
            arguments: (0..count)
                .map(|index| BoundArgument {
                    slot: index,
                    written_position: index,
                })
                .collect(),
            defaulted_slots: Vec::new(),
        }
    }

    /// Return the slot records of either resolved binding, one per surrounding argument operand in operand order.
    ///
    /// Both [`Self::Resolved`] and [`Self::ResolvedDeclarationOrder`] pair operand `i` with `arguments[i]`, so a
    /// consumer that only maps declared slots to already evaluated operands reads them the same way. A consumer that
    /// sequences evaluation or ownership must still match the variant. [`Self::UnresolvedPositional`] makes no slot
    /// claim and yields `None`.
    pub fn slot_arguments(&self) -> Option<&[BoundArgument]> {
        match self {
            Self::Resolved { arguments, .. } | Self::ResolvedDeclarationOrder { arguments, .. } => Some(arguments),
            Self::UnresolvedPositional => None,
        }
    }

    /// Render the non-trivial parts of this binding as snapshot suffixes.
    ///
    /// Slot and written-order lists are each emitted only when they differ from the identity mapping, and defaults
    /// only when the call site actually omitted something, so an ordinary positional call keeps a compact spelling
    /// and a non-trivial binding stands out. An unresolved binding always renders its own marker, so it can never be
    /// confused with a resolved identity binding.
    pub(super) fn render_snapshot(&self) -> String {
        let (arguments, defaulted_slots, declaration_order) = match self {
            Self::Resolved {
                arguments,
                defaulted_slots,
            } => (arguments, defaulted_slots, false),
            Self::ResolvedDeclarationOrder {
                arguments,
                defaulted_slots,
            } => (arguments, defaulted_slots, true),
            Self::UnresolvedPositional => return " unbound".to_string(),
        };
        let mut out = String::new();
        if declaration_order {
            out.push_str(" evaluation_order=declaration");
        }
        if arguments
            .iter()
            .enumerate()
            .any(|(index, argument)| argument.slot != index)
        {
            let slots: Vec<String> = arguments.iter().map(|argument| argument.slot.to_string()).collect();
            let _ = write!(out, " slots=[{}]", slots.join(", "));
        }
        if arguments
            .iter()
            .enumerate()
            .any(|(index, argument)| argument.written_position != index)
        {
            let written: Vec<String> = arguments
                .iter()
                .map(|argument| argument.written_position.to_string())
                .collect();
            let _ = write!(out, " written=[{}]", written.join(", "));
        }
        if !defaulted_slots.is_empty() {
            let defaults: Vec<String> = defaulted_slots.iter().map(usize::to_string).collect();
            let _ = write!(out, " defaults=[{}]", defaults.join(", "));
        }
        out
    }
}

/// One call argument's resolved declaration slot and its position in written source order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundArgument {
    /// Declared parameter or field index this operand supplies.
    pub slot: usize,
    /// Zero-based position of this argument among the call site's written arguments.
    pub written_position: usize,
}

/// Render a resolved call-site type-argument list as a snapshot suffix, or nothing when the call had none.
pub(super) fn render_type_arguments(type_args: &[IncanType]) -> String {
    if type_args.is_empty() {
        return String::new();
    }
    let rendered: Vec<String> = type_args.iter().map(IncanType::to_string).collect();
    format!("[{}]", rendered.join(", "))
}
