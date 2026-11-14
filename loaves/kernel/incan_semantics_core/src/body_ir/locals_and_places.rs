//! Locals, scopes, places, place projections and operands.

use super::*;

// ============================================================================
// Locals, scopes, places
// ============================================================================

/// Stable index of one local within a [`Body`]'s `locals` vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LocalId(pub u32);

impl LocalId {
    /// Look up this local's index for direct `locals[..]` access.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// One explicit local or temporary value declared in a body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalDecl {
    pub id: LocalId,
    /// Source-level binding name, or `None` for a compiler-introduced temporary.
    pub name: Option<String>,
    /// Canonical source identity of this binding, or `None` for compiler temporaries and explicitly unresolved
    /// recovery locals.
    pub identity: Option<Box<CanonicalSymbolId>>,
    pub ty: IncanType,
    pub origin: LocalOrigin,
    /// Lexical scope this local is declared in.
    pub scope: ScopeId,
    pub span: HirSourceSpan,
}

impl LocalDecl {
    /// Render a deterministic maintainer-facing snapshot line for this local.
    pub(super) fn render_snapshot(&self) -> String {
        let name = self.name.as_deref().unwrap_or("<tmp>");
        let identity = self
            .identity
            .as_deref()
            .map_or_else(|| "unproven".to_string(), CanonicalSymbolId::render_compact);
        format!(
            "local {} {} : {} [{}] identity={} scope={} span={}..{}",
            self.id.0,
            name,
            self.ty,
            self.origin.as_str(),
            identity,
            self.scope.0,
            self.span.start,
            self.span.end
        )
    }
}

/// Where a local came from, for diagnostics and drop-planning purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocalOrigin {
    /// Bound to a function/method parameter.
    Parameter,
    /// Bound by a source-level assignment (`x = ...`, `let x = ...`, `mut x = ...`).
    UserBinding,
    /// Introduced by lowering to hold an intermediate value (e.g. flattening a nested call or binary expression).
    Temporary,
    /// A source reference for which the resolver supplied no canonical identity. Modeled as an opaque local with
    /// [`OwnershipFact::Unknown`](crate::body_ir::OwnershipFact::Unknown) reads rather than silently treated as a
    /// resolved local, per #653's "explicit unknowns" requirement. Resolver-proven module storage uses a canonical
    /// [`PlaceRoot::Global`] instead.
    External,
    /// Bound to a method's `self`/`mut self` receiver (#1102).
    ///
    /// A receiver is always a Rust-level reference (`&self` or `&mut self`) at the emission boundary — Incan's
    /// `Receiver` AST has no "by value" variant — so it is never itself drop-relevant and a bare read of it can
    /// never soundly select [`OwnershipFact::Move`]; see the receiver carve-out in
    /// [`crate::body_ir::OwnershipFact`]'s use sites in `loaves/compiler/incan_frontend/src/body_ir.rs` for how
    /// lowering enforces that. `mutable` records `self` (`false`) vs `mut self` (`true`) purely as a descriptive
    /// fact for now — v0 does not yet use it to pick a different ownership fact at read sites, the same way
    /// parameter mutability is not tracked as an ownership-fact input either.
    Receiver {
        /// Whether the receiver was declared `mut self` (`true`) rather than plain `self` (`false`).
        mutable: bool,
    },
    /// Bound to a free variable a [`Rvalue::Closure`] or [`Rvalue::Generator`] captured from its enclosing scope,
    /// to a generator expression's construction-time first-clause source, or to a preset value a partial callable's
    /// synthesized closure captured from its own preset expression. The initial value comes from one explicit
    /// [`Operand`] read recorded on the owning rvalue at construction, not from a caller-supplied argument the way
    /// [`Self::Parameter`] locals are -- kept as its own origin rather than folded into [`Self::Parameter`] so a
    /// later consumer can tell "the caller supplied this" apart from "the value's environment supplied this."
    Captured,
}

impl LocalOrigin {
    /// Compact snapshot spelling for this origin.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Parameter => "param",
            Self::UserBinding => "binding",
            Self::Temporary => "temp",
            Self::External => "external",
            Self::Receiver { mutable: false } => "receiver",
            Self::Receiver { mutable: true } => "receiver_mut",
            Self::Captured => "captured",
        }
    }
}

/// Stable index of one lexical scope within a [`Body`]'s `scopes` vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScopeId(pub u32);

/// One lexical source scope, used to map statements and locals back to diagnostics-relevant source structure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeInfo {
    pub id: ScopeId,
    /// Enclosing scope, or `None` for the body's root scope.
    pub parent: Option<ScopeId>,
    pub span: HirSourceSpan,
}

/// Root storage selected for a Body IR place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlaceRoot {
    /// A function-frame local selected by canonical source identity.
    Local(LocalId),
    /// A module-level storage declaration selected by canonical source identity.
    Global(GlobalPlace),
}

/// Canonical module-level storage retained in a Body IR place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalPlace {
    pub identity: CanonicalSymbolId,
    pub ty: IncanType,
    pub write_policy: GlobalWritePolicy,
}

/// Which writes a global place permits after typechecking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlobalWritePolicy {
    /// Constants cannot be written at either their root or through a projection.
    ReadOnly,
    /// An imported static permits mutation through a projection but cannot be rebound in this module.
    ProjectionOnly,
    /// A static declared by this module can be rebound and mutated through projections.
    Rebindable,
}

/// A place in memory: a canonical local/global root plus zero or more projections (field/index) into it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub root: PlaceRoot,
    pub projection: Vec<PlaceElem>,
}

impl Place {
    /// Build a bare place referring to a local with no projection.
    pub const fn from_local(local: LocalId) -> Self {
        Self {
            root: PlaceRoot::Local(local),
            projection: Vec::new(),
        }
    }

    /// Build a bare place referring to canonical module storage with no projection.
    pub fn from_global(global: GlobalPlace) -> Self {
        Self {
            root: PlaceRoot::Global(global),
            projection: Vec::new(),
        }
    }

    /// Return the local root when this place belongs to the current execution frame.
    pub const fn local_id(&self) -> Option<LocalId> {
        match &self.root {
            PlaceRoot::Local(local) => Some(*local),
            PlaceRoot::Global(_) => None,
        }
    }

    /// Return canonical module storage when this place is global.
    pub const fn global(&self) -> Option<&GlobalPlace> {
        match &self.root {
            PlaceRoot::Local(_) => None,
            PlaceRoot::Global(global) => Some(global),
        }
    }

    /// Return whether this place is a legal write target under its compiler-recorded storage policy.
    pub fn permits_write(&self) -> bool {
        match &self.root {
            PlaceRoot::Local(_) => true,
            PlaceRoot::Global(global) => match global.write_policy {
                GlobalWritePolicy::ReadOnly => false,
                GlobalWritePolicy::ProjectionOnly => !self.projection.is_empty(),
                GlobalWritePolicy::Rebindable => true,
            },
        }
    }

    /// Render a deterministic maintainer-facing spelling for this place.
    pub(super) fn render_snapshot(&self) -> String {
        let mut out = match &self.root {
            PlaceRoot::Local(local) => format!("_{}", local.0),
            PlaceRoot::Global(global) => format!("@{}", global.identity.render_compact()),
        };
        for elem in &self.projection {
            match elem {
                PlaceElem::Field { name, .. } => {
                    let _ = write!(&mut out, ".{name}");
                }
                PlaceElem::UnionMember { ty } => {
                    let _ = write!(&mut out, ".union[{ty}]");
                }
                PlaceElem::OptionPayload {
                    option_type,
                    payload_type,
                } => {
                    let _ = write!(&mut out, ".some[{option_type}->{payload_type}]");
                }
                PlaceElem::Index(operand) => {
                    let _ = write!(&mut out, "[{}]", operand.render_snapshot());
                }
                PlaceElem::Slice { start, end, step } => {
                    let start = start.as_ref().map(|o| o.render_snapshot()).unwrap_or_default();
                    let end = end.as_ref().map(|o| o.render_snapshot()).unwrap_or_default();
                    match step {
                        Some(step) => {
                            let _ = write!(&mut out, "[{start}:{end}:{}]", step.render_snapshot());
                        }
                        None => {
                            let _ = write!(&mut out, "[{start}:{end}]");
                        }
                    }
                }
            }
        }
        out
    }
}

/// One projection step applied to a place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlaceElem {
    /// `.field` access, with the selected source member identity when the projection came from checked source.
    ///
    /// Compiler-synthesized tuple/range/protocol projections have an explicit structural marker. A missing source
    /// identity alone is never permission to resolve a nominal member by name or publish a covered body.
    Field {
        /// Written or compiler-synthesized field spelling retained for diagnostics and physical layout selection.
        name: String,
        /// Canonical member selected by typechecking, independent of the source spelling.
        canonical: Option<CanonicalSymbolId>,
        /// Whether checked tuple/range/protocol lowering proved a structural projection without a nominal member.
        #[serde(default)]
        structural: bool,
    },
    /// `[index]` access. Boxed because the index itself is an arbitrary operand.
    Index(Box<Operand>),
    /// `[start:end:step]` slice access, mirroring `ast::SliceExpr`'s shape: each component is independently
    /// optional (`x[:5]`, `x[2:]`, `x[::2]`, `x[:]`, ...). Boxed for the same reason as [`PlaceElem::Index`] --
    /// each component is an arbitrary operand, not a compile-time constant.
    Slice {
        start: Option<Box<Operand>>,
        end: Option<Box<Operand>>,
        step: Option<Box<Operand>>,
    },
    /// A union payload selected by the checker's flow-narrowed expression type. The backend resolves its normalized
    /// variant slot; it must not infer narrowing from enclosing control flow.
    UnionMember {
        /// Exact alias-expanded member type proved at this source read.
        ty: IncanType,
    },
    /// A read whose checked narrowed type proves the retained Option stores its Some payload. Container and payload
    /// types are both retained so publication and lowering validate this step rather than infer it from control flow.
    OptionPayload {
        /// Exact checked type of the physical Option storage before this projection.
        option_type: IncanType,
        /// Exact checked type of its Some payload before any subsequent union-member projection.
        payload_type: IncanType,
    },
}

impl PlaceElem {
    /// Build a source-owned field projection from the identity selected by typechecking.
    pub fn field(name: impl Into<String>, canonical: Option<CanonicalSymbolId>) -> Self {
        Self::Field {
            name: name.into(),
            canonical,
            structural: false,
        }
    }

    /// Build a projection proved by checked structural type or protocol lowering, with no nominal member identity.
    pub fn structural_field(name: impl Into<String>) -> Self {
        Self::Field {
            name: name.into(),
            canonical: None,
            structural: true,
        }
    }
}

// ============================================================================
// Operands and ownership facts
// ============================================================================

/// A value used as input to an [`Rvalue`], call argument, branch condition, or return value.
///
/// Every place-read carries its own [`OwnershipFact`] and last-use marker directly (see [`PlaceOperand`]) — this is
/// the Duckborrower fact for that read, exposed as compiler-owned data rather than left for a backend to re-derive
/// from lowered code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Operand {
    /// A read of a place, annotated with its ownership decision.
    Place(PlaceOperand),
    /// A literal constant value.
    Constant(Constant),
}

impl Operand {
    /// Build a place-read operand with an explicit ownership fact and last-use marker.
    pub const fn place(place: Place, fact: OwnershipFact, last_use: bool) -> Self {
        Self::Place(PlaceOperand { place, fact, last_use })
    }

    /// Render a deterministic maintainer-facing spelling for this operand.
    pub(super) fn render_snapshot(&self) -> String {
        match self {
            Self::Place(op) => format!(
                "{}({}{})",
                op.fact.as_str(),
                op.place.render_snapshot(),
                if op.last_use { ", last_use" } else { "" }
            ),
            Self::Constant(c) => c.render_snapshot(),
        }
    }
}

/// A place-read operand paired with its Duckborrower ownership fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceOperand {
    pub place: Place,
    /// The ownership decision selected for this read.
    pub fact: OwnershipFact,
    /// Whether this is statically the last read of `place`'s local within its declaring scope. `fact` is only ever
    /// `Move` when this is `true` for a non-Copy type; the flag is retained separately (rather than folded
    /// implicitly into `fact`) because #653 names last-use as its own explicit fact, independent of which decision
    /// it produced.
    pub last_use: bool,
}
