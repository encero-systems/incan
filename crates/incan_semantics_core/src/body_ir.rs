//! Incan Body IR v0 data model.
//!
//! Body IR v0 is the backend-facing, target-agnostic representation of one function/method body. It sits between
//! typechecked source (AST + [`crate::SemanticFactStore`] / declaration-level [`crate::HirModule`]) and the
//! target-specific backend lowering under `src/backend/ir/` — it must be consumable by a replacement backend without
//! that backend needing to read generated-Rust semantics or private compiler internals.
//!
//! Body IR v0 deliberately models a **normalized** statement/control-flow vocabulary rather than a flattened
//! basic-block CFG: `while`/`for` desugar into a single canonical `Loop` + conditional `Break` shape during
//! lowering, and every place-read carries its own [`OwnershipFact`] plus [`bool`] last-use marker inline, instead of
//! relying on a separate borrow-checker-shaped analysis pass. This keeps the model close to what a v0 slice can
//! compute and verify deterministically, while leaving full CFG flattening, precise per-path drop dataflow, and a
//! committed panic strategy to later, more optimizer-shaped work (explicitly out of scope for #653).
//!
//! Unsupported source constructs lower to an explicit [`StatementKind::Unsupported`] node rather than panicking or
//! being silently dropped, so the model stays total over real programs while being honest about v0's coverage.
//! Generator expressions lower to [`Rvalue::Generator`], preserving their established iterator-adapter timing: the
//! outer source expression is evaluated at construction, while polling, later clause sources, filters, and elements
//! remain inside the owned [`GeneratorBody`]. Expression-position `yield` remains a stub in the existing
//! Rust-emission backend too.
//!
//! #1101 adds dict/set aggregates, slices, assignment variants, expression-position `if`/`loop`/`try`, f-strings,
//! general iteration, list/dict comprehensions, closures/partial construction and local invocation, statement
//! `yield`, generator expressions, and `match`. Captures are explicit [`Operand`] reads on
//! [`Rvalue::Closure`]/[`Rvalue::Generator`], and a stored callable is invoked through [`CallableTarget::Local`]
//! rather than being approximated as a named function, so neither fact is an implicit target-backend decision.
//!
//! "Method body" includes `class`/`model`/`trait` methods (#1102). A `self`/`mut self` receiver is an ordinary
//! [`LocalOrigin::Receiver`] local and is always reference-shaped at the emission boundary, so a bare receiver
//! read can never select [`OwnershipFact::Move`].
//!
//! #1158 makes a call site's argument binding explicit. A call's operand vector is in resolved declaration order --
//! declared parameter order for a callable, declared field order for a nominal constructor -- while the statements
//! computing those operands stay in written source order, and [`ArgumentBinding`] records both facts plus the slots
//! a call site left to their declared defaults. One binding type serves direct calls, local callable values, method
//! calls, and construction, so a consumer reads the same fact the same way regardless of the spelling. A default's
//! *value* is deliberately not materialized at the call site: its computation stays owned by the declaration that
//! introduced it, matching the contract [`CallableParam`] already states. Resolved explicit call-site type arguments
//! ride on the callee target rather than the argument list, because they are part of which callable was selected
//! rather than what it was passed.

use std::fmt::Write as _;

use crate::{AbiV0RuntimeRequirement, CompilerNodeId, HirSourceSpan, IncanType};

// ============================================================================
// Module / body containers
// ============================================================================

/// One module's lowered function/method bodies and direct-execution declaration facts.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyIrModule {
    /// Identity of the owning module, matching [`crate::HirModule::id`].
    pub module_id: CompilerNodeId,
    /// Source-local plain-model declarations whose construction layout is available to a direct runtime.
    ///
    /// This is not a general nominal symbol table. It contains only the source-local model declarations lowering
    /// has explicitly retained for direct execution, and each record carries its declaration-span identity and
    /// canonical field order. A consumer must resolve a [`ConstructorTarget::direct_declaration_id`] against this
    /// list rather than recovering a constructor identity from a source spelling, import, alias, or typechecker
    /// lookup. Models outside this deliberately narrow value profile are absent and must refuse.
    pub nominal_declarations: Vec<NominalDeclaration>,
    /// Source-local RFC 032 value-enum declarations whose canonical scalar members are available to direct execution.
    ///
    /// This is deliberately separate from [`Self::nominal_declarations`]: a value enum has no model field layout,
    /// and the direct runtime may extract only a retained member's scalar backing through the compiler-provided
    /// zero-argument `.value()` surface. Ordinary enums, payload variants, aliases, imports, behavior-bearing
    /// enums, and generic enums are absent and must refuse rather than being rediscovered by spelling.
    pub value_enum_declarations: Vec<ValueEnumDeclaration>,
    /// One [`Body`] per lowered function/method declaration in the module.
    pub bodies: Vec<Body>,
}

impl BodyIrModule {
    /// Render a deterministic maintainer-facing snapshot of every body in the module.
    pub fn render_snapshot(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(&mut out, "body_ir_module {}", self.module_id);
        for declaration in &self.nominal_declarations {
            let _ = writeln!(
                &mut out,
                "nominal {} id={} fields=[{}] type_params={}",
                declaration.name,
                declaration.direct_declaration_id,
                declaration.fields.join(", "),
                declaration.type_parameter_count
            );
        }
        for declaration in &self.value_enum_declarations {
            let _ = writeln!(
                &mut out,
                "value_enum {} id={} backing={} variants=[{}]",
                declaration.name,
                declaration.direct_declaration_id,
                declaration.backing.as_str(),
                declaration
                    .variants
                    .iter()
                    .map(ValueEnumVariantDeclaration::render_snapshot)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        for body in &self.bodies {
            out.push_str(&body.render_snapshot());
        }
        out
    }
}

/// The exact local declaration and canonical field layout for one direct-executable plain model.
///
/// The record is module-scoped and deliberately excludes classes, enums, imported nominals, generic models, and
/// behavior-bearing models. Its field order is the checked constructor-slot order; a direct runtime must pair it
/// with [`ConstructorTarget::binding`] rather than treating constructor argument spelling as layout evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NominalDeclaration {
    /// Exact source-local declaration identity, derived from the declaration source span.
    pub direct_declaration_id: CompilerNodeId,
    /// Canonical source declaration name, checked again by consumers as a defence against malformed Body IR.
    pub name: String,
    /// Canonical declared field names in declaration order.
    pub fields: Vec<String>,
    /// Number of declared type parameters; this profile admits only zero.
    pub type_parameter_count: usize,
}

/// The scalar backing category a retained RFC 032 value enum exposes through `.value()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueEnumBacking {
    /// An integer-backed value enum.
    Int,
    /// A string-backed value enum.
    Str,
}

impl ValueEnumBacking {
    /// Render the canonical source-level scalar type name for snapshots and diagnostics.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Str => "str",
        }
    }
}

/// One source-local RFC 032 value enum retained for direct scalar extraction.
///
/// The record exists only for undecorated, non-generic declarations with no traits, methods, aliases, or payload
/// variants. Each member has its own source-span identity, so a direct executor verifies the actual selected member
/// without treating a qualified `Enum.Member` spelling as an identity.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueEnumDeclaration {
    /// Exact source-local enum declaration identity, derived from its declaration span.
    pub direct_declaration_id: CompilerNodeId,
    /// Canonical source declaration name.
    pub name: String,
    /// The only scalar carrier exposed by the admitted generated `.value()` operation.
    pub backing: ValueEnumBacking,
    /// Canonical value-enum members in source declaration order.
    pub variants: Vec<ValueEnumVariantDeclaration>,
}

/// One canonical scalar member of a retained source-local value enum.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueEnumVariantDeclaration {
    /// Exact source-local variant declaration identity, derived from its declaration span.
    pub direct_declaration_id: CompilerNodeId,
    /// Canonical member name; aliases are intentionally not retained.
    pub name: String,
    /// The checked source literal exposed only through identity-validated `.value()` extraction.
    pub raw_value: Constant,
}

impl ValueEnumVariantDeclaration {
    /// Render the member's identity and raw scalar fact for a deterministic module snapshot.
    fn render_snapshot(&self) -> String {
        format!(
            "variant {} id={} raw={}",
            self.name,
            self.direct_declaration_id,
            self.raw_value.render_snapshot()
        )
    }
}

/// Body IR v0 for a single function or method.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    /// Identity of the owning declaration, matching the [`crate::HirDeclaration::id`] this body was lowered from.
    pub decl_id: CompilerNodeId,
    /// Exact source-local identity used to dispatch a direct named Body-IR call.
    ///
    /// [`Self::decl_id`] intentionally stays correlated with declaration-level HIR, whose named identity is useful
    /// to existing consumers but is not collision-safe for same-name overloads. This span-based declaration identity
    /// is retained separately so a direct executor can select precisely the declaration the typechecker chose,
    /// without reconstructing name resolution or consulting generated Rust. It is always scoped to this
    /// [`BodyIrModule`] and must never stand in for an imported callable identity.
    pub direct_call_id: CompilerNodeId,
    /// Source-level function/method name.
    pub name: String,
    /// Full source span of the declaration this body was lowered from.
    pub span: HirSourceSpan,
    /// Every local (parameter, user binding, or compiler-introduced temporary) declared in this body, in
    /// declaration order. Referenced elsewhere by [`LocalId`] index.
    pub locals: Vec<LocalDecl>,
    /// The callable contract for every parameter, in declaration order.
    ///
    /// Unlike `param_locals`, this keeps the direct-consumer binding contract: parameter identity, source span,
    /// resolved type, and whether omission requires a call-time computation, uses a construction-time partial
    /// preset, or must refuse because v0 cannot evaluate the source default. A target must consume this field rather
    /// than reconstructing defaults from AST/HIR/typechecker state or generated Rust.
    pub params: Vec<CallableParam>,
    /// Locals bound to this body's parameters, in parameter order.
    ///
    /// Kept as a compatibility projection for pre-#1172 consumers that only support required arguments. New
    /// consumers must use `params` so omission behavior stays visible and fail-closed.
    pub param_locals: Vec<LocalId>,
    /// Source scope tree for this body, used to map statements/locals back to lexical scopes for diagnostics.
    pub scopes: Vec<ScopeInfo>,
    /// Normalized statement sequence for the body, rooted at the function's top-level block.
    pub block: Block,
    /// Runtime/helper requirements this body imposes, deduplicated and kept in first-seen (source-order) sequence
    /// for deterministic snapshots — [`crate::types::AbiV0RuntimeRequirement`] does not derive `Ord`, so lowering
    /// relies on deterministic traversal order rather than sorting. Lets a later target profile decide
    /// hosted-only / alloc-requiring / panic-requiring / potentially freestanding-compatible without inferring
    /// facts from generated Rust helper calls.
    pub runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    /// Panic-interaction facts recorded for this body, without committing to a stable public panic strategy.
    pub panic_facts: Vec<PanicFact>,
    /// Whether the source declaration was marked `async` (#1164).
    ///
    /// Deliberately a **stored** field, unlike the derived [`Self::is_generator`] next to it. Async-ness is a
    /// declaration-level fact: an `async def` with no `await` in its body is still async, and its caller still gets
    /// an awaitable. Deriving this by scanning for [`StatementKind::Await`] the way `is_generator` scans for
    /// [`StatementKind::Yield`] would therefore be wrong, not merely a different implementation. This is the first
    /// declaration-level fact `Body` carries; see [`Self::is_generator`]'s docs for why that one stayed derived.
    pub is_async: bool,
}

impl Body {
    /// Return the locals in this body whose type is not [`crate::types::AbiV0Ownership::CopyOrTrivial`] and are
    /// therefore drop-relevant if a panic unwinds through this body.
    ///
    /// This is a **conservative over-approximation**: it returns every non-Copy local in the body rather than the
    /// precise set still live at a specific panic site, because computing the precise per-site set needs full
    /// control-flow dataflow that is out of scope for v0 (see the module-level docs). Callers that need a stable
    /// panic strategy must not treat this as a final drop plan; it only exposes that such locals exist.
    ///
    /// [`LocalOrigin::Receiver`] locals are excluded unconditionally, regardless of their type's Copy-ness: `self`/
    /// `mut self` is always a Rust-level reference at the emission boundary, and references have no destructor of
    /// their own to run on unwind — only the value a reference points at might, and that value is owned by the
    /// method's caller, not by this body.
    pub fn locals_requiring_unwind_drop(&self) -> Vec<LocalId> {
        self.locals
            .iter()
            .filter(|local| !matches!(local.origin, LocalOrigin::Receiver { .. }))
            .filter(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
            .map(|local| local.id)
            .collect()
    }

    /// Whether this body is a generator body: it contains at least one statement-position `yield value`
    /// ([`StatementKind::Yield`]) reachable from its top-level block.
    ///
    /// This is a **derived** fact, walked from the already-lowered statement tree, rather than a flag stored
    /// redundantly on `Body` -- mirroring how the existing Rust-emission backend computes its own `is_generator`
    /// boolean at lowering time (`return_type_is_generator(&return_type) && body_contains_yield(&f.body)` in
    /// `src/backend/ir/lower/decl/functions.rs`) rather than threading a separate stored flag through its own IR.
    /// Unlike that backend function, this does not also fold in a return-type check: `Generator[T]` is
    /// declaration-level information a `Body` alone does not carry, so a caller that has the owning declaration in
    /// hand should combine the two the same way the existing backend does. In practice a well-typed program's
    /// `yield` only ever appears inside a function whose declared return type is `Generator[T]` (the typechecker
    /// enforces this), so this alone is already a reliable generator signal for a `Body` in isolation.
    ///
    /// The walk recurses into every statement kind that carries a nested [`Block`] --
    /// [`StatementKind::If`], [`StatementKind::Loop`], and each [`StatementKind::Race`] arm -- but does not recurse
    /// into a [`Rvalue::Closure`]'s
    /// [`ClosureBody`] or a [`Rvalue::Generator`]'s [`GeneratorBody`]. A yield nested in either value does not make
    /// the *enclosing* function body a generator, matching how the existing backend's own `body_contains_yield`
    /// walker never descends into nested closure/function literals.
    pub fn is_generator(&self) -> bool {
        block_contains_yield(&self.block)
    }

    /// Render a deterministic maintainer-facing snapshot of this body.
    pub fn render_snapshot(&self) -> String {
        let mut out = String::new();
        let async_marker = if self.is_async { " async" } else { "" };
        let _ = writeln!(
            &mut out,
            "body{async_marker} {} {} span={}..{}",
            self.name, self.decl_id, self.span.start, self.span.end
        );
        for local in &self.locals {
            let _ = writeln!(&mut out, "  {}", local.render_snapshot());
        }
        if !self.params.is_empty() {
            let _ = writeln!(&mut out, "  params:");
            for param in &self.params {
                let _ = writeln!(&mut out, "    {}", param.render_snapshot());
            }
        }
        render_block(&mut out, &self.block, 1);
        if !self.runtime_requirements.is_empty() {
            let _ = writeln!(&mut out, "  runtime_requirements:");
            for req in &self.runtime_requirements {
                let _ = writeln!(&mut out, "    {}", render_runtime_requirement(req));
            }
        }
        if !self.panic_facts.is_empty() {
            let _ = writeln!(&mut out, "  panic_facts:");
            for fact in &self.panic_facts {
                let _ = writeln!(&mut out, "    {}", fact.render_snapshot());
            }
        }
        out
    }
}

/// Whether any statement in `block`, or in a block nested under one of its `If`/`Loop` statements, is a
/// [`StatementKind::Yield`]. Backs [`Body::is_generator`]; see that method's docs for why this does not recurse
/// into nested closure bodies.
fn block_contains_yield(block: &Block) -> bool {
    block.stmts.iter().any(statement_contains_yield)
}

/// Whether `stmt` is itself a [`StatementKind::Yield`], or contains one in a nested `If`/`Loop` block. See
/// [`block_contains_yield`].
fn statement_contains_yield(stmt: &Statement) -> bool {
    match &stmt.kind {
        StatementKind::Yield { .. } => true,
        StatementKind::If {
            then_block, else_block, ..
        } => block_contains_yield(then_block) || else_block.as_ref().is_some_and(block_contains_yield),
        StatementKind::Loop { body } => block_contains_yield(body),
        // A race arm body is a nested block like any other. A body cannot currently be both async and a generator,
        // so this is unreachable today; walking it anyway keeps the traversal total over the statement vocabulary
        // rather than silently under-reporting if that ever changes.
        StatementKind::Race { arms, .. } => arms.iter().any(|arm| block_contains_yield(&arm.body)),
        _ => false,
    }
}

/// Render one [`AbiV0RuntimeRequirement`] using a stable, deterministic spelling.
///
/// [`AbiV0RuntimeRequirement`] does not derive `Display`, so Body IR renders it locally rather than depending on
/// `{:?}` output remaining stable across unrelated changes to that enum's derive list.
fn render_runtime_requirement(req: &AbiV0RuntimeRequirement) -> String {
    match req {
        AbiV0RuntimeRequirement::RuntimeHelper(name) => format!("runtime_helper({name})"),
        AbiV0RuntimeRequirement::HostedStd => "hosted_std".to_string(),
        AbiV0RuntimeRequirement::Allocator => "allocator".to_string(),
        AbiV0RuntimeRequirement::PanicStrategy => "panic_strategy".to_string(),
        AbiV0RuntimeRequirement::AsyncRuntime => "async_runtime".to_string(),
    }
}

// ============================================================================
// Locals, scopes, places
// ============================================================================

/// Stable index of one local within a [`Body`]'s `locals` vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalId(pub u32);

impl LocalId {
    /// Look up this local's index for direct `locals[..]` access.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// One explicit local or temporary value declared in a body.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalDecl {
    pub id: LocalId,
    /// Source-level binding name, or `None` for a compiler-introduced temporary.
    pub name: Option<String>,
    pub ty: IncanType,
    pub origin: LocalOrigin,
    /// Lexical scope this local is declared in.
    pub scope: ScopeId,
    pub span: HirSourceSpan,
}

impl LocalDecl {
    /// Render a deterministic maintainer-facing snapshot line for this local.
    fn render_snapshot(&self) -> String {
        let name = self.name.as_deref().unwrap_or("<tmp>");
        format!(
            "local {} {} : {} [{}] scope={} span={}..{}",
            self.id.0,
            name,
            self.ty,
            self.origin.as_str(),
            self.scope.0,
            self.span.start,
            self.span.end
        )
    }
}

/// Where a local came from, for diagnostics and drop-planning purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalOrigin {
    /// Bound to a function/method parameter.
    Parameter,
    /// Bound by a source-level assignment (`x = ...`, `let x = ...`, `mut x = ...`).
    UserBinding,
    /// Introduced by lowering to hold an intermediate value (e.g. flattening a nested call or binary expression).
    Temporary,
    /// A name lowering could not resolve to a parameter or local binding within this body (for example a
    /// module-level `const`/`static`, or a reference lowering does not yet track). Modeled as an opaque local with
    /// [`OwnershipFact::Unknown`](crate::body_ir::OwnershipFact::Unknown) reads rather than silently treated as a
    /// resolved local, per #653's "explicit unknowns" requirement.
    External,
    /// Bound to a method's `self`/`mut self` receiver (#1102).
    ///
    /// A receiver is always a Rust-level reference (`&self` or `&mut self`) at the emission boundary — Incan's
    /// `Receiver` AST has no "by value" variant — so it is never itself drop-relevant and a bare read of it can
    /// never soundly select [`OwnershipFact::Move`]; see the receiver carve-out in
    /// [`crate::body_ir::OwnershipFact`]'s use sites in `src/frontend/body_ir.rs` for how lowering enforces that.
    /// `mutable` records `self` (`false`) vs `mut self` (`true`) purely as a descriptive fact for now — v0 does not
    /// yet use it to pick a different ownership fact at read sites, the same way parameter mutability is not
    /// tracked as an ownership-fact input either.
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
    const fn as_str(self) -> &'static str {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ScopeId(pub u32);

/// One lexical source scope, used to map statements and locals back to diagnostics-relevant source structure.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeInfo {
    pub id: ScopeId,
    /// Enclosing scope, or `None` for the body's root scope.
    pub parent: Option<ScopeId>,
    pub span: HirSourceSpan,
}

/// A place in memory: a local plus zero or more projections (field/index) into it.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub local: LocalId,
    pub projection: Vec<PlaceElem>,
}

impl Place {
    /// Build a bare place referring to a local with no projection.
    pub const fn from_local(local: LocalId) -> Self {
        Self {
            local,
            projection: Vec::new(),
        }
    }

    /// Render a deterministic maintainer-facing spelling for this place.
    fn render_snapshot(&self) -> String {
        let mut out = format!("_{}", self.local.0);
        for elem in &self.projection {
            match elem {
                PlaceElem::Field(name) => {
                    let _ = write!(&mut out, ".{name}");
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
#[derive(Debug, Clone, PartialEq)]
pub enum PlaceElem {
    /// `.field` access.
    Field(String),
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
}

// ============================================================================
// Operands and ownership facts
// ============================================================================

/// A value used as input to an [`Rvalue`], call argument, branch condition, or return value.
///
/// Every place-read carries its own [`OwnershipFact`] and last-use marker directly (see [`PlaceOperand`]) — this is
/// the Duckborrower fact for that read, exposed as compiler-owned data rather than left for a backend to re-derive
/// from generated Rust.
#[derive(Debug, Clone, PartialEq)]
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
    fn render_snapshot(&self) -> String {
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
#[derive(Debug, Clone, PartialEq)]
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

/// A literal constant operand value.
#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Int(i64),
    Float(String),
    Bool(bool),
    Str(String),
    Unit,
    None,
}

impl Constant {
    /// Render a deterministic maintainer-facing spelling for this constant.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Int(v) => format!("const({v})"),
            Self::Float(v) => format!("const({v})"),
            Self::Bool(v) => format!("const({v})"),
            Self::Str(v) => format!("const({v:?})"),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    const fn as_str(self) -> &'static str {
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
#[derive(Debug, Clone, PartialEq)]
pub enum Rvalue {
    /// Use an operand's value directly.
    Use(Operand),
    UnaryOp(UnOp, Operand),
    BinaryOp(BinOp, Operand, Operand),
    /// Build a tuple, list, or nominal-constructor value from its element/field operands.
    Aggregate(AggregateKind, Vec<Operand>),
    /// Materialize one exact source-local RFC 032 value-enum member.
    ///
    /// This stores declaration identities rather than a raw scalar so a direct runtime can revalidate enum/member
    /// membership against [`BodyIrModule::value_enum_declarations`] before the compiler-provided `.value()` method
    /// exposes the literal. It never represents an ordinary enum, payload variant, alias, import, or Result value.
    ValueEnumVariant(ValueEnumVariantTarget),
    /// An f-string interpolation, built from a sequence of literal text chunks and already-lowered embedded
    /// expressions. Mirrors the existing Rust-emission backend's dedicated `IrExprKind::Format { parts }` node
    /// (`src/backend/ir/expr.rs`) rather than a helper-call desugar: an f-string is a compiler-owned structured
    /// value, not something inferred later from a generated Rust call shape (#653 criterion 3), so it gets its own
    /// `Rvalue` shape just like the existing backend gives it its own `IrExprKind` shape.
    Format(Vec<FormatPart>),
    /// A closure literal (`(params) => expr`), or a partial callable's synthesized forwarding closure (`partial
    /// Target(presets)`) -- see `src/frontend/body_ir.rs`'s `BodyBuilder::lower_partial`.
    ///
    /// Unlike the existing Rust-emission backend's `IrExprKind::Closure` (whose `captures: Vec<String>` field is
    /// always populated empty at both of that backend's own lowering call sites -- it relies entirely on Rust's own
    /// closure syntax plus rustc's borrow checker to work out by-value/by-reference capture), Body IR represents
    /// every capture explicitly: representing Duckborrower ownership facts rather than deferring to
    /// generated-Rust semantics is this IR's entire reason to exist (#653), so a closure capturing an outer
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
    /// Mirrors the existing Rust-emission backend's own `IrExprKind::Match { scrutinee, arms }` node
    /// (`src/backend/ir/expr.rs`): that backend has already reduced Incan's match-pattern surface to the small,
    /// closed vocabulary [`Pattern`] mirrors (see its own docs), and compiles each arm's pattern directly into a
    /// native Rust `match` arm, letting rustc perform exhaustiveness checking and the actual destructuring/dispatch
    /// itself. Matching the same #653-criterion-3 "compiler-owned semantic gets its own explicit node" treatment as
    /// [`Self::Format`]/[`StatementKind::TryPropagate`]/[`StatementKind::IterNext`], `match` stays a single
    /// structured `Rvalue` here too rather than being decomposed into a chain of `If` statements: decomposing it
    /// would mean re-deriving the same destructuring/dispatch logic a target backend's native `match` already
    /// gives for free, and would lose the direct correspondence with the existing backend's own `Pattern`
    /// vocabulary this model is built to mirror.
    Match {
        /// The value being matched. Always read as [`OwnershipFact::Borrow`]: more than one arm's pattern bindings
        /// may read from the scrutinee across the arm list (only one arm actually runs at a time, but see
        /// `BodyBuilder::lower_match` in `src/frontend/body_ir.rs` for why this model's last-use approximation does
        /// not attempt per-arm-exclusive dataflow), so treating this top-level read as an unconditional move would
        /// risk being unsound for whichever arm ends up executing. Each pattern binding computes its own, more
        /// precise ownership fact separately (see [`Pattern::Var`]/[`PatternBinding`]) -- a *nested*
        /// (`Tuple`/`Struct`/`Enum`-projected) binding can never disagree with this field, since a projected read
        /// is never a move (see [`PatternBinding::fact`]'s own docs), but a *root-level* `Pattern::Var`/wildcard
        /// binding that captures the scrutinee's whole value can legitimately select
        /// [`OwnershipFact::Move`]/[`OwnershipFact::Clone`] for that one arm. This field's own `Borrow` and such a
        /// root binding's `Move` are not reconciled against each other here -- a target backend that sees a
        /// root-level `Move`/`Clone` binding in some arm must match the scrutinee by value (or clone it) rather
        /// than by the reference this field's `Borrow` would otherwise suggest, the same kind of cross-fact
        /// reconciliation v0 already leaves to later work elsewhere (see the module-level docs).
        scrutinee: Operand,
        /// Every arm, in source order. The first arm whose pattern matches and whose guard (if any) is truthy runs.
        /// Incan's typechecker enforces match exhaustiveness ahead of lowering (`check_match_exhaustiveness` in
        /// `src/frontend/typechecker/check_expr/match_.rs`), so Body IR itself does not need to model a fallthrough
        /// "no arm matched" case.
        arms: Vec<MatchArm>,
    },
}

impl Rvalue {
    /// Render a deterministic maintainer-facing spelling for this rvalue.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Use(op) => op.render_snapshot(),
            Self::UnaryOp(op, operand) => format!("{}{}", op.as_str(), operand.render_snapshot()),
            Self::BinaryOp(op, lhs, rhs) => {
                format!("{} {} {}", lhs.render_snapshot(), op.as_str(), rhs.render_snapshot())
            }
            Self::Aggregate(AggregateKind::Dict, operands) => {
                // `AggregateKind::Dict`'s operands alternate key, value, key, value, ...; render as `k: v` pairs
                // rather than a flat list so the snapshot stays readable for dict literals specifically.
                let pairs: Vec<String> = operands
                    .chunks(2)
                    .map(|pair| match pair {
                        [key, value] => format!("{}: {}", key.render_snapshot(), value.render_snapshot()),
                        [key] => key.render_snapshot(),
                        _ => String::new(),
                    })
                    .collect();
                format!("dict[{}]", pairs.join(", "))
            }
            Self::Aggregate(kind, operands) => {
                let items: Vec<String> = operands.iter().map(Operand::render_snapshot).collect();
                format!("{}[{}]", kind.as_str(), items.join(", "))
            }
            Self::ValueEnumVariant(target) => target.render_snapshot(),
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

/// Exact source-local enum/member identity selected for one value-enum member expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueEnumVariantTarget {
    /// Exact source-local owner enum declaration identity.
    pub enum_declaration_id: CompilerNodeId,
    /// Exact source-local member declaration identity.
    pub variant_declaration_id: CompilerNodeId,
    /// Canonical owner name, retained for malformed-Body-IR cross-checks and diagnostics.
    pub enum_name: String,
    /// Canonical member name, retained for malformed-Body-IR cross-checks and diagnostics.
    pub variant_name: String,
}

impl ValueEnumVariantTarget {
    /// Render both retained identities so snapshots cannot mistake a member spelling for its direct target fact.
    fn render_snapshot(&self) -> String {
        format!(
            "value_enum_variant({}::{} enum_id={} variant_id={})",
            self.enum_name, self.variant_name, self.enum_declaration_id, self.variant_declaration_id
        )
    }
}

/// One parameter of a top-level/method [`Body`] or [`Rvalue::Closure`].
///
/// Every callable surface uses this one representation so a direct consumer can bind top-level functions, methods,
/// and local closures by the same contract. `default` is intentionally tagged instead of using an availability bit
/// plus an optional capture: ordinary defaults are evaluated at the omitted call site, while partial presets were
/// already evaluated when the callable was constructed. A consumer must visibly refuse
/// [`CallableParamDefault::Unsupported`] at its stored source span instead of guessing from source structures or
/// silently falling back.
#[derive(Debug, Clone, PartialEq)]
pub struct CallableParam {
    /// The local that receives this argument in the callable's execution frame.
    pub local: LocalId,
    /// Source-level parameter name.
    pub name: String,
    /// Fully resolved parameter type.
    pub ty: IncanType,
    /// Original parameter span, retained for argument-binding diagnostics.
    ///
    /// The syntax AST currently retains no receiver-token range, so a synthetic method `self` parameter uses the
    /// enclosing method declaration span. All source-spelled parameters retain their own parameter span.
    pub span: HirSourceSpan,
    /// The value to use when this parameter is omitted, or the explicit reason direct evaluation is unavailable.
    pub default: CallableParamDefault,
}

impl CallableParam {
    /// Render a deterministic maintainer-facing spelling for this parameter.
    fn render_snapshot(&self) -> String {
        format!(
            "{}: {} local=_{} span={}..{}{}",
            self.name,
            self.ty,
            self.local.0,
            self.span.start,
            self.span.end,
            self.default.render_snapshot()
        )
    }
}

/// The direct-consumer binding behavior for one [`CallableParam`].
///
/// A source default owns a closed, type-fact-backed Body-IR computation and is evaluated only when the corresponding
/// argument is omitted. Lowering must use [`CallableParamDefault::Unsupported`] instead when it cannot establish
/// that contract. A partial preset instead names a closure-frame local populated from the partial's construction-time
/// capture. These variants must remain distinct: treating a preset as a source computation would evaluate it again,
/// while treating a source computation as a capture would evaluate it too early.
#[derive(Debug, Clone, PartialEq)]
pub enum CallableParamDefault {
    /// The caller must supply this argument.
    Required,
    /// A source-declared default with usable canonical type facts whose Body-IR statements and result run at the
    /// omitted call site.
    ///
    /// The computation runs in its declaration-owned default-evaluation context, before the callable execution
    /// frame receives an argument for this parameter. Its lowering deliberately refuses reads that would require a
    /// callable-local binding, so a direct consumer must execute the stored computation as-is and then bind its
    /// result to [`CallableParam::local`].
    Source(DefaultComputation),
    /// A partial-callable value captured when the partial was constructed and still overrideable by the caller.
    PartialPreset {
        /// The closure-frame local populated from the matching captured operand.
        capture: LocalId,
    },
    /// A source-declared default Body IR cannot evaluate without recreating another compiler layer.
    ///
    /// Direct consumers must refuse at `span`; they must not use a legacy fallback or report the enclosing
    /// declaration/call span instead.
    Unsupported {
        /// Original source span of the unrepresentable default expression.
        span: HirSourceSpan,
        /// Maintainer-facing reason the expression cannot be executed directly.
        description: String,
    },
}

impl CallableParamDefault {
    /// Render a deterministic maintainer-facing spelling of the omission behavior.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Required => String::new(),
            Self::Source(computation) => format!(" = source_default({})", computation.render_snapshot()),
            Self::PartialPreset { capture } => format!(" = captured(_{})", capture.0),
            Self::Unsupported { span, description } => {
                format!(
                    " = unsupported_default({description} span={}..{})",
                    span.start, span.end
                )
            }
        }
    }
}

/// A deferred Body-IR computation for one source-declared parameter default.
///
/// `stmts` are intentionally not appended to the owning callable's normal block: evaluating them there would run a
/// default even when a caller supplied an argument. A direct consumer executes this closed computation at the
/// omitted call site, before binding the callable frame's [`CallableParam::local`] argument. Any source read that
/// would require a callable-local or otherwise unrepresented lexical binding is recorded instead as
/// [`CallableParamDefault::Unsupported`].
#[derive(Debug, Clone, PartialEq)]
pub struct DefaultComputation {
    /// Full source span of the default expression.
    pub span: HirSourceSpan,
    /// Statements that must run before producing `result`.
    pub stmts: Vec<Statement>,
    /// The default expression's computed value.
    pub result: Operand,
}

impl DefaultComputation {
    /// Render a deterministic maintainer-facing spelling of the deferred computation.
    fn render_snapshot(&self) -> String {
        let flattened = render_flattened_stmts(&self.stmts);
        if flattened.is_empty() {
            format!(
                "span={}..{} result: {}",
                self.span.start,
                self.span.end,
                self.result.render_snapshot()
            )
        } else {
            format!(
                "span={}..{} {}; result: {}",
                self.span.start,
                self.span.end,
                flattened.join("; "),
                self.result.render_snapshot()
            )
        }
    }
}

/// A closure literal's or synthesized partial-callable closure's own self-contained body computation.
///
/// Deliberately lighter than [`Body`]: it carries no `decl_id`/`name`/`scopes`/`runtime_requirements`/`panic_facts`
/// of its own -- a closure is not a top-level declaration, and any runtime/panic facts its body introduces are
/// folded directly into the owning [`Body`]'s own accumulated facts by lowering rather than tracked separately per
/// closure. It also reuses the *same* [`LocalId`] numbering as its owning [`Body`] rather than starting a fresh
/// local space at zero: the frontend lowering that builds this model (`src/frontend/body_ir.rs`) already keeps one
/// flat, function-wide local/binding namespace with no scope push/pop machinery (nested blocks shadow by simple
/// overwrite, restored explicitly around closure bodies specifically -- see `BodyBuilder::lower_closure`), so
/// giving each closure a separate zero-based local space would mean inventing a parallel indexing scheme just for
/// this one construct. Reusing the owning body's monotonic counter keeps every [`LocalId`] in a function globally
/// unique and lets [`Rvalue::Closure`]'s [`CallableParam::local`] values and [`Self::capture_locals`] simply index
/// into the same [`Body::locals`] the rest of the function uses, so a closure's own parameters and captures show up
/// in the ordinary `locals:` listing like any other local.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureBody {
    /// The closure's own captured-binding locals, in the same order as [`Rvalue::Closure::captured_operands`] --
    /// `capture_locals[i]` is where a read of the `i`-th captured operand's value is durably bound inside the
    /// closure body, so subsequent reads inside the body see it as an ordinary local rather than re-reading the
    /// enclosing body's place directly.
    pub capture_locals: Vec<LocalId>,
    /// Statements needed to compute `result`.
    pub stmts: Vec<Statement>,
    /// The closure body expression's value.
    pub result: Operand,
}

impl ClosureBody {
    /// Render a deterministic maintainer-facing spelling for this closure body, flattening its (possibly
    /// multi-line) statement rendering onto one `; `-joined line so it nests inside the single-line
    /// [`Rvalue::render_snapshot`] output the same way every other `Rvalue` variant does.
    fn render_snapshot(&self) -> String {
        let flattened = render_flattened_stmts(&self.stmts);
        if flattened.is_empty() {
            format!("result: {}", self.result.render_snapshot())
        } else {
            format!("{}; result: {}", flattened.join("; "), self.result.render_snapshot())
        }
    }
}

/// The deferred, suspendable body of one [`Rvalue::Generator`].
///
/// Like [`ClosureBody`], this reuses its owning [`Body`]'s flat local-id namespace. `source_local` receives the
/// rvalue's construction-time [`Rvalue::Generator::source`] value. `capture_locals[i]` receives the matching
/// `captured_operands[i]` value, so no statement in `stmts` reaches back into the enclosing body's places when the
/// generator is later polled. Clause bindings and iterator temporaries are ordinary locals in `stmts`; they become
/// live only while the corresponding deferred loops execute.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratorBody {
    /// Local holding the eagerly evaluated first-clause source value.
    pub source_local: LocalId,
    /// Locals holding the rvalue's construction-time lexical captures, in operand order.
    pub capture_locals: Vec<LocalId>,
    /// Deferred iteration, filtering, and [`StatementKind::Yield`] operations.
    pub stmts: Vec<Statement>,
}

impl GeneratorBody {
    /// Render a deterministic maintainer-facing spelling of the deferred body for nested rvalue snapshots.
    fn render_snapshot(&self) -> String {
        let flattened = render_flattened_stmts(&self.stmts);
        if flattened.is_empty() {
            "deferred: <empty>".to_string()
        } else {
            format!("deferred: {}", flattened.join("; "))
        }
    }
}

/// Render `stmts` at zero indentation and split into trimmed, non-empty lines, for embedding a (possibly
/// multi-statement) nested block into a single-line snapshot alongside a trailing `result: ...`/`if ...` segment.
/// Shared by [`ClosureBody::render_snapshot`], [`GeneratorBody::render_snapshot`], and
/// [`MatchArm::render_snapshot`], which all need to flatten a nested [`Block`]-shaped computation the same way.
fn render_flattened_stmts(stmts: &[Statement]) -> Vec<String> {
    let mut body = String::new();
    for stmt in stmts {
        render_statement(&mut body, stmt, "", 0);
    }
    body.lines().map(str::trim).map(str::to_string).collect()
}

/// One arm of a [`Rvalue::Match`].
#[derive(Debug, Clone, PartialEq)]
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
    /// always resolves to [`Constant::Unit`], mirroring the existing Rust-emission backend's own
    /// `IrExprKind::Block { stmts, value: None }` treatment of the same shape
    /// (`src/backend/ir/lower/expr/patterns.rs`'s `lower_match_arms`).
    pub result: Operand,
}

impl MatchArm {
    /// Render a deterministic maintainer-facing spelling for this arm, flattening `guard_stmts`/`body_stmts` onto
    /// one `; `-joined segment each so the whole arm nests inside [`Rvalue::render_snapshot`]'s single-line output.
    fn render_snapshot(&self) -> String {
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
/// Mirrors the existing Rust-emission backend's own closed `Pattern` vocabulary (`src/backend/ir/expr.rs`) almost
/// exactly -- see #1101's B6 pre-intake in `plan.md` for why this vocabulary is already small and closed rather
/// than something this bucket needed to design from scratch: the existing backend compiles each variant here
/// directly into the matching native Rust pattern syntax and lets rustc itself do the actual destructuring/
/// dispatch, so a target backend consuming this model can do the same. The one deliberate divergence from the
/// existing backend's vocabulary is [`Self::Var`]: the existing backend's `Pattern::Var(String)` carries a bare
/// source name (that backend's own separate, string-keyed scope tracks what it resolves to), while this model's
/// [`PatternBinding`] carries an already-declared [`LocalId`] plus the Duckborrower fact/last-use marker for
/// reading that part of the scrutinee -- consistent with #653's requirement that ownership decisions be
/// represented as explicit facts on the model itself, not deferred to a target backend's own name resolution.
///
/// v0 does not model the existing backend's union-type pattern narrowing (matching one member of a source `Union`
/// type against a target's own narrower union subset, rewriting the pattern and synthesizing extra arms --
/// `lower_narrowed_union_capture_arms`/`union_pattern_target` in `src/backend/ir/lower/expr/patterns.rs`) or RFC
/// 021 field-alias resolution for named struct-pattern fields (`resolve_field_alias`, private to that backend's
/// own lowering pass, with no Body IR v0 equivalent). Both are backend-owned refinements layered on top of the same
/// closed vocabulary below, not part of the vocabulary itself, and out of scope for this bucket; a pattern that
/// would need either still lowers structurally through the plain (non-narrowed) mapping, at the cost of the
/// resulting field types sometimes falling back to [`IncanType::Unknown`] where the existing backend's richer
/// resolution would have found something more precise.
#[derive(Debug, Clone, PartialEq)]
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
    /// A named-field constructor pattern (`Point(x=a, y=b)`): matches a model/class scrutinee and recursively
    /// matches/binds each named field.
    Struct {
        name: String,
        fields: Vec<(String, Pattern)>,
    },
    /// A positional constructor pattern (`Some(x)`, `Ok(value)`, `Shape::Circle(r)`): matches an enum-variant (or
    /// `Option`/`Result`) scrutinee and recursively matches/binds each positional field. `name` is the enum type
    /// name when known, or empty when v0 lowering could not resolve it (matching the existing backend's own
    /// `Pattern::Enum { name: String::new(), .. }` fallback for a bare, non-union constructor pattern) -- a target
    /// backend must not rely on `name` being populated.
    Enum {
        name: String,
        variant: String,
        fields: Vec<Pattern>,
    },
    /// An alternation pattern (`A | B`): matches if any alternative matches. Incan's typechecker (RFC 071) requires
    /// every alternative to bind an identical name/type set (`check_or_pattern` in
    /// `src/frontend/typechecker/check_expr/match_.rs`), so lowering declares exactly one shared local per bound
    /// name across all alternatives rather than one per alternative -- see `BodyBuilder::lower_match_pattern` in
    /// `src/frontend/body_ir.rs`.
    Or(Vec<Pattern>),
}

impl Pattern {
    /// Render a deterministic maintainer-facing spelling for this pattern.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Wildcard => "_".to_string(),
            Self::Var(binding) => binding.render_snapshot(),
            Self::Literal(constant) => constant.render_snapshot(),
            Self::Tuple(items) => {
                let items: Vec<String> = items.iter().map(Pattern::render_snapshot).collect();
                format!("({})", items.join(", "))
            }
            Self::Struct { name, fields } => {
                let fields: Vec<String> = fields
                    .iter()
                    .map(|(field_name, pat)| format!("{field_name}: {}", pat.render_snapshot()))
                    .collect();
                format!("{name} {{ {} }}", fields.join(", "))
            }
            Self::Enum { name, variant, fields } => {
                let label = if name.is_empty() {
                    variant.clone()
                } else {
                    format!("{name}::{variant}")
                };
                if fields.is_empty() {
                    label
                } else {
                    let fields: Vec<String> = fields.iter().map(Pattern::render_snapshot).collect();
                    format!("{label}({})", fields.join(", "))
                }
            }
            Self::Or(items) => {
                let items: Vec<String> = items.iter().map(Pattern::render_snapshot).collect();
                items.join(" | ")
            }
        }
    }
}

/// A name bound by a [`Pattern::Var`], pairing the fresh arm-scoped [`LocalId`] lowering declared for it with the
/// Duckborrower fact/last-use marker for reading the part of the scrutinee it binds.
///
/// See [`Rvalue::Match`]'s docs for why a pattern binding is modeled as this kind of read rather than an explicit
/// `Assign` statement copying out of the scrutinee: the actual value transfer happens as a side effect of the
/// target backend's native pattern match, and this model only needs to record *how* that transfer should be
/// treated (move/clone/borrow/copy) -- the same way [`PlaceOperand`] records it for an ordinary read.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternBinding {
    pub local: LocalId,
    /// The ownership decision selected for this binding, computed the same way [`PlaceOperand::fact`] is: the
    /// frontend lowering that builds this model (`src/frontend/body_ir.rs`) calls its own equivalent of the same
    /// place-read ownership selection used for every other read in this file, on the scrutinee place this binding
    /// projects into.
    pub fact: OwnershipFact,
    /// Whether this is statically the last read of the underlying scrutinee place this binding was computed from
    /// (see [`PlaceOperand::last_use`] for the same caveat about `fact`/`last_use` being kept as separate facts).
    pub last_use: bool,
}

impl PatternBinding {
    /// Render a deterministic maintainer-facing spelling for this binding.
    fn render_snapshot(&self) -> String {
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
/// Mirrors the existing Rust-emission backend's `FormatPart` (`src/backend/ir/expr.rs`), except the expression side
/// carries an [`Operand`] rather than a full expression tree -- Body IR always lowers embedded expressions through
/// the same [`Operand`]-producing path as any other read, so ownership facts and last-use tracking apply to
/// f-string interpolations exactly like any other expression use.
#[derive(Debug, Clone, PartialEq)]
pub enum FormatPart {
    /// Literal text between interpolations, carried through unescaped -- brace/format-string escaping is an
    /// emission-target concern (see the existing Rust-emission backend's `escape_format_literal`), not something
    /// this target-agnostic model commits to.
    Literal(String),
    /// An interpolated `{expr}` or `{expr!r}` segment.
    Expr {
        /// The already-lowered embedded expression's value.
        operand: Operand,
        /// Which formatting style the source syntax requested for this interpolation.
        style: FormatStyle,
    },
}

impl FormatPart {
    /// Render a deterministic maintainer-facing spelling for this format part.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Literal(s) => format!("lit({s:?})"),
            Self::Expr { operand, style } => format!("{}:{}", operand.render_snapshot(), style.as_str()),
        }
    }
}

/// Formatting style requested by one f-string interpolation (`{expr}` vs. `{expr!r}`). Mirrors the existing
/// Rust-emission backend's `FormatStyle` (`src/backend/ir/expr.rs`); unlike that backend's version, this one carries
/// no `emits_rust_debug`-style target-representation logic, since Body IR v0 stays target-agnostic and leaves the
/// decision of how a given style maps to a concrete formatting call to the consuming backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatStyle {
    /// User-facing display formatting (`{value}`).
    Display,
    /// Structured debug formatting (`{value!r}`).
    Debug,
}

impl FormatStyle {
    /// Compact snapshot spelling for this formatting style.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::Debug => "debug",
        }
    }
}

/// Unary operator kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Invert,
}

impl UnOp {
    /// Compact snapshot spelling for this unary operator.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "not ",
            Self::Invert => "~",
        }
    }
}

/// Binary operator kind supported by v0 lowering (arithmetic, comparison, boolean).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    /// Compact snapshot spelling for this binary operator.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::FloorDiv => "//",
            Self::Mod => "%",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::And => "and",
            Self::Or => "or",
        }
    }

    /// Whether this operator can panic at runtime (division/modulo by a possibly-zero divisor).
    pub const fn may_panic(self) -> bool {
        matches!(self, Self::Div | Self::FloorDiv | Self::Mod)
    }
}

/// Aggregate value shape built by [`Rvalue::Aggregate`].
#[derive(Debug, Clone, PartialEq)]
pub enum AggregateKind {
    Tuple,
    List,
    /// `{k: v, ...}` dict literal. The paired [`Rvalue::Aggregate`] operand vector alternates key, value, key,
    /// value, ... (`operands[2*i]` is the i-th entry's key, `operands[2*i + 1]` is its value), so a single flat
    /// operand vector can carry key/value pairs without a second `Rvalue` shape. Callers must always push keys and
    /// values in matching pairs; this doc comment is the single source of truth for that invariant, since
    /// [`Rvalue::Aggregate`] itself carries no static arity guarantee.
    Dict,
    /// `{v, ...}` set literal. Operands are the set's elements, one per entry -- the same flat shape as
    /// [`AggregateKind::List`].
    Set,
    Constructor(ConstructorTarget),
}

impl AggregateKind {
    /// Compact snapshot spelling for this aggregate kind.
    fn as_str(&self) -> String {
        match self {
            Self::Tuple => "tuple".to_string(),
            Self::List => "list".to_string(),
            Self::Dict => "dict".to_string(),
            Self::Set => "set".to_string(),
            Self::Constructor(target) => format!("constructor({}){}", target.name, target.binding.render_snapshot()),
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
/// *written source order*. The two orders differ whenever a caller writes named arguments out of declaration order,
/// and both are part of the source contract: the binding decides which parameter receives which value, while the
/// written order decides the order in which argument sub-expressions take effect.
///
/// # Ownership facts are sequenced by written order, not by operand index
///
/// This is the invariant a consumer is most likely to get wrong. Each operand's [`OwnershipFact`] and last-use
/// marker were decided while lowering the arguments **in written source order**, so they are only coherent when read
/// in that order. For `two(q=a, p=a)` the operand vector is `[move(_0, last_use), clone(_0)]`: read left to right
/// that appears to move a local and then clone it, but `written_position` says the clone happened first. An executor
/// that walks `args` in vector order will observe a use-after-move. Walk [`Self::Resolved::arguments`] by ascending
/// `written_position` when evaluating, and by operand index only when passing already-evaluated values.
///
/// This is the single binding mechanism for every call shape, replacing the per-target slot map #1124 introduced on
/// [`LocalCallableTarget`]: local callables, direct named calls, method calls, and nominal construction all record
/// their binding here, so a consumer learns the same fact the same way regardless of how the call was spelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentBinding {
    /// Arguments were supplied positionally and this stage resolved no declared parameter slots for them.
    ///
    /// Operand `i` is simply the `i`-th written argument; **no declared-slot claim is made**. This is the honest
    /// representation for a callee whose declared surface Body IR did not resolve — a builtin, a compiler-
    /// synthesized collection-growth call, or a signature with a `*args`/`**kwargs` rest parameter, where a written
    /// argument does not correspond one-to-one with a declared slot. Recording an identity slot map for those would
    /// assert a binding nobody checked.
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
        /// its own. Recording the omission here is still what
        /// frees a consumer from diffing the operand vector against the declaration, and it is why no defaulted slot
        /// carries an [`OwnershipFact`]: this call site evaluates nothing for it. Making those defaults evaluable is
        /// #1172's own scope, and this fact is what tells it which slots need a value.
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

    /// Render the non-trivial parts of this binding as snapshot suffixes.
    ///
    /// Slot and written-order lists are each emitted only when they differ from the identity mapping, and defaults
    /// only when the call site actually omitted something, so an ordinary positional call keeps a compact spelling
    /// and a non-trivial binding stands out. An unresolved binding always renders its own marker, so it can never be
    /// confused with a resolved identity binding.
    fn render_snapshot(&self) -> String {
        let Self::Resolved {
            arguments,
            defaulted_slots,
        } = self
        else {
            return " unbound".to_string();
        };
        let mut out = String::new();
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundArgument {
    /// Declared parameter or field index this operand supplies.
    pub slot: usize,
    /// Zero-based position of this argument among the call site's written arguments.
    pub written_position: usize,
}

/// Render a resolved call-site type-argument list as a snapshot suffix, or nothing when the call had none.
fn render_type_arguments(type_args: &[IncanType]) -> String {
    if type_args.is_empty() {
        return String::new();
    }
    let rendered: Vec<String> = type_args.iter().map(IncanType::to_string).collect();
    format!("[{}]", rendered.join(", "))
}

/// Call target for a [`StatementKind::Call`].
#[derive(Debug, Clone, PartialEq)]
pub enum Callee {
    /// A direct named function or a locally held callable value.
    ///
    /// Full call-target resolution (which physical declaration binds through imports/traits/overloads) mirrors the
    /// typechecker/backend resolution passes and is deferred past v0; Body IR records the source-level callee
    /// spelling plus argument ownership facts, which is enough to prove the model end-to-end.
    Function(CallableTarget),
    /// A method call `receiver.method(args)`. `args[0]` in the surrounding [`StatementKind::Call`] is the receiver.
    ///
    /// Also used for compiler-synthesized collection-growth calls a comprehension desugar introduces (`push`/
    /// `insert`) that have no source-level call site of their own -- see `lower_comprehension_terminal` in
    /// `src/frontend/body_ir.rs` for the synthesized case.
    Method(MethodTarget),
    /// A compiler-owned runtime/helper operation, represented explicitly instead of as a generated-Rust helper-call
    /// idiom (#653 criterion 3).
    Helper(HelperOp),
}

impl Callee {
    /// Render a deterministic maintainer-facing spelling for this callee.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Function(target) => target.render_snapshot(),
            Self::Method(target) => target.render_snapshot(),
            Self::Helper(op) => format!("helper:{}", op.as_str()),
        }
    }
}

/// The source-authoritative target carried by [`Callee::Function`].
///
/// The outer `Callee::Function` spelling is retained as the established call statement category: both alternatives
/// ultimately invoke a callable. The alternatives themselves are intentionally distinct. In particular, a stored
/// closure/partial is never represented as [`Self::Named`] with a fabricated source name; it is a
/// [`Self::Local`] operand, carrying the lexical-environment ownership decision the caller made.
#[derive(Debug, Clone, PartialEq)]
pub enum CallableTarget {
    /// A direct call to a named Incan function, by its source-level spelling.
    ///
    /// Full call-target resolution (which physical declaration binds through imports/traits/overloads) mirrors the
    /// typechecker/backend resolution passes and is deferred past v0; Body IR records the source-level callee
    /// spelling plus argument ownership facts, which is enough to prove the model end-to-end.
    Named(NamedCallableTarget),
    /// Invoke a callable value held in one local place.
    ///
    /// The [`PlaceOperand`] records the read's Duckborrower ownership fact and last-use marker exactly once, at the
    /// call target. Frontend lowering emits only an unprojected local place here: field/index call targets remain
    /// unsupported until Body IR has an equally explicit representation for their receiver/projection evaluation.
    /// Consumers must preserve this operand's ownership decision when invoking the value, because it can own a
    /// closure's lexical environment.
    Local(LocalCallableTarget),
}

/// One arm of a [`StatementKind::Race`].
///
/// The source spells a single binding name shared by every arm (`race for value:`), but each arm re-scopes it and
/// arms may resolve it to different types, so each arm owns its own `binding` local rather than sharing one. The
/// body is a full [`Block`] plus a `result` operand, mirroring how [`ClosureBody`] carries a nested statement
/// sequence with an explicit value instead of relying on a trailing-statement convention.
#[derive(Debug, Clone, PartialEq)]
pub struct RaceArm {
    /// This arm's awaitable, evaluated before selection along with every other arm's.
    pub awaitable: Operand,
    /// The local this arm binds the winning value to, visible only inside `body`.
    pub binding: LocalId,
    /// Statements that run only if this arm wins.
    pub body: Block,
    /// This arm's result value, written to the race's destination when it wins.
    pub result: Operand,
}

impl RaceArm {
    /// Render this arm's header and result. Its block is rendered separately by the statement renderer, so nested
    /// control flow inside an arm keeps the same line-oriented, indented shape it has anywhere else.
    fn render_header(&self) -> String {
        format!("await {} => _{}:", self.awaitable.render_snapshot(), self.binding.0)
    }
}

/// A locally stored callable target and the resolved binding of its call arguments.
///
/// The binding makes a local partial's two source-level call conventions explicit: positional arguments skip
/// preset-default slots, while named arguments may override them. Callers that lower an ordinary local closure use
/// [`ArgumentBinding::positional`].
#[derive(Debug, Clone, PartialEq)]
pub struct LocalCallableTarget {
    /// The local read that owns the callable value and its lexical environment.
    pub operand: PlaceOperand,
    /// Resolved binding of the surrounding [`StatementKind::Call::args`] to this callable's declared parameters.
    pub binding: ArgumentBinding,
}

/// One compiler-recognized named callable with an explicit direct-runtime rule.
///
/// This is intentionally distinct from an absent same-module declaration identity. An absent identity means the
/// source target is unresolved, imported, or otherwise unavailable to this Body-IR module and must refuse; only a
/// checked builtin fact may select a compiler-owned direct-runtime behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedCallableBuiltin {
    /// The compiler-owned `range` iterator constructor.
    Range,
}

/// A direct call to a named function, with its resolved call-site identity.
///
/// Explicit call-site type arguments are part of that identity rather than decoration: `decode_rows[Order](path)`
/// and `decode_rows[Row](path)` are different calls, and a consumer that only saw the name could not tell them
/// apart. `type_args` holds the *typechecker-resolved* arguments, so lowering never re-derives them from the source
/// spelling.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedCallableTarget {
    /// Source-level function name.
    pub name: String,
    /// Exact same-module declaration selected for this call, when Body IR can retain one.
    ///
    /// `None` is never permission for an executor to guess an imported, unresolved, or otherwise non-local target
    /// from [`Self::name`]. A compiler-owned callable uses [`Self::builtin`] instead.
    pub direct_call_id: Option<CompilerNodeId>,
    /// Compiler-recognized builtin target, if the typechecker proved this call did not resolve to a source binding.
    ///
    /// This is `None` for every same-module, imported, unresolved, or otherwise external target. Consumers must
    /// reject an absent `direct_call_id` and absent `builtin` rather than using [`Self::name`] as a dispatch key.
    pub builtin: Option<NamedCallableBuiltin>,
    /// Resolved explicit call-site type arguments, in declared type-parameter order. Empty when the call site wrote
    /// none.
    pub type_args: Vec<IncanType>,
    /// Resolved binding of the surrounding [`StatementKind::Call::args`] to this function's declared parameters.
    pub binding: ArgumentBinding,
}

/// A method call target and its resolved call-site identity.
///
/// Carries the same resolved type arguments and argument binding as [`NamedCallableTarget`]; the receiver itself
/// stays `args[0]` of the surrounding [`StatementKind::Call`] and is deliberately *not* part of the binding, whose
/// slots index the method's own declared parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodTarget {
    /// Source-level method name.
    pub name: String,
    /// Resolved explicit call-site type arguments, in declared type-parameter order. Empty when the call site wrote
    /// none.
    pub type_args: Vec<IncanType>,
    /// Resolved binding of the surrounding call's arguments *after* the receiver to this method's declared
    /// parameters.
    pub binding: ArgumentBinding,
}

impl MethodTarget {
    /// Build a method target for a compiler-synthesized positional call with no resolved declared signature.
    ///
    /// Used for calls a desugar introduces (a comprehension's `push`/`insert`, an iteration protocol's `__iter__`)
    /// that have no source-level call site and whose declared parameters this stage never resolved, so the binding
    /// is [`ArgumentBinding::UnresolvedPositional`] rather than a fabricated identity slot map.
    pub fn synthesized(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            type_args: Vec::new(),
            binding: ArgumentBinding::UnresolvedPositional,
        }
    }

    /// Render this method target without losing its resolved type arguments or non-trivial argument binding.
    fn render_snapshot(&self) -> String {
        format!(
            "method:{}{}{}",
            self.name,
            render_type_arguments(&self.type_args),
            self.binding.render_snapshot()
        )
    }
}

impl PartialEq<str> for MethodTarget {
    /// Compare against a bare method name, ignoring resolved type arguments and argument binding.
    ///
    /// This keeps existing bounded consumers' direct method-name checks working without each having to reach into
    /// the target's fields, mirroring [`CallableTarget`]'s own compatibility implementation.
    fn eq(&self, other: &str) -> bool {
        self.name == other
    }
}

impl std::fmt::Display for MethodTarget {
    /// Render the bare method name for concise diagnostics labels.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)
    }
}

/// A nominal `model`/`class` construction target and the resolved binding of its field arguments.
///
/// Source-level construction is named-only, so the binding is what records which declared field each operand fills.
/// The surrounding [`Rvalue::Aggregate`] operand vector is in declared field order; the binding carries the written
/// source order alongside it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstructorTarget {
    /// Source-level nominal type name.
    pub name: String,
    /// Exact source-local nominal declaration selected for this construction, when the module retained one.
    ///
    /// An absent identity is not permission to look up [`Self::name`] in another compiler structure: imports,
    /// aliases, classes, generic models, and any unretained nominal target must refuse at the constructor span.
    pub direct_declaration_id: Option<CompilerNodeId>,
    /// Resolved binding of the surrounding aggregate's operands to the type's declared fields.
    pub binding: ArgumentBinding,
}

impl CallableTarget {
    /// Render this callable target without collapsing local values into named functions.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Named(target) => format!(
                "fn:{}{}{}",
                target.name,
                render_type_arguments(&target.type_args),
                target.binding.render_snapshot()
            ),
            Self::Local(target) => format!(
                "local:{}{}",
                Operand::Place(target.operand.clone()).render_snapshot(),
                target.binding.render_snapshot()
            ),
        }
    }
}

impl PartialEq<str> for CallableTarget {
    /// A local callable can never compare equal to a direct function name. This compatibility implementation keeps
    /// existing bounded consumers' direct-function checks conservative while they visibly reject `Local`.
    fn eq(&self, other: &str) -> bool {
        matches!(self, Self::Named(target) if target.name == other)
    }
}

impl std::fmt::Display for CallableTarget {
    /// Render a concise diagnostics label without exposing a local callable as a function name.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Named(target) => f.write_str(&target.name),
            Self::Local(target) => write!(
                f,
                "<local:{}>",
                Operand::Place(target.operand.clone()).render_snapshot()
            ),
        }
    }
}

/// Compiler-owned runtime/helper operation. These correspond to the stdlib helper calls the existing Rust-emission
/// backend generates for string/list operators (see `src/backend/ir/conversions.rs::determine_binop_plan`), but are
/// represented here as an explicit Body IR operation rather than inferred later from generated Rust call shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperOp {
    StrConcat,
    StrEq,
    StrNe,
    StrLt,
    StrLe,
    StrGt,
    StrGe,
    ListConcat,
}

impl HelperOp {
    /// Compact snapshot spelling for this helper operation, also used as the [`AbiV0RuntimeRequirement::RuntimeHelper`]
    /// name so callers building runtime-requirement facts stay on the same helper naming as the snapshot renderer.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StrConcat => "str_concat",
            Self::StrEq => "str_eq",
            Self::StrNe => "str_ne",
            Self::StrLt => "str_lt",
            Self::StrLe => "str_le",
            Self::StrGt => "str_gt",
            Self::StrGe => "str_ge",
            Self::ListConcat => "list_concat",
        }
    }
}

// ============================================================================
// Statements and blocks
// ============================================================================

/// A normalized block of statements within a body, scoped to one [`ScopeId`].
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub scope: ScopeId,
    pub stmts: Vec<Statement>,
}

/// Render a block's statements at the given indentation depth (in two-space units).
fn render_block(out: &mut String, block: &Block, depth: usize) {
    let indent = "  ".repeat(depth);
    for stmt in &block.stmts {
        render_statement(out, stmt, &indent, depth);
    }
}

/// Render one statement, recursing into nested blocks for control-flow statements.
fn render_statement(out: &mut String, stmt: &Statement, indent: &str, depth: usize) {
    match &stmt.kind {
        StatementKind::Assign { place, rvalue } => {
            let _ = writeln!(
                out,
                "{indent}{} = {}",
                place.render_snapshot(),
                rvalue.render_snapshot()
            );
        }
        StatementKind::Call {
            destination,
            callee,
            args,
            may_panic,
        } => {
            let dest = destination
                .as_ref()
                .map(|p| format!("{} = ", p.render_snapshot()))
                .unwrap_or_default();
            let args_str: Vec<String> = args.iter().map(Operand::render_snapshot).collect();
            let panic_marker = if *may_panic { " may_panic" } else { "" };
            let _ = writeln!(
                out,
                "{indent}{dest}call {}({}){panic_marker}",
                callee.render_snapshot(),
                args_str.join(", ")
            );
        }
        StatementKind::Await { destination, awaited } => {
            let dest = destination
                .as_ref()
                .map(|place| format!("{} = ", place.render_snapshot()))
                .unwrap_or_default();
            let _ = writeln!(out, "{indent}{dest}await {}", awaited.render_snapshot());
        }
        StatementKind::Race { destination, arms } => {
            let dest = destination
                .as_ref()
                .map(|place| format!("{} = ", place.render_snapshot()))
                .unwrap_or_default();
            let _ = writeln!(out, "{indent}{dest}race:");
            for arm in arms {
                let _ = writeln!(out, "{indent}  {}", arm.render_header());
                render_block(out, &arm.body, depth + 2);
                let _ = writeln!(out, "{indent}  -> {}", arm.result.render_snapshot());
            }
        }
        StatementKind::Drop { local } => {
            let _ = writeln!(out, "{indent}drop _{}", local.0);
        }
        StatementKind::If {
            cond,
            then_block,
            else_block,
        } => {
            let _ = writeln!(out, "{indent}if {}:", cond.render_snapshot());
            render_block(out, then_block, depth + 1);
            if let Some(else_block) = else_block {
                let _ = writeln!(out, "{indent}else:");
                render_block(out, else_block, depth + 1);
            }
        }
        StatementKind::Loop { body } => {
            let _ = writeln!(out, "{indent}loop:");
            render_block(out, body, depth + 1);
        }
        StatementKind::Break { value } => {
            let value_str = value.as_ref().map(Operand::render_snapshot).unwrap_or_default();
            let _ = writeln!(out, "{indent}break {value_str}");
        }
        StatementKind::Continue => {
            let _ = writeln!(out, "{indent}continue");
        }
        StatementKind::Return { value } => {
            let value_str = value.as_ref().map(Operand::render_snapshot).unwrap_or_default();
            let _ = writeln!(out, "{indent}return {value_str}");
        }
        StatementKind::Yield { value } => {
            let _ = writeln!(out, "{indent}yield {}", value.render_snapshot());
        }
        StatementKind::Assert {
            cond,
            message,
            may_panic,
        } => {
            let msg = message
                .as_ref()
                .map(|m| format!(", {}", m.render_snapshot()))
                .unwrap_or_default();
            let panic_marker = if *may_panic { " may_panic" } else { "" };
            let _ = writeln!(out, "{indent}assert {}{msg}{panic_marker}", cond.render_snapshot());
        }
        StatementKind::Expr { value } => {
            let _ = writeln!(out, "{indent}expr {}", value.render_snapshot());
        }
        StatementKind::TryPropagate { destination, operand } => {
            let _ = writeln!(
                out,
                "{indent}{} = try?({})",
                destination.render_snapshot(),
                operand.render_snapshot()
            );
        }
        StatementKind::IterNext {
            destination,
            iterator,
            protocol,
        } => {
            let _ = writeln!(
                out,
                "{indent}{} = iter_next({}, {})",
                destination.render_snapshot(),
                iterator.render_snapshot(),
                protocol.render_snapshot()
            );
        }
        StatementKind::Unsupported { description } => {
            let _ = writeln!(out, "{indent}unsupported({description})");
        }
    }
}

/// How [`StatementKind::IterNext`] should poll one iteration, mirroring the two paths the existing Rust-emission
/// backend already branches on for general-iterable `for` (`src/backend/ir/lower/stmt.rs`'s `ast::Statement::For`
/// arm, keyed by `TypeCheckInfo::protocol_iteration`): a builtin collection needs no named method dispatch at all,
/// while a user-defined iterable resolves concrete `__iter__`/`__next__`-shaped method names through the
/// typechecker's iteration-protocol resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IterProtocol {
    /// Iterate a builtin collection (`List`/`Dict`/`String`) or a range with no explicit method dispatch. How to
    /// concretely advance such an iterator is left to the consuming backend, matching how a plain `for` doesn't
    /// manually unroll `IntoIterator` at the existing Rust-emission backend's own IR level either.
    Builtin,
    /// Iterate through a resolved user-defined iterator-protocol dispatch (`__iter__`/`__next__`-shaped magic
    /// methods).
    UserDefined {
        /// Method resolved on the iterator object to poll the next item (`iterator.__next__()`).
        next_method: String,
        /// Whether `next_method` returns a fallible `Result[Option[T], E]` rather than a plain `Option[T]`
        /// (`for item in iterable?:`, RFC 115). When `true`, [`StatementKind::IterNext`]'s implicit poll also
        /// carries an implicit early-return-with-conversion on the failure variant, mirroring
        /// [`StatementKind::TryPropagate`]'s own `From`/`Into` semantics, before the ordinary
        /// exhausted-vs-produced-a-value branch applies to the success payload.
        fallible: bool,
    },
}

impl IterProtocol {
    /// Compact snapshot spelling for this iteration protocol.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Builtin => "builtin".to_string(),
            Self::UserDefined { next_method, fallible } => {
                if *fallible {
                    format!("user_defined({next_method}, fallible)")
                } else {
                    format!("user_defined({next_method})")
                }
            }
        }
    }
}

/// One statement in a normalized Body IR block, carrying its own source span for diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: HirSourceSpan,
}

/// Canonical statement vocabulary for Body IR v0.
///
/// `while`/`for` source statements desugar into `Loop` + a conditional `Break` during lowering, rather than being
/// represented as distinct statement kinds, so the canonical vocabulary has a single loop shape.
#[derive(Debug, Clone, PartialEq)]
pub enum StatementKind {
    /// Assign an rvalue into a place.
    Assign { place: Place, rvalue: Rvalue },
    /// Call a function, method, locally stored callable value, or runtime helper, optionally storing its result.
    Call {
        destination: Option<Place>,
        callee: Callee,
        args: Vec<Operand>,
        /// Whether this call is known to be able to panic (helper operations that check preconditions).
        may_panic: bool,
    },
    /// Explicit drop of a local whose value was never moved out of its declaring scope.
    Drop { local: LocalId },
    /// `if cond: then_block [else: else_block]`.
    If {
        cond: Operand,
        then_block: Block,
        else_block: Option<Block>,
    },
    /// Normalized loop body. `while`/`for` desugar into this plus a leading conditional `Break`.
    Loop { body: Block },
    /// Exit the innermost enclosing `Loop`, optionally producing a value (loop-expression support is deferred; v0
    /// only lowers `break` as a statement, so this is always `None` from lowering today but is modeled for forward
    /// compatibility with loop-expression support).
    Break { value: Option<Operand> },
    /// Skip to the next iteration of the innermost enclosing `Loop`.
    Continue,
    /// Return from the body, optionally with a value.
    Return { value: Option<Operand> },
    /// `await operand` — a suspension point in an async body.
    ///
    /// Deliberately **not** [`Self::Yield`]. A generator `yield` produces a value outward and has no destination;
    /// an `await` consumes an awaitable and writes the resumed value into `destination`. Sharing one node would make
    /// two different contracts indistinguishable, and a task runtime needs exactly the fact that separates them:
    /// where the body suspends and what it resumes with.
    ///
    /// Effect ordering across this point is source-observable — statements before it run before suspension,
    /// statements after it run after resumption — and is preserved by this statement's position in its block. The
    /// awaited operand carries its own [`OwnershipFact`] and last-use marker like any other read.
    ///
    /// This node says a suspension happens here. It says nothing about *how*: task/frame state, wake/resume routing,
    /// and scheduling are the executing backend's, tracked by #1155.
    Await {
        /// Where the resumed value is written.
        ///
        /// Frontend lowering always supplies a destination today, including for a discarded `await` in statement
        /// position: the resumed value gets a temporary that the surrounding statement then ignores. The option
        /// exists for a future producer that can prove the value is unobservable, not because one exists now.
        destination: Option<Place>,
        /// The awaitable being suspended on.
        awaited: Operand,
    },
    /// `race for value:` — concurrent await over several arms, resuming on the first to complete.
    ///
    /// The contract this node fixes, which arm ordering alone could not express:
    ///
    /// - every arm's awaitable is evaluated **before** any selection happens, in source order;
    /// - arm order is source order and carries **no** priority — a consumer must not treat `arms[0]` as preferred;
    /// - exactly one arm's `body` runs, the one whose awaitable completed first;
    /// - every non-winning arm's awaitable is cancelled at this statement's boundary.
    ///
    /// As with [`Self::Await`], this records what the race *means*, not how it is driven: concurrent polling,
    /// first-completion detection, and the cancellation mechanism belong to #1155.
    Race {
        /// Where the winning arm's result is written. Always supplied by frontend lowering today, for the same
        /// reason as [`StatementKind::Await::destination`].
        destination: Option<Place>,
        /// The arms, in source order. Order is not priority — see this variant's docs.
        arms: Vec<RaceArm>,
    },
    /// `yield value` in statement position, suspending a generator body to produce one value.
    ///
    /// Only the statement-position form with a value is modeled -- see the module docs and
    /// [`Body::is_generator`]. A bare `yield` (no value) and expression-position `yield` (the two-way send/receive
    /// protocol, e.g. `x = yield val`) are out of scope for v0: both are stubs even in the existing Rust-emission
    /// backend today (`ast::Expr::Yield(_) => (IrExprKind::Unit, IrType::Unknown)` in
    /// `src/backend/ir/lower/expr/mod.rs`), so there is no real, delivered behavior for this variant to preserve.
    /// A generator function's body needs no separate top-level state-machine node: it lowers through this same
    /// statement vocabulary, while the target runtime owns the concrete suspension mechanism. The existing
    /// generated-Rust path uses `incan_stdlib::iter::Generator`'s channel-backed spawn bridge for `yield`-based
    /// functions; generator expressions instead carry their own deferred [`Rvalue::Generator`] body and use the
    /// iterator-adapter runtime path. Neither representation asks a consumer to infer a suspension point from a
    /// target-language closure shape.
    Yield { value: Operand },
    /// `assert cond[, message]`.
    Assert {
        cond: Operand,
        message: Option<Operand>,
        may_panic: bool,
    },
    /// An expression evaluated for its side effects only; its value is discarded.
    Expr { value: Operand },
    /// `operand?` (try/propagate). Evaluates `operand` (a `Result`-typed value; the current typechecker only
    /// allows `?` on `Result`, not `Option` -- see `validate_try_result_type` in
    /// `src/frontend/typechecker/check_expr/control_flow.rs`). On the failure variant (`Err`), returns early from
    /// the enclosing function with the failure value, converting it via `From`/`Into` when the enclosing
    /// function's error type differs from `operand`'s, mirroring Rust's built-in `?` desugaring. Otherwise stores
    /// the unwrapped success value (`Ok(v)`'s `v`) into `destination` and falls through to the next statement.
    ///
    /// Modeled as a single compiler-owned primitive rather than decomposed into explicit `is_err`/`unwrap`-style
    /// calls, matching the same #653-criterion-3 rationale as [`Callee::Helper`]: this operation is a
    /// compiler-owned semantic, not something to be inferred later from generated Rust call shapes, and full
    /// call-target resolution for a manual decomposition is out of scope for v0 (see the module docs). Unlike
    /// [`Callee::Helper`] this needs no runtime-helper requirement: the conversion is Rust's own `From`/`Into`
    /// machinery, not a compiler-provided function call.
    TryPropagate { destination: Place, operand: Operand },
    /// Poll one iteration of a general (non-range) `for` loop or comprehension `for` clause, standing in for
    /// `iterator.next_method()` (or a builtin collection's implicit advance) plus the branch on its result -- the
    /// same #653-criterion-3 "compiler-owned semantic gets its own explicit node" treatment as
    /// [`Self::TryPropagate`], applied to `Option`-shaped loop polling instead of `Result`-shaped early return.
    ///
    /// On exhaustion (the poll conceptually returns `None`, or `Ok(None)` under [`IterProtocol::UserDefined`]'s
    /// `fallible` flag), breaks out of the innermost enclosing [`Self::Loop`] -- mirroring how a range-based `for`
    /// already injects a leading conditional `Break` for its own exhaustion check. On a produced value (`Some(v)`,
    /// or `Ok(Some(v))` when fallible), stores `v` into `destination` and falls through to the next statement. When
    /// `protocol` is [`IterProtocol::UserDefined`] with `fallible: true`, a failure result (`Err(e)`) additionally
    /// short-circuits with an early return from the enclosing function, converting `e` via `From`/`Into` exactly
    /// like [`Self::TryPropagate`] -- so this single statement can carry up to three implicit outcomes when
    /// fallible, matching what `for item in iterable?:` (RFC 115) means as one syntactic form at the source level
    /// rather than decomposing it into a raw match a downstream consumer would have to re-derive.
    IterNext {
        /// Where the produced item is written when the iterator was not exhausted.
        destination: Place,
        /// The iterator being polled (already materialized by an earlier `Assign`/`Call` -- see
        /// `lower_general_iteration` in `src/frontend/body_ir.rs`).
        iterator: Operand,
        /// Which iteration protocol drives this poll.
        protocol: IterProtocol,
    },
    /// A source construct v0 lowering does not yet model. Keeps the model total over real programs instead of
    /// panicking or silently dropping the construct.
    Unsupported { description: String },
}

// ============================================================================
// Panic facts
// ============================================================================

/// One panic-interaction fact recorded for a body, without committing to a stable public panic strategy. This only
/// exposes *that* a statement may panic and *why* — strategy decisions (unwind vs. abort, drop-on-unwind ordering)
/// are left to later, target-specific work.
#[derive(Debug, Clone, PartialEq)]
pub struct PanicFact {
    pub span: HirSourceSpan,
    pub reason: PanicReason,
}

impl PanicFact {
    /// Render a deterministic maintainer-facing snapshot line for this panic fact.
    fn render_snapshot(&self) -> String {
        format!("{} span={}..{}", self.reason.as_str(), self.span.start, self.span.end)
    }
}

/// Why a statement may panic at runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum PanicReason {
    AssertFailure,
    DivisionOrModulo,
    HelperMayPanic(HelperOp),
}

impl PanicReason {
    /// Compact snapshot spelling for this panic reason.
    fn as_str(&self) -> String {
        match self {
            Self::AssertFailure => "assert_failure".to_string(),
            Self::DivisionOrModulo => "division_or_modulo".to_string(),
            Self::HelperMayPanic(op) => format!("helper_may_panic({})", op.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompilerNodeKind, IncanPrimitiveType};

    fn sample_body() -> Body {
        let decl_id = CompilerNodeId::declaration("m", "add");
        let direct_call_id = CompilerNodeId::declaration_span("m", 0, 30);
        let local_x = LocalId(0);
        let local_y = LocalId(1);
        let local_tmp = LocalId(2);
        Body {
            decl_id: decl_id.clone(),
            direct_call_id,
            name: "add".to_string(),
            span: HirSourceSpan::new(0, 30),
            locals: vec![
                LocalDecl {
                    id: local_x,
                    name: Some("x".to_string()),
                    ty: IncanType::Primitive(IncanPrimitiveType::Int),
                    origin: LocalOrigin::Parameter,
                    scope: ScopeId(0),
                    span: HirSourceSpan::new(4, 5),
                },
                LocalDecl {
                    id: local_y,
                    name: Some("y".to_string()),
                    ty: IncanType::Primitive(IncanPrimitiveType::Int),
                    origin: LocalOrigin::Parameter,
                    scope: ScopeId(0),
                    span: HirSourceSpan::new(7, 8),
                },
                LocalDecl {
                    id: local_tmp,
                    name: None,
                    ty: IncanType::Primitive(IncanPrimitiveType::Int),
                    origin: LocalOrigin::Temporary,
                    scope: ScopeId(0),
                    span: HirSourceSpan::new(20, 25),
                },
            ],
            params: vec![
                CallableParam {
                    local: local_x,
                    name: "x".to_string(),
                    ty: IncanType::Primitive(IncanPrimitiveType::Int),
                    span: HirSourceSpan::new(4, 5),
                    default: CallableParamDefault::Required,
                },
                CallableParam {
                    local: local_y,
                    name: "y".to_string(),
                    ty: IncanType::Primitive(IncanPrimitiveType::Int),
                    span: HirSourceSpan::new(7, 8),
                    default: CallableParamDefault::Required,
                },
            ],
            param_locals: vec![local_x, local_y],
            scopes: vec![ScopeInfo {
                id: ScopeId(0),
                parent: None,
                span: HirSourceSpan::new(0, 30),
            }],
            block: Block {
                scope: ScopeId(0),
                stmts: vec![
                    Statement {
                        kind: StatementKind::Assign {
                            place: Place::from_local(local_tmp),
                            rvalue: Rvalue::BinaryOp(
                                BinOp::Add,
                                Operand::place(Place::from_local(local_x), OwnershipFact::Copy, false),
                                Operand::place(Place::from_local(local_y), OwnershipFact::Copy, true),
                            ),
                        },
                        span: HirSourceSpan::new(20, 25),
                    },
                    Statement {
                        kind: StatementKind::Return {
                            value: Some(Operand::place(Place::from_local(local_tmp), OwnershipFact::Move, true)),
                        },
                        span: HirSourceSpan::new(20, 30),
                    },
                ],
            },
            runtime_requirements: Vec::new(),
            panic_facts: Vec::new(),
            is_async: false,
        }
    }

    #[test]
    fn body_snapshot_is_deterministic() {
        let body = sample_body();
        assert_eq!(body.render_snapshot(), body.render_snapshot());
    }

    #[test]
    fn body_snapshot_renders_locals_and_control_flow() {
        let snapshot = sample_body().render_snapshot();
        assert!(snapshot.contains("body add decl:m::add span=0..30"));
        assert!(snapshot.contains("local 0 x : int [param]"));
        assert!(snapshot.contains("local 2 <tmp> : int [temp]"));
        assert!(snapshot.contains("_2 = copy(_0) + copy(_1, last_use)"));
        assert!(snapshot.contains("return move(_2, last_use)"));
    }

    #[test]
    fn generator_rvalue_keeps_construction_inputs_separate_from_its_deferred_body() {
        let source_local = LocalId(2);
        let capture_local = LocalId(1);
        let generator = Rvalue::Generator {
            source: Operand::Constant(Constant::Int(1)),
            captured_operands: vec![Operand::place(
                Place::from_local(capture_local),
                OwnershipFact::Copy,
                false,
            )],
            body: Box::new(GeneratorBody {
                source_local,
                capture_locals: vec![capture_local],
                stmts: vec![Statement {
                    kind: StatementKind::Yield {
                        value: Operand::Constant(Constant::Int(2)),
                    },
                    span: HirSourceSpan::new(10, 15),
                }],
            }),
        };
        let snapshot = generator.render_snapshot();
        assert_eq!(
            snapshot,
            "generator(source=const(1), captures=[copy(_1)]) { deferred: yield const(2) }"
        );

        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Assign {
                    place: Place::from_local(source_local),
                    rvalue: generator,
                },
                span: HirSourceSpan::new(0, 15),
            },
        );
        assert!(
            !body.is_generator(),
            "a yielded generator expression is a value; it must not turn its enclosing function into a generator"
        );
    }

    #[test]
    fn helper_call_and_runtime_requirements_render() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Call {
                    destination: Some(Place::from_local(LocalId(2))),
                    callee: Callee::Helper(HelperOp::StrConcat),
                    args: vec![Operand::Constant(Constant::Str("a".to_string()))],
                    may_panic: false,
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        body.runtime_requirements = vec![
            AbiV0RuntimeRequirement::Allocator,
            AbiV0RuntimeRequirement::RuntimeHelper("str_concat".to_string()),
        ];
        body.panic_facts = vec![PanicFact {
            span: HirSourceSpan::new(20, 25),
            reason: PanicReason::DivisionOrModulo,
        }];

        let snapshot = body.render_snapshot();
        assert!(snapshot.contains("call helper:str_concat(const(\"a\"))"));
        assert!(snapshot.contains("runtime_requirements:"));
        assert!(snapshot.contains("allocator"));
        assert!(snapshot.contains("runtime_helper(str_concat)"));
        assert!(snapshot.contains("panic_facts:"));
        assert!(snapshot.contains("division_or_modulo span=20..25"));
    }

    #[test]
    fn local_callable_target_and_tagged_default_parameter_render() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Call {
                    destination: Some(Place::from_local(LocalId(2))),
                    callee: Callee::Function(CallableTarget::Local(LocalCallableTarget {
                        operand: PlaceOperand {
                            place: Place::from_local(LocalId(0)),
                            fact: OwnershipFact::Copy,
                            last_use: false,
                        },
                        binding: ArgumentBinding::resolved_positional(1),
                    })),
                    args: vec![Operand::Constant(Constant::Int(1))],
                    may_panic: false,
                },
                span: HirSourceSpan::new(0, 1),
            },
        );

        let snapshot = body.render_snapshot();
        assert!(
            snapshot.contains("call local:copy(_0)(const(1))"),
            "a local call target must retain its operand ownership fact: {snapshot}"
        );
        assert_eq!(
            CallableParam {
                local: LocalId(7),
                name: "suffix".to_string(),
                ty: IncanType::Primitive(IncanPrimitiveType::Str),
                span: HirSourceSpan::new(12, 18),
                default: CallableParamDefault::Source(DefaultComputation {
                    span: HirSourceSpan::new(21, 27),
                    stmts: Vec::new(),
                    result: Operand::Constant(Constant::Str("txt".to_string())),
                }),
            }
            .render_snapshot(),
            "suffix: str local=_7 span=12..18 = source_default(span=21..27 result: const(\"txt\"))"
        );
    }

    #[test]
    fn callable_parameter_default_origins_are_distinct_and_deterministic() {
        let source = CallableParam {
            local: LocalId(1),
            name: "limit".to_string(),
            ty: IncanType::Primitive(IncanPrimitiveType::Int),
            span: HirSourceSpan::new(4, 14),
            default: CallableParamDefault::Source(DefaultComputation {
                span: HirSourceSpan::new(12, 13),
                stmts: vec![Statement {
                    kind: StatementKind::Assign {
                        place: Place::from_local(LocalId(3)),
                        rvalue: Rvalue::UnaryOp(UnOp::Neg, Operand::Constant(Constant::Int(1))),
                    },
                    span: HirSourceSpan::new(12, 13),
                }],
                result: Operand::place(Place::from_local(LocalId(3)), OwnershipFact::Copy, true),
            }),
        };
        let preset = CallableParam {
            local: LocalId(2),
            name: "method".to_string(),
            ty: IncanType::Primitive(IncanPrimitiveType::Str),
            span: HirSourceSpan::new(16, 27),
            default: CallableParamDefault::PartialPreset { capture: LocalId(6) },
        };
        let unsupported = CallableParam {
            local: LocalId(4),
            name: "payload".to_string(),
            ty: IncanType::Primitive(IncanPrimitiveType::Bytes),
            span: HirSourceSpan::new(29, 43),
            default: CallableParamDefault::Unsupported {
                span: HirSourceSpan::new(40, 43),
                description: "bytes literal".to_string(),
            },
        };

        assert_eq!(
            source.render_snapshot(),
            "limit: int local=_1 span=4..14 = source_default(span=12..13 _3 = -const(1); result: copy(_3, last_use))"
        );
        assert_eq!(
            preset.render_snapshot(),
            "method: str local=_2 span=16..27 = captured(_6)"
        );
        assert_eq!(
            unsupported.render_snapshot(),
            "payload: bytes local=_4 span=29..43 = unsupported_default(bytes literal span=40..43)"
        );
    }

    #[test]
    fn dict_aggregate_renders_as_key_value_pairs() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Assign {
                    place: Place::from_local(LocalId(2)),
                    rvalue: Rvalue::Aggregate(
                        AggregateKind::Dict,
                        vec![
                            Operand::Constant(Constant::Str("a".to_string())),
                            Operand::Constant(Constant::Int(1)),
                        ],
                    ),
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();
        assert!(
            snapshot.contains("dict[const(\"a\"): const(1)]"),
            "dict aggregate should render key/value pairs, not a flat list: {snapshot}"
        );
    }

    #[test]
    fn set_aggregate_renders_as_a_flat_element_list() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Assign {
                    place: Place::from_local(LocalId(2)),
                    rvalue: Rvalue::Aggregate(AggregateKind::Set, vec![Operand::Constant(Constant::Int(1))]),
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();
        assert!(snapshot.contains("set[const(1)]"));
    }

    #[test]
    fn slice_projection_renders_optional_components() {
        let full_slice = Place {
            local: LocalId(0),
            projection: vec![PlaceElem::Slice {
                start: Some(Box::new(Operand::Constant(Constant::Int(1)))),
                end: Some(Box::new(Operand::Constant(Constant::Int(3)))),
                step: None,
            }],
        };
        assert_eq!(full_slice.render_snapshot(), "_0[const(1):const(3)]");

        let stepped_slice = Place {
            local: LocalId(0),
            projection: vec![PlaceElem::Slice {
                start: None,
                end: None,
                step: Some(Box::new(Operand::Constant(Constant::Int(2)))),
            }],
        };
        assert_eq!(stepped_slice.render_snapshot(), "_0[::const(2)]");
    }

    #[test]
    fn try_propagate_statement_renders_destination_and_operand() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::TryPropagate {
                    destination: Place::from_local(LocalId(2)),
                    operand: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::Move, true),
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();
        assert!(snapshot.contains("_2 = try?(move(_0, last_use))"));
    }

    #[test]
    fn iter_next_renders_builtin_protocol() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::IterNext {
                    destination: Place::from_local(LocalId(2)),
                    iterator: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::MutBorrow, false),
                    protocol: IterProtocol::Builtin,
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();
        assert!(snapshot.contains("_2 = iter_next(mut_borrow(_0), builtin)"));
    }

    #[test]
    fn iter_next_renders_user_defined_protocol_and_fallible_flag() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::IterNext {
                    destination: Place::from_local(LocalId(2)),
                    iterator: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::MutBorrow, false),
                    protocol: IterProtocol::UserDefined {
                        next_method: "__next__".to_string(),
                        fallible: false,
                    },
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();
        assert!(snapshot.contains("_2 = iter_next(mut_borrow(_0), user_defined(__next__))"));

        let mut fallible_body = sample_body();
        fallible_body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::IterNext {
                    destination: Place::from_local(LocalId(2)),
                    iterator: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::MutBorrow, false),
                    protocol: IterProtocol::UserDefined {
                        next_method: "__next__".to_string(),
                        fallible: true,
                    },
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let fallible_snapshot = fallible_body.render_snapshot();
        assert!(fallible_snapshot.contains("_2 = iter_next(mut_borrow(_0), user_defined(__next__, fallible))"));
    }

    #[test]
    fn format_rvalue_renders_literal_and_expr_parts_in_order() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Assign {
                    place: Place::from_local(LocalId(2)),
                    rvalue: Rvalue::Format(vec![
                        FormatPart::Literal("x=".to_string()),
                        FormatPart::Expr {
                            operand: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::Copy, false),
                            style: FormatStyle::Display,
                        },
                        FormatPart::Literal(" y=".to_string()),
                        FormatPart::Expr {
                            operand: Operand::place(Place::from_local(LocalId(1)), OwnershipFact::Borrow, false),
                            style: FormatStyle::Debug,
                        },
                    ]),
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();
        assert!(
            snapshot.contains("_2 = fstring(lit(\"x=\"), copy(_0):display, lit(\" y=\"), borrow(_1):debug)"),
            "unexpected fstring rendering: {snapshot}"
        );
    }

    #[test]
    fn closure_rvalue_renders_params_captures_and_nested_body() {
        let mut body = sample_body();
        // Simulate `(z: int) => x + z` capturing `_0` (the sample body's `x` param) with a `clone` fact, where the
        // closure's own param `z` gets local `_3` and the capture gets local `_4`.
        let param_local = LocalId(3);
        let capture_local = LocalId(4);
        body.locals.push(LocalDecl {
            id: param_local,
            name: Some("z".to_string()),
            ty: IncanType::Primitive(IncanPrimitiveType::Int),
            origin: LocalOrigin::Parameter,
            scope: ScopeId(0),
            span: HirSourceSpan::new(0, 1),
        });
        body.locals.push(LocalDecl {
            id: capture_local,
            name: Some("x".to_string()),
            ty: IncanType::Primitive(IncanPrimitiveType::Int),
            origin: LocalOrigin::Captured,
            scope: ScopeId(0),
            span: HirSourceSpan::new(0, 1),
        });
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Assign {
                    place: Place::from_local(LocalId(2)),
                    rvalue: Rvalue::Closure {
                        params: vec![CallableParam {
                            local: param_local,
                            name: "z".to_string(),
                            ty: IncanType::Primitive(IncanPrimitiveType::Int),
                            span: HirSourceSpan::new(0, 1),
                            default: CallableParamDefault::Required,
                        }],
                        captured_operands: vec![Operand::place(
                            Place::from_local(LocalId(0)),
                            OwnershipFact::Clone,
                            false,
                        )],
                        body: Box::new(ClosureBody {
                            capture_locals: vec![capture_local],
                            stmts: Vec::new(),
                            result: Operand::place(
                                Place {
                                    local: capture_local,
                                    projection: Vec::new(),
                                },
                                OwnershipFact::Copy,
                                false,
                            ),
                        }),
                    },
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();
        assert!(
            snapshot.contains("closure(params=[z: int local=_3 span=0..1], captures=[clone(_0)]) { result: copy(_4) }"),
            "unexpected closure rendering: {snapshot}"
        );
        assert!(snapshot.contains("local 4 x : int [captured]"));
    }

    #[test]
    fn yield_statement_renders_and_marks_the_body_as_a_generator() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Yield {
                    value: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::Copy, false),
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();
        assert!(
            snapshot.contains("yield copy(_0)"),
            "unexpected yield rendering: {snapshot}"
        );
        assert!(
            body.is_generator(),
            "a top-level yield should mark the body a generator"
        );
    }

    #[test]
    fn is_generator_finds_a_yield_nested_inside_if_and_loop_blocks() {
        let mut body = sample_body();
        let yield_stmt = Statement {
            kind: StatementKind::Yield {
                value: Operand::Constant(Constant::Int(1)),
            },
            span: HirSourceSpan::new(0, 1),
        };
        // Nested under a `Loop` inside an `If`'s `then_block`, mirroring `yield` inside `if cond: while ...`.
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::If {
                    cond: Operand::Constant(Constant::Bool(true)),
                    then_block: Block {
                        scope: ScopeId(0),
                        stmts: vec![Statement {
                            kind: StatementKind::Loop {
                                body: Block {
                                    scope: ScopeId(0),
                                    stmts: vec![yield_stmt],
                                },
                            },
                            span: HirSourceSpan::new(0, 1),
                        }],
                    },
                    else_block: None,
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        assert!(
            body.is_generator(),
            "a yield nested under If -> Loop should still be found"
        );
    }

    #[test]
    fn is_generator_is_false_without_any_yield_statement() {
        assert!(
            !sample_body().is_generator(),
            "sample_body contains no yield and must not be reported as a generator"
        );
    }

    #[test]
    fn locals_requiring_unwind_drop_is_conservative_over_non_copy_locals() {
        let mut body = sample_body();
        body.locals.push(LocalDecl {
            id: LocalId(3),
            name: Some("s".to_string()),
            ty: IncanType::Primitive(IncanPrimitiveType::Str),
            origin: LocalOrigin::UserBinding,
            scope: ScopeId(0),
            span: HirSourceSpan::new(10, 15),
        });

        let drop_relevant = body.locals_requiring_unwind_drop();
        assert_eq!(drop_relevant, vec![LocalId(3)]);
    }

    #[test]
    fn locals_requiring_unwind_drop_excludes_receiver_locals_even_when_non_copy() {
        let mut body = sample_body();
        body.locals.push(LocalDecl {
            id: LocalId(3),
            name: Some("self".to_string()),
            ty: IncanType::Named("Counter".to_string()),
            origin: LocalOrigin::Receiver { mutable: true },
            scope: ScopeId(0),
            span: HirSourceSpan::new(0, 4),
        });

        let drop_relevant = body.locals_requiring_unwind_drop();
        assert!(
            !drop_relevant.contains(&LocalId(3)),
            "a receiver is a reference, not drop-relevant, even though its type is non-Copy: {drop_relevant:?}"
        );
    }

    #[test]
    fn receiver_origin_renders_mutability_in_the_snapshot() {
        let mut body = sample_body();
        body.locals.push(LocalDecl {
            id: LocalId(3),
            name: Some("self".to_string()),
            ty: IncanType::Named("Counter".to_string()),
            origin: LocalOrigin::Receiver { mutable: false },
            scope: ScopeId(0),
            span: HirSourceSpan::new(0, 4),
        });
        body.locals.push(LocalDecl {
            id: LocalId(4),
            name: Some("self".to_string()),
            ty: IncanType::Named("Counter".to_string()),
            origin: LocalOrigin::Receiver { mutable: true },
            scope: ScopeId(0),
            span: HirSourceSpan::new(0, 4),
        });

        let snapshot = body.render_snapshot();
        assert!(snapshot.contains("local 3 self : Counter [receiver]"));
        assert!(snapshot.contains("local 4 self : Counter [receiver_mut]"));
    }

    #[test]
    fn body_ir_module_snapshot_wraps_bodies() {
        let module = BodyIrModule {
            module_id: CompilerNodeId::new(CompilerNodeKind::Module, "m"),
            nominal_declarations: Vec::new(),
            value_enum_declarations: Vec::new(),
            bodies: vec![sample_body()],
        };
        let snapshot = module.render_snapshot();
        assert!(snapshot.starts_with("body_ir_module module:m\n"));
        assert!(snapshot.contains("body add decl:m::add"));
    }

    /// One `Rvalue::Match` exercising every `Pattern` variant this data model closes over (see #1101's B6): a
    /// literal, a tuple nesting a binding and a wildcard behind a guard, a named-field struct constructor, a
    /// positional enum constructor, and an alternation. Mirrors `sample_body`'s own style of hand-building a
    /// [`Statement`] rather than going through the frontend lowering the `src/frontend/body_ir.rs` integration
    /// tests exercise instead.
    #[test]
    fn match_rvalue_renders_scrutinee_and_every_pattern_shape() {
        let mut body = sample_body();
        let match_stmt = Statement {
            kind: StatementKind::Assign {
                place: Place::from_local(LocalId(2)),
                rvalue: Rvalue::Match {
                    scrutinee: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::Borrow, false),
                    arms: vec![
                        MatchArm {
                            pattern: Pattern::Literal(Constant::Int(0)),
                            guard_stmts: Vec::new(),
                            guard: None,
                            body_stmts: Vec::new(),
                            result: Operand::Constant(Constant::Int(100)),
                        },
                        MatchArm {
                            pattern: Pattern::Tuple(vec![
                                Pattern::Var(PatternBinding {
                                    local: LocalId(1),
                                    fact: OwnershipFact::Copy,
                                    last_use: false,
                                }),
                                Pattern::Wildcard,
                            ]),
                            guard_stmts: Vec::new(),
                            guard: Some(Operand::place(
                                Place::from_local(LocalId(1)),
                                OwnershipFact::Copy,
                                false,
                            )),
                            body_stmts: Vec::new(),
                            result: Operand::place(Place::from_local(LocalId(1)), OwnershipFact::Copy, false),
                        },
                        MatchArm {
                            pattern: Pattern::Struct {
                                name: "Point".to_string(),
                                fields: vec![("x".to_string(), Pattern::Wildcard)],
                            },
                            guard_stmts: Vec::new(),
                            guard: None,
                            body_stmts: Vec::new(),
                            result: Operand::Constant(Constant::Unit),
                        },
                        MatchArm {
                            pattern: Pattern::Enum {
                                name: String::new(),
                                variant: "Some".to_string(),
                                fields: vec![Pattern::Wildcard],
                            },
                            guard_stmts: Vec::new(),
                            guard: None,
                            body_stmts: Vec::new(),
                            result: Operand::Constant(Constant::Unit),
                        },
                        MatchArm {
                            pattern: Pattern::Or(vec![
                                Pattern::Literal(Constant::Int(1)),
                                Pattern::Literal(Constant::Int(2)),
                            ]),
                            guard_stmts: Vec::new(),
                            guard: None,
                            body_stmts: Vec::new(),
                            result: Operand::Constant(Constant::Unit),
                        },
                    ],
                },
            },
            span: HirSourceSpan::new(0, 1),
        };
        body.block.stmts.insert(0, match_stmt);
        let snapshot = body.render_snapshot();

        assert!(
            snapshot.contains("match borrow(_0) {"),
            "unexpected match rendering: {snapshot}"
        );
        assert!(
            snapshot.contains("const(0) => const(100)"),
            "literal pattern: {snapshot}"
        );
        assert!(
            snapshot.contains("(bind(_1, copy), _) if copy(_1) => copy(_1)"),
            "tuple pattern with a nested binding, wildcard, and guard: {snapshot}"
        );
        assert!(
            snapshot.contains("Point { x: _ } => const(())"),
            "named-field struct pattern: {snapshot}"
        );
        assert!(
            snapshot.contains("Some(_) => const(())"),
            "positional enum pattern: {snapshot}"
        );
        assert!(
            snapshot.contains("const(1) | const(2) => const(())"),
            "alternation pattern: {snapshot}"
        );
    }

    #[test]
    fn match_rvalue_snapshot_is_deterministic() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Assign {
                    place: Place::from_local(LocalId(2)),
                    rvalue: Rvalue::Match {
                        scrutinee: Operand::place(Place::from_local(LocalId(0)), OwnershipFact::Borrow, false),
                        arms: vec![MatchArm {
                            pattern: Pattern::Wildcard,
                            guard_stmts: Vec::new(),
                            guard: None,
                            body_stmts: Vec::new(),
                            result: Operand::Constant(Constant::Unit),
                        }],
                    },
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        assert_eq!(body.render_snapshot(), body.render_snapshot());
    }
}
