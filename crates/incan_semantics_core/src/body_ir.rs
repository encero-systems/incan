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

use incan_core::errors::ErrorKind;
use incan_core::lang::builtins::BuiltinFnId;
use incan_core::lang::errors;

use crate::{
    AbiV0RuntimeRequirement, CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanType, module_identity_for_path,
};

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
    /// Source-local fieldless normal-enum declarations whose canonical unit variants are available to direct
    /// execution.
    ///
    /// This is not a general algebraic-data-type registry. The direct profile may materialize only an exact retained
    /// unit variant, compare two carriers of the same retained enum identity, and dispatch a pattern carrying the
    /// same exact enum/member identities; payload variants, aliases, imports, traits, and methods remain absent and
    /// must refuse.
    pub fieldless_enum_declarations: Vec<FieldlessEnumDeclaration>,
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
    /// Resolve a canonical callable identity to the body that declares it, when this module owns the declaration.
    ///
    /// This is the consumer seam for [`CanonicalSymbolId`]: a backend asks *which declaration was selected* and gets
    /// the declaration itself or nothing. It never consults the *reference site's* spelling or an emitted Rust name,
    /// so a consumer cannot dispatch on the text at a call site.
    ///
    /// Matching is on the declaration span, which is unique within a module. The declared name deliberately does not
    /// participate: bodies do not carry owner-qualified names, so a class method and a free function in one module can
    /// both be named `render`, and a name match would hand back whichever came first.
    ///
    /// `None` has three distinct causes and is never permission to fall back to [`NamedCallableTarget::name`]: the
    /// identity is owned by another module; its origin is not a project source module at all (a package, a `rust::`
    /// crate, or a builtin); or this module owns it but it has no lowered body, as for a model or a `const`.
    pub fn body_for_canonical_target(&self, target: &CanonicalSymbolId) -> Option<&Body> {
        if module_identity_for_path(target.module_path()?) != self.module_id.path() {
            return None;
        }
        self.bodies.iter().find(|body| body.span == target.declaration_span)
    }

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
        for declaration in &self.fieldless_enum_declarations {
            let _ = writeln!(
                &mut out,
                "fieldless_enum {} id={} variants=[{}]",
                declaration.name,
                declaration.direct_declaration_id,
                declaration
                    .variants
                    .iter()
                    .map(FieldlessEnumVariantDeclaration::render_snapshot)
                    .collect::<Vec<_>>()
                    .join(", ")
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
/// behavior-bearing models. Its field order is the checked constructor-slot order; a direct runtime must compare it
/// with [`ConstructorTarget::canonical_field_layout`] before applying [`ConstructorTarget::binding`], rather than
/// treating constructor argument spelling as layout evidence.
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

/// One source-local fieldless normal enum retained for direct identity comparison.
///
/// The record exists only for undecorated, non-generic declarations with no traits, methods, aliases, value backing,
/// or payload variants. A runtime validates these records before materializing a carrier and never treats the enum
/// spelling itself as an execution authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldlessEnumDeclaration {
    /// Exact source-local enum declaration identity, derived from the declaration source span.
    pub direct_declaration_id: CompilerNodeId,
    /// Canonical source declaration name.
    pub name: String,
    /// Canonical zero-payload variants in source declaration order.
    pub variants: Vec<FieldlessEnumVariantDeclaration>,
}

/// One canonical zero-payload member of a retained fieldless normal enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldlessEnumVariantDeclaration {
    /// Exact source-local variant declaration identity, derived from the declaration source span.
    pub direct_declaration_id: CompilerNodeId,
    /// Canonical member name; aliases are intentionally not retained.
    pub name: String,
}

impl FieldlessEnumVariantDeclaration {
    /// Render the member identity for a deterministic module snapshot.
    fn render_snapshot(&self) -> String {
        format!("variant {} id={}", self.name, self.direct_declaration_id)
    }
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
}

impl Constant {
    /// Render a deterministic maintainer-facing spelling for this constant.
    ///
    /// A byte string renders every byte as a `\xNN` escape rather than as text, so a snapshot can never read a
    /// `bytes` constant as the `str` constant it is not.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Int(v) => format!("const({v})"),
            Self::Float(v) => format!("const({v})"),
            Self::Bool(v) => format!("const({v})"),
            Self::Str(v) => format!("const({v:?})"),
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

/// Exact source-local enum/member identity selected for one fieldless normal-enum member expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldlessEnumVariantTarget {
    /// Exact source-local owner enum declaration identity.
    pub enum_declaration_id: CompilerNodeId,
    /// Exact source-local member declaration identity.
    pub variant_declaration_id: CompilerNodeId,
    /// Canonical owner name, retained for malformed-Body-IR cross-checks and diagnostics.
    pub enum_name: String,
    /// Canonical member name, retained for malformed-Body-IR cross-checks and diagnostics.
    pub variant_name: String,
}

impl FieldlessEnumVariantTarget {
    /// Render both retained identities so snapshots cannot mistake a unit-variant spelling for a direct target fact.
    fn render_snapshot(&self) -> String {
        format!(
            "fieldless_enum_variant({}::{} enum_id={} variant_id={})",
            self.enum_name, self.variant_name, self.enum_declaration_id, self.variant_declaration_id
        )
    }
}

/// One direct compiler-owned `Result` construction with its checked payload and error facts retained.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultVariant {
    /// Which intrinsic Result constructor source checking selected.
    pub kind: ResultVariantKind,
    /// The one source-order payload operand.
    pub payload: Operand,
    /// Checked `Result` success type.
    pub ok_type: IncanType,
    /// Checked `Result` error type.
    pub error_type: IncanType,
}

impl ResultVariant {
    /// Render the construction and checked type facts without consulting any target-language Result spelling.
    fn render_snapshot(&self) -> String {
        format!(
            "result_{}({}, ok_type={}, error_type={})",
            self.kind.as_str(),
            self.payload.render_snapshot(),
            self.ok_type,
            self.error_type
        )
    }
}

/// Intrinsic source-level Result constructor selected by typechecking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultVariantKind {
    /// `Ok(payload)`.
    Ok,
    /// `Err(payload)`.
    Err,
}

impl ResultVariantKind {
    /// Stable lowercase spelling for snapshots and receipts.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Err => "err",
        }
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
/// flat, function-wide local-ID namespace. The frontend snapshots and restores its active name-to-local map at
/// lexical boundaries (including closures), so
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
    /// A source-local plain-model pattern whose exact declaration identity is retained alongside canonical named
    /// fields. This is distinct from [`Self::Struct`], whose name-only fallback remains a visible refusal.
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

/// Exact source-local plain-model declaration selected by one structural pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NominalPatternTarget {
    /// Exact source-local model declaration identity.
    pub direct_declaration_id: CompilerNodeId,
    /// Canonical model name retained only for malformed-Body-IR cross-checks and diagnostics.
    pub name: String,
}

impl NominalPatternTarget {
    /// Render the nominal identity explicitly so a snapshot cannot mistake the name for an authority.
    fn render_snapshot(&self) -> String {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    const fn as_str(self) -> &'static str {
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
#[derive(Debug, Clone, PartialEq)]
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
    fn render_snapshot(&self) -> String {
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
#[derive(Debug, Clone, PartialEq)]
pub struct SpreadElement {
    /// The sequence or mapping being spliced.
    pub source: Operand,
    /// Which spread form the source spelled.
    pub kind: SpreadKind,
}

impl SpreadElement {
    /// Render a deterministic maintainer-facing spelling, marked by the source spread form.
    fn render_snapshot(&self) -> String {
        let marker = match self.kind {
            SpreadKind::Sequence => "*",
            SpreadKind::Mapping => "**",
        };
        format!("{marker}{}", self.source.render_snapshot())
    }
}

/// Which spread form a [`SpreadElement`] was spelled with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadKind {
    /// `*source` — splices a sequence's elements positionally.
    Sequence,
    /// `**source` — splices a mapping's entries by key.
    Mapping,
}

/// One entry of a dict literal, in written source order.
///
/// A dict is built from an ordered sequence of *effects*, not from a finished mapping: keys are expressions
/// evaluated at runtime, their evaluation order is observable, and a duplicate key is legal and meaningful
/// (a later entry overwrites an earlier one). That is why [`Rvalue::Dict`] carries this ordered list rather than a
/// map — a map would have to collapse duplicates at construction, which is exactly the override semantics that
/// must survive, and it could not hold a spread whose keys are unknown until runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum DictEntry {
    /// A `key: value` pair. Both operands are evaluated, key first.
    Pair(Operand, Operand),
    /// A `**source` spread. Its entries are spliced in at this position, and its key set is a runtime fact.
    Spread(SpreadElement),
}

impl DictEntry {
    /// Render a deterministic maintainer-facing spelling.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Pair(key, value) => format!("{}: {}", key.render_snapshot(), value.render_snapshot()),
            Self::Spread(spread) => spread.render_snapshot(),
        }
    }
}

/// Aggregate value shape built by [`Rvalue::Aggregate`].
#[derive(Debug, Clone, PartialEq)]
pub enum AggregateKind {
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
    /// nothing -- so modelling it as a [`Callee::Helper`] call would invent a runtime dependency that the `for`
    /// header's own normalization proves is not there. That is also why range construction records no
    /// [`crate::AbiV0RuntimeRequirement`], the same as [`Self::Tuple`] and unlike [`Self::List`]/[`Self::Set`].
    ///
    /// Operands appear in exactly [`Self::RANGE_FIELDS`] order, and a consumer reading one back projects it by
    /// that name (`PlaceElem::Field("start")`). Inclusivity is an *operand*, not a static property of this
    /// variant: `..` versus `..=` is fixed per construction site, but the site that constructs a range and the
    /// loop that later iterates it need not be the same statement, so a consumer holding only the value must be
    /// able to read which one it is instead of having to prove where it came from.
    Range,
    Constructor(ConstructorTarget),
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
    fn as_str(&self) -> String {
        match self {
            Self::Tuple => "tuple".to_string(),
            Self::List => "list".to_string(),
            Self::Set => "set".to_string(),
            Self::Range => "range".to_string(),
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
    /// No declared parameter slots were resolved for this call's arguments.
    ///
    /// **No declared-slot claim is made, and elements are in written source order — not slot order.** A consumer
    /// must inspect each [`ArgumentElement`] rather than assume element `i` fills parameter `i`: an element may be
    /// a single value, a value written with a name, or a spread whose length is only known at runtime. Reading this
    /// as a positional argument vector is the mistake this variant exists to prevent.
    ///
    /// This is the honest representation for a callee whose declared surface Body IR did not resolve — a builtin, a
    /// compiler-synthesized collection-growth call, or a signature with a `*args`/`**kwargs` rest parameter — and
    /// for any call carrying a spread, where a written argument does not correspond one-to-one with a declared
    /// slot. Recording an identity slot map for those would assert a binding nobody checked.
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
    /// An admitted provider-service operation, carrying the checked plan its execution needs (#1213).
    ///
    /// Deliberately a sibling of [`Self::Function`] rather than a flavor of it. A named function target says which
    /// declaration was selected and nothing more; a provider operation additionally carries the provider activation,
    /// the RFC 104 capability its invocation must be authorized against, and the runtime requirements its execution
    /// imposes. Folding those onto [`NamedCallableTarget`] would give every ordinary call fields that are only ever
    /// absent, and would let a consumer treat a call with no plan as an unchecked provider operation.
    ///
    /// Boxed because the plan is by far the largest payload a callee can carry — two canonical identities, the
    /// provider record, and one fact per input — while being the rarest. Inlining it would grow every
    /// [`StatementKind::Call`] in every body by that much.
    ProviderOperation(Box<ProviderOperationPlan>),
}

impl Callee {
    /// Render a deterministic maintainer-facing spelling for this callee.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Function(target) => target.render_snapshot(),
            Self::Method(target) => target.render_snapshot(),
            Self::Helper(op) => format!("helper:{}", op.as_str()),
            Self::ProviderOperation(plan) => plan.render_snapshot(),
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
    /// A direct call to a named Incan function.
    ///
    /// The callee's source-level spelling is always present. Which physical declaration that spelling binds is
    /// carried separately and only where it is proven: [`NamedCallableTarget::canonical`] for the declaration
    /// identity, [`NamedCallableTarget::direct_call_id`] for a same-module span identity. Resolution through traits,
    /// and through overloads whose candidates a name cannot separate, remains deferred past v0, so both facts are
    /// absent for those and the spelling alone is never a dispatch key.
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
    /// reject an absent `direct_call_id` and absent `builtin` rather than using [`Self::name`] as a dispatch key —
    /// or resolve [`Self::canonical`] through [`BodyIrModule::body_for_canonical_target`], which is the only route
    /// that also reaches a declaration this module does not own.
    pub builtin: Option<BuiltinFnId>,
    /// Resolved explicit call-site type arguments, in declared type-parameter order. Empty when the call site wrote
    /// none.
    pub type_args: Vec<IncanType>,
    /// Resolved binding of the surrounding [`StatementKind::Call::args`] to this function's declared parameters.
    pub binding: ArgumentBinding,
    /// RFC 120 canonical identity of the declaration this call selected, independent of the call site's spelling.
    ///
    /// [`Self::direct_call_id`] is a *span* identity and so exists only for a declaration physically present in this
    /// module. This answers the different question of *which declaration* was selected, in a form that survives an
    /// import, an alias, or a re-export, and is `None` whenever that answer is not proven. [`Self::name`] remains the
    /// call site's own spelling, so the pair records both what was written and what it means.
    pub canonical: Option<CanonicalSymbolId>,
}

/// Whether the provider owning an operation can be executed against in this compilation.
///
/// The three states are the ones the checked provider catalog already distinguishes, kept apart here for the same
/// reason it keeps them apart: they have different remedies. A disabled provider is present but not selected by the
/// project graph, while an unavailable one is selected but has no locally verified artifact. Collapsing them into a
/// single "not usable" flag would leave a consumer unable to say which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderActivationState {
    /// Enabled and locally available, so an admitted operation may be planned for execution.
    Active,
    /// Known to the catalog but not enabled by the project or component selection.
    Disabled,
    /// Enabled, but no locally verified artifact backs it.
    Unavailable,
}

impl ProviderActivationState {
    /// Compact maintainer-facing spelling used in snapshots and refusal text.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Unavailable => "unavailable",
        }
    }
}

/// The checked provider an operation belongs to, and whether this compilation can execute against it.
///
/// [`Self::provider_key`] is carried as provenance, never as a dispatch key. Nothing in Body IR or in a consumer may
/// branch on it: which operation was invoked is answered by [`ProviderOperationPlan::operation`], a canonical
/// identity, and whether it may run is answered by [`Self::state`]. Recording the key anyway is what lets a receipt
/// or a diagnostic say *which* provider without re-resolving the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderActivation {
    /// Stable key of the catalog record this operation was admitted from.
    pub provider_key: String,
    /// Canonical module path the operation is declared under, as the catalog claims it.
    pub module_path: Vec<String>,
    /// Whether this compilation can execute against the provider.
    pub state: ProviderActivationState,
}

impl ProviderActivation {
    /// Render a deterministic maintainer-facing spelling.
    fn render_snapshot(&self) -> String {
        format!(
            "{}@{}:{}",
            self.provider_key,
            self.module_path.join("."),
            self.state.as_str()
        )
    }
}

/// One provider-operation input, described alongside its runtime call operand rather than re-carried.
///
/// The evaluated *values* stay in the surrounding [`StatementKind::Call::args`], exactly as they do for every other
/// call shape, because an [`Operand`] carries an ownership decision and a last-use marker that must be recorded once
/// and only once. Duplicating the operand into the plan would record both twice. What the plan adds is the facts a
/// consumer cannot recover from the operand alone: which declared slot the value fills, where it was written (and so
/// when it was evaluated relative to its siblings), its checked type, and its own source span — which is what lets an
/// authority denial point at the argument that carried an out-of-scope value rather than at the whole call.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderOperationInput {
    /// Declared parameter slot this input supplies. Indexes the same slot space as [`BoundArgument::slot`].
    pub slot: usize,
    /// Zero-based position among the call site's written arguments, which is also its evaluation order.
    pub written_position: usize,
    /// Checked type of the evaluated argument expression.
    pub ty: IncanType,
    /// Span of the argument expression itself.
    pub span: HirSourceSpan,
}

impl ProviderOperationInput {
    /// Render a deterministic maintainer-facing spelling.
    fn render_snapshot(&self) -> String {
        format!("slot{}@{}:{}", self.slot, self.written_position, self.ty)
    }
}

/// A checked execution plan for one resolved provider-operation invocation (#1213).
///
/// This is a compiler-owned internal plan, not a public provider API and not a second source-meaning model. Every
/// fact on it is *consumed* from an upstream owner rather than decided here: the canonical operation identity comes
/// from the RFC 120 callable-target contract, the provider and its activation come from the checked provider catalog,
/// and [`Self::required_capability`] is an RFC 104 capability declaration's own canonical identity. The plan
/// deliberately holds no grant set, no mode, no decision, and no receipt — obtaining an authority decision is the
/// consumer's call through [`Self::authority_request`], and executing the operation belongs to whoever holds a
/// runtime.
///
/// Lowering emits a plan only for an *admitted* operation. An operation whose provider is not active, whose required
/// capability identity does not name a capability, or whose inputs cannot all be represented is refused at its
/// source span instead, so it has no plan to execute and no receipt to emit. Consumers must still validate a plan:
/// constructed or corrupt IR is never evidence that source admission occurred.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderOperationPlan {
    /// RFC 120 canonical identity of the operation declaration this call selected.
    ///
    /// This is the only thing that answers "which operation is this". A consumer must not recover that answer from a
    /// call-site spelling, a provider name, or an emitted Rust name.
    pub operation: CanonicalSymbolId,
    /// The checked provider that owns the operation, and its activation in this compilation.
    pub provider: ProviderActivation,
    /// Canonical identity of the RFC 104 capability this invocation's authority must be decided against.
    ///
    /// A capability identity, not a grant spelling: the spelling is a rendering of the identity (see
    /// [`Self::authority_request`]), and only the identity survives an import, an alias, or a re-export.
    pub required_capability: CanonicalSymbolId,
    /// Runtime requirements executing this operation imposes, in the catalog's own order.
    pub runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    /// Runtime call inputs, one per declared slot, in written source order.
    pub inputs: Vec<ProviderOperationInput>,
    /// Span of the invocation, which is where a refusal or a denial is reported.
    pub call_span: HirSourceSpan,
}

impl ProviderOperationPlan {
    /// Build the RFC 104 authority request this invocation must have decided before it may execute.
    ///
    /// This is the whole of the plan's relationship with authority: it phrases the question and hands it to whatever
    /// implements [`crate::authority::AuthorityDecisionSource`]. It does not consult a grant set, interpret a mode, or
    /// decide anything, so a governed host, a permissive local run, and a test double all answer the same request.
    ///
    /// `requested_scope` is empty. RFC 104 binds scope *values* at grant time and checks them against the operation's
    /// actual attributes at the moment it happens, so a request assembled during lowering has no scope values to
    /// carry; a consumer that has evaluated [`Self::inputs`] may add them to the returned request.
    pub fn authority_request(&self) -> crate::authority::AuthorityRequest {
        crate::authority::AuthorityRequest {
            capability: self.required_capability.clone(),
            operation: self.operation.clone(),
            request_span: self.call_span,
            requested_scope: Vec::new(),
            suggested_grant: render_symbol_path(&self.required_capability),
        }
    }

    /// Render a deterministic maintainer-facing spelling.
    fn render_snapshot(&self) -> String {
        let inputs: Vec<String> = self
            .inputs
            .iter()
            .map(ProviderOperationInput::render_snapshot)
            .collect();
        let requirements: Vec<String> = self
            .runtime_requirements
            .iter()
            .map(render_runtime_requirement)
            .collect();
        format!(
            "provider_operation:{} provider={} capability={} inputs=[{}] requires=[{}]",
            render_symbol_path(&self.operation),
            self.provider.render_snapshot(),
            render_symbol_path(&self.required_capability),
            inputs.join(", "),
            requirements.join(", ")
        )
    }
}

/// Render a canonical identity as its dotted declaration path.
///
/// This is a *projection of* the identity for humans and for RFC 104's suggested-grant spelling, never a substitute
/// for it: two declarations can share a rendering while remaining distinct identities, so nothing may compare or
/// dispatch on the result. The origin is included because a capability's grant spelling is its declaration location —
/// `host.http.request` is the `request` capability declared in the `host.http` module, not a string an author chose.
fn render_symbol_path(symbol: &CanonicalSymbolId) -> String {
    let prefix: Vec<String> = match &symbol.origin {
        crate::SymbolOrigin::Module(path) | crate::SymbolOrigin::RustCrate(path) => path.clone(),
        crate::SymbolOrigin::Package { library, module_path } => std::iter::once(library.clone())
            .chain(module_path.iter().cloned())
            .collect(),
        crate::SymbolOrigin::Builtin => Vec::new(),
    };
    prefix
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(symbol.declaration_name.as_str()))
        .collect::<Vec<_>>()
        .join(".")
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
    /// Canonical declared field names retained with the exact source-local declaration selected for this call.
    ///
    /// This is present only beside [`Self::direct_declaration_id`] and comes from the same checked Body-IR
    /// declaration snapshot. Consumers compare it with the module declaration before binding operands, so a
    /// malformed declaration cannot shift values merely by changing field order or names after lowering.
    pub canonical_field_layout: Option<Vec<String>>,
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

/// Compiler-owned runtime/helper operation: a source operator whose meaning is a call into the runtime rather than
/// a machine instruction. Represented here as an explicit Body IR operation rather than inferred later from
/// generated Rust call shapes.
///
/// Most of these mirror a helper the existing Rust-emission backend already generates for string and list operators
/// (see `src/backend/ir/conversions.rs::determine_binop_plan`). The membership helpers do not: that backend reaches
/// containment through a `contains` method call rather than a binary-operator plan, so every `*_contains` name here
/// states a runtime requirement Body IR needs and the runtime must satisfy, not a call shape read back out of
/// emitted Rust. Each variant's [`Self::as_str`] name *is* that requirement's name; it is not a promise that a Rust
/// function of exactly that signature already exists.
///
/// Membership and concatenation are named per operand type rather than shared across collections. One `Contains`
/// helper would oblige a consumer to re-derive string-versus-list-versus-set-versus-dict from operand types, which
/// is the inference this data model exists to replace; and one `Concat` would conflate a string join with a list
/// concatenation. Negation likewise gets its own variant per collection rather than a wrapper, so one source
/// operator stays one Body IR operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperOp {
    StrConcat,
    StrEq,
    StrNe,
    StrLt,
    StrLe,
    StrGt,
    StrGe,
    /// `needle in haystack` on two strings: substring containment, not element membership.
    ///
    /// Membership is the one operator whose meaning genuinely changes with its operand types — substring for `str`,
    /// element lookup for a collection — so it cannot be a [`BinOp`] without picking one of those meanings and
    /// silently applying it to the other. Naming the string policy as its own helper is what makes the substring
    /// choice a represented fact.
    ///
    /// **Argument order is the helper's own: `(haystack, needle)`.** Membership is the one string operator whose
    /// surface order is the reverse of its signature — `needle in haystack` reads needle-first, while `str_contains`
    /// takes the haystack first, as every `contains` in Rust does. Lowering swaps the operands so the call matches
    /// the function it names. Preserving source order instead would leave one helper out of nine disagreeing with
    /// its own signature, and a backend binding these positionally has no way to know which one.
    StrContains,
    /// `needle not in haystack` on two strings, with the same `(haystack, needle)` argument order as
    /// [`Self::StrContains`].
    ///
    /// A separate helper rather than a negated [`Self::StrContains`], following the [`Self::StrEq`]/[`Self::StrNe`]
    /// pair: one source operator stays one Body IR operation, so a consumer never has to recognize a negation
    /// wrapper to know which operator was written.
    StrNotContains,
    /// `xs + ys` on two lists: concatenation producing a new list, not a machine addition.
    ///
    /// Lists are the one non-string builtin whose `+` the typechecker accepts through a dedicated branch rather than
    /// through a `__add__` hook. Because that branch resolves no operator dispatch, nothing downstream would mark the
    /// operation as a call, and a plain [`BinOp::Add`] would have contradicted the Rust-emission backend outright:
    /// `determine_binop_plan` routes list `+` to `incan_stdlib::collections::list_concat`, so no addition is emitted
    /// for it at all. Naming the concatenation as its own helper is what keeps the two backends stating the same
    /// thing.
    ///
    /// This is specifically *not* an argument that a collection operand cannot sit under a [`BinOp`]. That same
    /// backend emits comparisons as an infix operator, so `==` on two lists is faithfully a primitive, and refusing
    /// it would invent a divergence rather than close one.
    ///
    /// Arguments are in source order, `(lhs, rhs)`: concatenation is not commutative, and unlike the membership
    /// helpers there is no receiver convention pulling the operands apart.
    ListConcat,
    /// `v in xs` on a list: element containment, not substring containment.
    ///
    /// Membership is the one operator whose meaning genuinely changes with its operand types, so each collection
    /// names its own helper rather than sharing one `Contains`. A single variant would leave the backend to re-derive
    /// list-versus-set-versus-dict from operand types — exactly the inference this data model exists to replace with
    /// a represented fact.
    ///
    /// **Argument order is the helper's own: `(haystack, needle)`**, matching [`Self::StrContains`] and every
    /// `contains` in Rust, while the source spelling reads needle-first. Lowering swaps the operands so all
    /// membership helpers agree with each other and with their own signatures.
    ListContains,
    /// `v not in xs` on a list, with the same `(haystack, needle)` argument order as [`Self::ListContains`].
    ///
    /// A separate helper rather than a negated [`Self::ListContains`], following the
    /// [`Self::StrContains`]/[`Self::StrNotContains`] pair: one source operator stays one Body IR operation, so a
    /// consumer never has to recognize a negation wrapper to know which operator was written.
    ListNotContains,
    /// `v in xs` on a set: element containment, with the same `(haystack, needle)` argument order as
    /// [`Self::ListContains`].
    ///
    /// Distinct from [`Self::ListContains`] despite the identical source spelling, because the containers are
    /// different types with different lookup costs and different Rust receivers. The operation records which one the
    /// source actually held.
    SetContains,
    /// `v not in xs` on a set, with the same `(haystack, needle)` argument order as [`Self::SetContains`].
    SetNotContains,
    /// `k in d` on a dict: **key** containment, with the same `(haystack, needle)` argument order as
    /// [`Self::ListContains`].
    ///
    /// Named `ContainsKey` rather than `Contains` because dict membership tests keys while the sibling collections
    /// test elements. Leaving that to be inferred from the receiver's type would make key-versus-value a backend
    /// convention; naming it here makes it a represented fact, and matches the `contains_key` the operation lowers
    /// toward.
    DictContainsKey,
    /// `k not in d` on a dict, with the same `(haystack, needle)` argument order as [`Self::DictContainsKey`].
    DictNotContainsKey,
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
            Self::StrContains => "str_contains",
            Self::StrNotContains => "str_not_contains",
            Self::ListConcat => "list_concat",
            Self::ListContains => "list_contains",
            Self::ListNotContains => "list_not_contains",
            Self::SetContains => "set_contains",
            Self::SetNotContains => "set_not_contains",
            Self::DictContainsKey => "dict_contains_key",
            Self::DictNotContainsKey => "dict_not_contains_key",
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
            let args_str: Vec<String> = args.iter().map(ArgumentElement::render_snapshot).collect();
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
            kind,
            message,
            may_panic,
        } => {
            let msg = message
                .as_ref()
                .map(|m| format!(", {}", m.render_snapshot()))
                .unwrap_or_default();
            let panic_marker = if *may_panic { " may_panic" } else { "" };
            let _ = writeln!(out, "{indent}assert {}{msg}{panic_marker}", kind.render_snapshot());
        }
        StatementKind::Expr { value } => {
            let _ = writeln!(out, "{indent}expr {}", value.render_snapshot());
        }
        StatementKind::TryPropagate {
            destination,
            operand,
            error_routing,
        } => {
            let _ = writeln!(
                out,
                "{indent}{} = try?({}, {})",
                destination.render_snapshot(),
                operand.render_snapshot(),
                error_routing.render_snapshot()
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
        args: Vec<ArgumentElement>,
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
    /// - when two or more awaitables become ready in the same scheduler turn, the earliest source-order arm wins;
    /// - exactly one arm's `body` runs, the one whose awaitable completed first after applying that ready-tie rule;
    /// - every non-winning arm's awaitable is cancelled at this statement's boundary.
    ///
    /// As with [`Self::Await`], this records what the race *means*, not how it is driven: concurrent polling,
    /// first-completion detection, and the cancellation mechanism belong to #1155.
    Race {
        /// Where the winning arm's result is written. Always supplied by frontend lowering today, for the same
        /// reason as [`StatementKind::Await::destination`].
        destination: Option<Place>,
        /// The arms, in source order. Order resolves only same-turn ready ties — see this variant's docs.
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
    /// One RFC 018 assertion in any of its three source forms, with the optional failure message and panic marker
    /// every form shares.
    ///
    /// The form-specific payload lives in [`AssertionKind`] rather than in three sibling statement kinds, because
    /// `message` and `may_panic` are invariant across all three forms and every assertion records the same
    /// [`PanicReason::AssertFailure`] fact and [`crate::AbiV0RuntimeRequirement::PanicStrategy`] requirement. Three
    /// sibling kinds would repeat those shared fields three times, let them drift apart, and force every exhaustive
    /// walk over this vocabulary to grow three arms for one concept; a consumer that only needs "is this an
    /// assertion" still matches one variant here, and one that cares which form matches the payload.
    Assert {
        /// Which assertion form this is, plus the operands and facts only that form carries.
        kind: AssertionKind,
        /// Optional failure message, applicable to all three forms.
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
    TryPropagate {
        destination: Place,
        operand: Operand,
        /// Checked routing fact for the failure payload. A direct runtime supports only the exact same error type;
        /// a conversion remains represented so it can refuse without guessing an `Into`/`From` implementation.
        error_routing: TryErrorRouting,
    },
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

/// The form-specific payload of one [`StatementKind::Assert`], covering all three RFC 018 assertion spellings.
///
/// See [`StatementKind::Assert`] for why the three forms are one variant with this payload rather than three
/// sibling statement kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum AssertionKind {
    /// `assert cond`: panics when `cond` is false.
    Condition { cond: Operand },
    /// `assert value is P`: panics when `value` does not match `P`, and on the matching path binds `P`'s names for
    /// the remainder of the enclosing block.
    ///
    /// The bindings are ordinary declared locals carried inside `pattern`'s [`PatternBinding`]s, exactly as a
    /// `match` arm's are, so a consumer reading one of those names later finds a local this body declared rather
    /// than an unresolved name. `scrutinee` is read as [`OwnershipFact::Borrow`] for the same reason
    /// [`Rvalue::Match::scrutinee`] is: the overall read must not risk an unconditional move while individual
    /// bindings compute their own, more precise facts against places projected out of it.
    Pattern { scrutinee: Operand, pattern: Pattern },
    /// `assert call() raises E`: evaluates `call`, expecting it to raise a runtime error of type `E`, and panics
    /// when it does not.
    ///
    /// `expected_error` is the resolved builtin-exception identity, not the source spelling after `raises`, so a
    /// consumer never has to re-resolve a name against the exception registry to know which error is expected.
    Raises { call: Operand, expected_error: ErrorKind },
}

impl AssertionKind {
    /// Render the form-specific part of an assertion's snapshot line, without the shared message/panic suffix.
    fn render_snapshot(&self) -> String {
        match self {
            Self::Condition { cond } => cond.render_snapshot(),
            Self::Pattern { scrutinee, pattern } => {
                format!("{} is {}", scrutinee.render_snapshot(), pattern.render_snapshot())
            }
            Self::Raises { call, expected_error } => {
                format!("{} raises {}", call.render_snapshot(), errors::as_str(*expected_error))
            }
        }
    }
}

/// Typechecker-proven error routing selected for one `?` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TryErrorRouting {
    /// The operand and enclosing Result have the same exact error type, so direct propagation needs no conversion.
    SameType { error_type: IncanType },
    /// Source semantics selected a conversion between distinct error types. The direct runtime intentionally does
    /// not reconstruct that conversion until Body IR retains the conversion authority itself.
    ConversionRequired {
        source_error_type: IncanType,
        destination_error_type: IncanType,
    },
    /// Lowering did not retain enough checked Result type information to describe error routing honestly.
    Unresolved,
}

impl TryErrorRouting {
    /// Stable maintainer-facing rendering paired with the `try?` snapshot line.
    fn render_snapshot(&self) -> String {
        match self {
            Self::SameType { error_type } => format!("same_error_type={error_type}"),
            Self::ConversionRequired {
                source_error_type,
                destination_error_type,
            } => format!("error_conversion={source_error_type}->{destination_error_type}"),
            Self::Unresolved => "error_routing=unresolved".to_string(),
        }
    }
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
    use crate::{CompilerNodeKind, IncanPrimitiveType, SemanticSourceTargetKind};

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
    fn a_dict_spread_cannot_be_mispaired_with_the_following_key() {
        // With a flattened key/value list, `{**base, "x": 1}` had to be walked carefully or the spread source
        // would pair with the following key. Structured entries make that state unrepresentable; this pins the
        // rendering that proves the entries stay distinct.
        let entries = vec![
            DictEntry::Spread(SpreadElement {
                source: Operand::Constant(Constant::Str("base".to_string())),
                kind: SpreadKind::Mapping,
            }),
            DictEntry::Pair(
                Operand::Constant(Constant::Str("x".to_string())),
                Operand::Constant(Constant::Int(1)),
            ),
        ];

        assert_eq!(
            Rvalue::Dict(entries).render_snapshot(),
            "dict[**const(\"base\"), const(\"x\"): const(1)]"
        );
    }

    #[test]
    fn a_non_spread_element_renders_exactly_as_its_operand_did() {
        // Every pre-existing snapshot assertion depends on this: wrapping operands in `ArgumentElement` must not
        // change how an ordinary aggregate or call renders.
        let operand = Operand::Constant(Constant::Int(7));
        assert_eq!(
            ArgumentElement::One(operand.clone()).render_snapshot(),
            operand.render_snapshot()
        );
        assert_eq!(
            ArgumentElement::Spread(SpreadElement {
                source: operand.clone(),
                kind: SpreadKind::Sequence,
            })
            .render_snapshot(),
            format!("*{}", operand.render_snapshot())
        );
        assert_eq!(
            ArgumentElement::Spread(SpreadElement {
                source: operand.clone(),
                kind: SpreadKind::Mapping,
            })
            .render_snapshot(),
            format!("**{}", operand.render_snapshot())
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
                    args: vec![ArgumentElement::One(Operand::Constant(Constant::Str("a".to_string())))],
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
                    args: vec![ArgumentElement::One(Operand::Constant(Constant::Int(1)))],
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
                    rvalue: Rvalue::Dict(vec![DictEntry::Pair(
                        Operand::Constant(Constant::Str("a".to_string())),
                        Operand::Constant(Constant::Int(1)),
                    )]),
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
                    rvalue: Rvalue::Aggregate(
                        AggregateKind::Set,
                        vec![ArgumentElement::One(Operand::Constant(Constant::Int(1)))],
                    ),
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
                    error_routing: TryErrorRouting::SameType {
                        error_type: IncanType::Named("E".to_string()),
                    },
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();
        assert!(snapshot.contains("_2 = try?(move(_0, last_use), same_error_type=E)"));
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
            fieldless_enum_declarations: Vec::new(),
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

    /// Build an admitted plan for a `pub::billing.charge` operation requiring `host.http.request`.
    fn sample_provider_operation_plan() -> ProviderOperationPlan {
        ProviderOperationPlan {
            operation: CanonicalSymbolId {
                namespace: crate::SymbolNamespace::OrdinaryLexical,
                origin: crate::SymbolOrigin::Package {
                    library: "billing".to_string(),
                    module_path: vec!["api".to_string()],
                },
                declaration_name: "charge".to_string(),
                kind: SemanticSourceTargetKind::Function,
                scope_discriminant: None,
                declaration_span: HirSourceSpan::new(40, 60),
            },
            provider: ProviderActivation {
                provider_key: "billing@1.2.3#abc[]".to_string(),
                module_path: vec!["billing".to_string(), "api".to_string()],
                state: ProviderActivationState::Active,
            },
            required_capability: CanonicalSymbolId::module_declaration(
                vec!["host".to_string(), "http".to_string()],
                "request",
                SemanticSourceTargetKind::Capability,
                HirSourceSpan::new(10, 20),
            ),
            runtime_requirements: vec![AbiV0RuntimeRequirement::HostedStd],
            inputs: vec![ProviderOperationInput {
                slot: 0,
                written_position: 0,
                ty: IncanType::Primitive(IncanPrimitiveType::Str),
                span: HirSourceSpan::new(120, 128),
            }],
            call_span: HirSourceSpan::new(110, 130),
        }
    }

    /// The plan phrases the RFC 104 question without answering any part of it itself.
    #[test]
    fn a_plan_builds_an_authority_request_naming_its_capability_and_operation() {
        let plan = sample_provider_operation_plan();

        let request = plan.authority_request();

        assert_eq!(request.capability, plan.required_capability);
        assert_eq!(request.operation, plan.operation);
        assert_eq!(request.request_span, plan.call_span);
        assert!(
            request.requested_scope.is_empty(),
            "scope values bind at grant time, not while lowering",
        );
    }

    /// The suggested grant is a rendering of the capability's declaration location, never an author-chosen string.
    #[test]
    fn a_capabilitys_grant_spelling_comes_from_where_it_was_declared() {
        let plan = sample_provider_operation_plan();

        assert_eq!(plan.authority_request().suggested_grant, "host.http.request");
    }

    /// A package-owned operation renders under its library identity, so two libraries cannot collide.
    #[test]
    fn a_package_owned_operation_renders_under_its_library_identity() {
        let plan = sample_provider_operation_plan();

        assert_eq!(render_symbol_path(&plan.operation), "billing.api.charge");
    }

    /// A provider operation renders its checked facts, so a snapshot shows what would execute.
    #[test]
    fn a_provider_operation_call_renders_its_plan() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Call {
                    destination: Some(Place::from_local(LocalId(2))),
                    callee: Callee::ProviderOperation(Box::new(sample_provider_operation_plan())),
                    args: vec![ArgumentElement::One(Operand::Constant(Constant::Str("x".to_string())))],
                    may_panic: false,
                },
                span: HirSourceSpan::new(110, 130),
            },
        );

        let snapshot = body.render_snapshot();

        assert!(
            snapshot.contains("provider_operation:billing.api.charge"),
            "the canonical operation identity must be visible: {snapshot}"
        );
        assert!(
            snapshot.contains("capability=host.http.request"),
            "the required capability must be visible: {snapshot}"
        );
        assert!(
            snapshot.contains("provider=billing@1.2.3#abc[]@billing.api:active"),
            "provider activation must be visible: {snapshot}"
        );
        assert_eq!(body.render_snapshot(), snapshot, "rendering must be deterministic");
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

    #[test]
    fn bytes_constants_and_range_aggregates_render_distinctly() {
        let mut body = sample_body();
        body.block.stmts.insert(
            0,
            Statement {
                kind: StatementKind::Assign {
                    place: Place::from_local(LocalId(2)),
                    rvalue: Rvalue::Use(Operand::Constant(Constant::Bytes(b"hi".to_vec()))),
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        body.block.stmts.insert(
            1,
            Statement {
                kind: StatementKind::Assign {
                    place: Place::from_local(LocalId(2)),
                    rvalue: Rvalue::Aggregate(
                        AggregateKind::Range,
                        vec![
                            ArgumentElement::One(Operand::Constant(Constant::Int(0))),
                            ArgumentElement::One(Operand::Constant(Constant::Int(10))),
                            ArgumentElement::One(Operand::Constant(Constant::Int(1))),
                            ArgumentElement::One(Operand::Constant(Constant::Bool(true))),
                        ],
                    ),
                },
                span: HirSourceSpan::new(0, 1),
            },
        );
        let snapshot = body.render_snapshot();

        // A byte string escapes every byte, so it can never be mistaken for the `str` constant with the same
        // contents -- which is exactly the conflation `Constant::Bytes` exists to prevent.
        assert!(
            snapshot.contains("const(b\"\\x68\\x69\")"),
            "bytes constant: {snapshot}"
        );
        assert_ne!(
            Constant::Bytes(b"hi".to_vec()).render_snapshot(),
            Constant::Str("hi".to_string()).render_snapshot(),
            "a bytes constant and a string constant with the same contents must not render alike"
        );
        assert!(
            snapshot.contains("range[const(0), const(10), const(1), const(true)]"),
            "range aggregate in RANGE_FIELDS order: {snapshot}"
        );
        assert_eq!(AggregateKind::RANGE_FIELDS.len(), 4);
    }
}
