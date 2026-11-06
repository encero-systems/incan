//! Incan Body IR v0 data model.
//!
//! Body IR v0 is the backend-facing, target-agnostic representation of one function/method body. It sits between
//! typechecked source (AST + [`crate::SemanticFactStore`] / declaration-level [`crate::HirModule`]) and the
//! target-specific native lowering in `loaves/compiler/incan_mir_lowering` — it must be consumable by a backend
//! without that backend needing to read generated-Rust semantics or private compiler internals.
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
//! remain inside the owned [`GeneratorBody`]. Expression-position `yield` remains a stub.
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

use incan_lang::errors::ErrorKind;
/// The compiler-owned builtin identities carried by checked call targets.
pub use incan_lang::lang::builtins::{BuiltinFnId, BuiltinMethodId};
use incan_lang::lang::errors;
use incan_lang::lang::surface::string_methods::StringMethodId;
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics::NumericTypeId;
use serde::{Deserialize, Serialize};

use crate::{
    AbiV0RuntimeRequirement, CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanCallableParamKind, IncanType,
    SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

mod bodies;
mod callable_values;
mod callees;
mod enum_declarations;
mod locals_and_places;
mod module_declarations;
mod nominal_declarations;
mod operations;
mod patterns_and_formats;
mod statements;
mod values;

pub use bodies::*;
pub use callable_values::*;
pub use callees::*;
pub use enum_declarations::*;
pub use locals_and_places::*;
pub use module_declarations::*;
pub use nominal_declarations::*;
pub use operations::*;
pub use patterns_and_formats::*;
pub use statements::*;
pub use values::*;

// ============================================================================
// Module / body containers
// ============================================================================

/// One module's lowered function/method bodies and direct-execution declaration facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyIrModule {
    /// Consumer-selected runtime facades authenticated by the installed Loaf manifest. Public ABI spellings select
    /// canonical declarations independently of private same-named dependencies; publication never serializes them.
    #[serde(skip)]
    pub runtime_callable_bindings: Vec<RuntimeCallableBinding>,
    /// RFC 036 originals retained separately from the initialized, module-visible callable bindings.
    #[serde(default)]
    pub decorated_bindings: Vec<DecoratedBinding>,
    /// Setup effects of this module's selected entrypoint, evaluated before its source body. Dependency modules do
    /// not lend their effects to a different entry module.
    #[serde(default)]
    pub entrypoint_effects: Vec<EntrypointEffect>,
    /// Canonical mains explicitly selected by the unit for separate process invocations, in selector order. This
    /// invocation-only selection is never published in a Loaf's positional executable representation.
    #[serde(skip)]
    pub program_entrypoints: Vec<CanonicalSymbolId>,
    /// Consumer invocation's source-import union and authenticated linked conversion contract, retained only on its
    /// final entry module. Ordinary source builders and published fragments leave this unselected.
    #[serde(default)]
    pub string_conversion_contract: StringConversionContract,
    /// Checker-evaluated scalar and text constants, addressed by their declaring canonical identity.
    #[serde(default)]
    pub constant_declarations: Vec<ConstantDeclaration>,
    /// Alias-expanded checked types for nongeneric source aliases; aliases introduce no runtime storage.
    #[serde(default)]
    pub type_alias_declarations: Vec<TypeAliasDeclaration>,
    /// Checked source-local nongeneric enum layouts, including canonical payload variants and selected derives.
    #[serde(default)]
    pub enum_declarations: Vec<EnumDeclaration>,
    /// External callables: linked Loaf declarations (DD-0007) and checker-selected Rust instantiations (DD-0006).
    /// Each record names the exact compiled item and signature the native adapter must verify.
    #[serde(default)]
    pub loaf_callables: Vec<LoafCallable>,
    /// Checked source union payload contracts and exact layouts admitted from selected Loaf artifacts.
    ///
    /// A local symbol retains source ownership; a qualified item preserves the selected producer's native owner and
    /// ordered payloads. A consumer must never infer Loaf ownership from a wrapper's hash.
    #[serde(default)]
    pub native_union_declarations: Vec<NativeUnionDeclaration>,
    /// Source-local persistent storage, including RFC 036 replacement callables, with literal or computed
    /// initialization.
    #[serde(default)]
    pub static_declarations: Vec<StaticDeclaration>,
    /// Identity of the owning module, matching [`crate::HirModule::id`].
    pub module_id: CompilerNodeId,
    /// Source-local plain-model and plain-newtype declarations whose construction layout is available to a direct
    /// runtime.
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
    /// same exact enum/member identities. Consumers may resolve aliases to this context through a published package;
    /// payload variants, traits, and methods remain outside the profile.
    pub fieldless_enum_declarations: Vec<FieldlessEnumDeclaration>,
    /// Source-local RFC 032 value-enum declarations whose canonical scalar members are available to direct execution.
    ///
    /// This is deliberately separate from [`Self::nominal_declarations`]: a value enum has no model field layout,
    /// and the direct runtime may extract only a retained member's scalar backing through the compiler-provided
    /// zero-argument `.value()` surface. Package aliases retain these canonical contexts. Ordinary enums, payload
    /// variants, behavior-bearing enums, and generic enums remain absent and must refuse.
    pub value_enum_declarations: Vec<ValueEnumDeclaration>,
    /// Canonical source-local trait owners; method identity validation uses their physical declaration spans.
    #[serde(default)]
    pub trait_declarations: Vec<CanonicalSymbolId>,
    /// Checked concrete trait slots and the implementation body selected for each adopter.
    #[serde(default)]
    pub trait_implementations: Vec<TraitImplementation>,
    /// One [`Body`] per lowered function/method declaration in the module.
    pub bodies: Vec<Body>,
}

impl BodyIrModule {
    /// A module that only declares, filled by its caller: a Loaf module's resolved declarations, or the Rust items a
    /// program holds (DD-0006). It has no bodies, storage or effects of its own.
    pub fn declarations_only(owner: String) -> Self {
        Self {
            runtime_callable_bindings: Vec::new(),
            string_conversion_contract: StringConversionContract::Unselected,
            decorated_bindings: Vec::new(),
            entrypoint_effects: Vec::new(),
            program_entrypoints: Vec::new(),
            constant_declarations: Vec::new(),
            type_alias_declarations: Vec::new(),
            trait_declarations: Vec::new(),
            trait_implementations: Vec::new(),
            module_id: CompilerNodeId::module(owner),
            loaf_callables: Vec::new(),
            native_union_declarations: Vec::new(),
            enum_declarations: Vec::new(),
            static_declarations: Vec::new(),
            nominal_declarations: Vec::new(),
            fieldless_enum_declarations: Vec::new(),
            value_enum_declarations: Vec::new(),
            bodies: Vec::new(),
        }
    }

    /// Read signatures and defaults from governed bodies and authenticated compiled declaration frames. Compiled
    /// frames contribute declaration facts only; execution and instance scheduling must keep using `bodies`.
    pub fn callable_declaration_frames(&self) -> impl Iterator<Item = &Body> {
        self.bodies.iter().chain(
            self.loaf_callables
                .iter()
                .filter_map(|callable| callable.declaration_frame.as_deref()),
        )
    }

    /// Resolve a canonical callable identity to the body that declares it, when this module owns the declaration.
    ///
    /// This is the consumer seam for [`CanonicalSymbolId`]: a backend asks *which declaration was selected* and gets
    /// the declaration itself or nothing. It never consults the *reference site's* spelling or an emitted Rust name,
    /// so a consumer cannot dispatch on the text at a call site.
    ///
    /// Matching is on the complete checker-minted canonical identity. The declared name deliberately does not
    /// participate: a class method and a free function in one module can both be named `render`, and a name match
    /// would hand back whichever came first.
    ///
    /// `None` has three distinct causes and is never permission to fall back to [`NamedCallableTarget::name`]: the
    /// identity is owned by another source or published module; its origin has no Body-IR module (a `rust::` crate or
    /// builtin); or this module owns it but it has no lowered body, as for a model or a `const`.
    pub fn body_for_canonical_target(&self, target: &CanonicalSymbolId) -> Option<&Body> {
        if crate::canonical_module_identity(target)? != self.module_id.path() {
            return None;
        }
        let mut bodies = self
            .bodies
            .iter()
            .filter(|body| body.canonical.as_ref() == Some(target));
        let body = bodies.next()?;
        bodies.next().is_none().then_some(body)
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
                "fieldless_enum {} id={} canonical={} variants=[{}]",
                declaration.name,
                declaration.direct_declaration_id,
                declaration.canonical.render_compact(),
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
                "value_enum {} id={} canonical={} backing={} variants=[{}]",
                declaration.name,
                declaration.direct_declaration_id,
                declaration.canonical.render_compact(),
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

#[cfg(test)]
mod tests;

/// A body's checked nominal bindings, shared between clones of the body.
pub type SharedIdentities = std::sync::Arc<std::collections::BTreeMap<String, CanonicalSymbolId>>;

/// Serialize [`SharedIdentities`] as the map itself, so the published representation is unchanged.
mod shared_identities {
    use super::{CanonicalSymbolId, SharedIdentities};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(value: &SharedIdentities, serializer: S) -> Result<S::Ok, S::Error> {
        value.as_ref().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SharedIdentities, D::Error> {
        BTreeMap::<String, CanonicalSymbolId>::deserialize(deserializer).map(std::sync::Arc::new)
    }
}
