//! Frontend bridge from typechecked AST function bodies into Body IR v0.
//!
//! Declaration-level HIR ([`crate::frontend::hir`]) does not model statements or expressions at all (see its module
//! docs), so Body IR v0 lowers directly from `ast::FunctionDecl` bodies plus [`TypeCheckInfo`], rather than from a
//! hypothetical body-shaped HIR that does not exist yet. Every [`Body`](incan_semantics_core::body_ir::Body) this
//! module produces carries a [`CompilerNodeId`] identical to the one [`crate::frontend::hir::build_hir_v0`] would
//! assign the same function's [`crate::frontend::hir`] declaration, so the two can be correlated by id without
//! threading a [`crate::frontend::hir`] value through this API.
//!
//! Body IR v0 lowers a representative, explicitly documented subset of the language surface (see
//! [`incan_semantics_core::body_ir`] module docs for the full rationale). Statements fully lowered: assignment
//! (inferred/let/mutable/reassignment), field/index assignment (including their pre-desugared compound `<op>=`
//! forms), compound assignment (`x <op>= y`), tuple unpacking, multi-target (lvalue) tuple assignment, chained
//! assignment, `return`, `if`/`elif`/`else`, `loop:`, `while`, `for` (both a `start..end` range and a general
//! iterable -- builtin collections or a resolved `__iter__`/`__next__` protocol, including the fallible `for item in
//! iterable?:` form), expression statements, statement-position `yield value` (see [`BodyBuilder::lower_stmt_into`]
//! and [`bir::Body::is_generator`]), all three RFC 018 `assert` forms -- plain condition, `assert value is P`
//! (whose bindings stay live for the rest of the enclosing block), and `assert call() raises E` (see
//! [`BodyBuilder::lower_assert`]) -- `pass`, `break` (including a value-producing `break` inside a `loop`
//! expression), `continue`. Expressions fully lowered: identifiers, literals (int/float/decimal/bool/string/bytes),
//! arithmetic/comparison/boolean binary operators and all three unary operators, calls and method calls (including
//! named, out-of-order, defaulted, and explicitly generic argument spellings -- see [`BodyBuilder::lower_call`]),
//! field access, indexing, slicing, parenthesization, range values, tuples, list/dict/set literals (list and dict
//! spread entries included; set literals have no spread spelling), `model`/`class`
//! construction (named-only at the source level, bound to declared field order -- see
//! [`BodyBuilder::lower_nominal_construction`]), expression-position `if`/`loop`, `try` (`?`), f-strings,
//! list/dict comprehensions, lazy generator expressions, closure literals, partial callables (see
//! [`BodyBuilder::lower_closure`]/[`BodyBuilder::lower_partial`] for how captures
//! are computed and represented explicitly rather than left implicit), and `match` (see [`BodyBuilder::lower_match`]
//! for how patterns are lowered and their bindings scoped).
//!
//! Everything else lowers to an explicit `Statement::Unsupported` / `Operand::Unknown` node rather than panicking,
//! so the model stays total over real programs. That residue is now short, and every entry is a decided refusal
//! with a stated reason rather than pending work — #1101 and its sixteen children (#1158 through #1167, #1172,
//! and the #1244/#1246 follow-ons) are closed, so nothing here awaits a named owner. What still refuses: a spread
//! in a `model`/`class` construction, because the typechecker records no field binding for it and a construction
//! bound to declared field order cannot admit an unresolved layout; a spread with no statically proven shape
//! against a callee whose fixed signature *is* resolvable, whose arity no stage can establish; and a spread to a
//! locally held callable value, whose declared slots exist only in the callable's type. All three are the same
//! fact gap — no stage records how a spread maps onto declared slots — and closing them means representing that
//! mapping, not adding a lowering arm. `if let`/`while let`, destructuring comprehension and generator clauses,
//! `await`, `race for`, statement-position `loop:`, bytes literals, and range values, which earlier revisions of
//! this paragraph listed as residue, all lower now.
//!
//! Vocab and scoped-DSL surface nodes are deliberately *not* on that list. They are not unmodeled language
//! constructs awaiting a lowering; the desugar pass resolves them before this module runs, and one reaching here
//! means a caller skipped it. See [`build_body_ir_module_v0`]'s input contract (#1166) for what a caller owes and
//! for how such a node is named when it arrives anyway.
//!
//! One entry in that residue is a decided boundary rather than pending work, and is stated here so it is not read
//! as a gap. An `unsafe:` region refuses permanently: it introduces no Incan scope, so inlining its statements
//! would be trivial, and that is precisely the problem — it would erase the acknowledgement the region exists to
//! record and let a direct replacement execution profile run an explicitly authorized region without ever being
//! told. Body IR v0 carries no acknowledgement fact a consumer could weigh, so the honest answer is a named
//! refusal owned by #1162 rather than a silent inline. See [`BodyBuilder::refuse_unsafe_region`].
//!
//! One coverage limit is silent rather than marked, and it is deliberate. Expression-position `yield` (the two-way
//! send/receive protocol) is a stub in the existing Rust-emission backend too, so there is no behavior to preserve;
//! the typechecker rejects a bare `yield` with no value before lowering runs. Model, class, trait-default, newtype,
//! and enum method bodies all lower through [`lower_owner_method_bodies`].

use std::collections::{HashMap, HashSet};

use incan_core::lang::keywords::KeywordId;
use incan_semantics_core::SurfaceFeatureKey;
use incan_semantics_core::body_ir as bir;
use incan_semantics_core::{
    AbiV0RuntimeRequirement, CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanCallableParam,
    IncanCallableParamKind, IncanPrimitiveType, IncanType, SemanticSourceTargetKind, rust_tuple_arity,
};

use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_core::lang::types::collections::{self, CollectionTypeId};

use crate::frontend::ast;
use crate::frontend::symbols::{CallableParam, ResolvedType};
use crate::frontend::typechecker::{
    FixedUnpackPlan, IdentKind, ResolvedOperatorKind, TypeCheckInfo, semantic_type_from_resolved,
};
use crate::provider::ProviderPlan;

/// Build Body IR v0 for every top-level function declaration and every non-abstract class/model/trait method in a
/// typechecked module.
///
/// `ast::Declaration::Function` items each produce one [`bir::Body`], matching the [`CompilerNodeId`]
/// [`crate::frontend::hir::build_hir_v0`] assigns the corresponding declaration (see that function's docs).
/// `ast::Declaration::Model`/`Class`/`Trait` items additionally contribute one [`bir::Body`] per non-abstract method
/// (#1102) — abstract methods (`body: None`, trait requirements with no implementation) contribute nothing, since
/// there is no body to lower. Method [`CompilerNodeId`]s are *not* assigned by [`crate::frontend::hir::build_hir_v0`]
/// today (declaration-level HIR only assigns ids to top-level declarations), so this function constructs its own
/// method ids by scoping the method name under its owning declaration's name — see [`lower_method_body`].
///
/// # Input contract
///
/// `program` must be a **desugared, feature-projected, typechecked** module, and `type_info` must be the checker
/// output for exactly that projected program. Every caller owes the same preparation the legacy pipeline performs
/// before emission (`CompilationSession::parse_source` in `src/cli/commands/common.rs`: parse, then
/// `vocab_desugar_pass::desugar_program_vocab_blocks`, then feature projection, and only then typecheck), so a
/// vocab-authored body means the same thing through either backend (#1166).
///
/// What must **not** cross this boundary:
///
/// - Raw vocabulary syntax — `ast::Declaration::VocabBlock`, `ast::Statement::VocabBlock`,
///   `ast::Statement::VocabExpressionItem`, `ast::Expr::VocabBlock` — and scoped-DSL surface nodes
///   ([`SurfaceFeatureKey::ScopedDslSurface`], and the `LeadingDotPath`/`ScopedGlyph`/`ScopedSymbolCall` payloads of
///   `ast::Expr::Surface`). The desugar pass owns their meaning; lowering them here would give one source construct two
///   independent definitions.
/// - Declarations gated behind a package feature that is not active. Feature projection is part of the contract, not an
///   optimization: a body behind an inactive feature must not be lowered at all, because lowering it would put a body
///   into Body IR that the compilation does not contain.
///
/// What must cross it: ordinary declarations, statements, and expressions, including the async surface
/// (`await`, `race for`), which is genuine language surface no desugarer removes.
///
/// The contract is enforced rather than assumed. A vocab or scoped-DSL node that still arrives lowers to a
/// `bir::StatementKind::Unsupported` whose description names it as a caller contract violation (see the `refusals`
/// submodule), so a broken caller is a visible diagnostic at the original span instead of a silently missing body.
/// Those refusals are a safety net, never the normal path; the desugar pass keeps ownership of the real diagnostic
/// when a vocabulary's library manifest is unavailable.
///
/// Manifest-free comparison helpers deliberately use [`apply_body_ir_input_contract`]'s empty feature projection. The
/// direct replacement CLI instead receives an already desugared, feature-projected program and its checked lowering
/// bridge from `CompilationSession`; it must not apply a second projection or recreate the typecheck authority here.
pub fn build_body_ir_module_v0(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
) -> bir::BodyIrModule {
    build_body_ir_module_v0_with_provider_operations(program, module_path, type_info, &ProviderOperationCatalog::new())
}

/// Prepare one manifest-free parsed module so it satisfies [`build_body_ir_module_v0`]'s input contract.
///
/// This lives beside the boundary it governs rather than inside any one caller, because every manifest-free caller
/// owes Body IR the same debt. The parity corpus and source-observable comparison route use this helper; a caller that
/// skips it hands lowering a program the legacy path would never have produced, which is the
/// divergence #1166 closes.
///
/// The legacy pipeline owes Body IR a desugared, feature-projected program, and it pays that debt at parse time:
/// `CompilationSession::parse_source` (`src/cli/commands/common.rs`) parses, runs
/// `vocab_desugar_pass::desugar_program_vocab_blocks`, then projects the result through the session's active
/// package features — all before the program is typechecked or lowered. A manifest-free caller has no package
/// feature graph, so it applies the same two steps with the empty feature projection. It must reject an explicit
/// package-feature selection rather than using this helper to approximate one.
///
/// Ordering matters beyond the pair itself. The caller applies this immediately after parsing, ahead of
/// [`replacement_module_profile_error`] and typechecking, because both of those must see the projected program: an
/// import behind an inactive feature is not part of this compilation, and refusing it as an unsupported profile
/// boundary would report a declaration the build does not contain.
///
/// # Errors
///
/// Returns the desugar pass's own diagnostics unchanged. A scoped-DSL surface whose owning library manifest is
/// unavailable is refused there, at that boundary, and this deliberately adds no second diagnostic for it.
pub fn apply_body_ir_input_contract(
    mut program: crate::frontend::ast::Program,
    entrypoint: &std::path::Path,
) -> Result<crate::frontend::ast::Program, Vec<crate::frontend::diagnostics::CompileError>> {
    let entrypoint_display = entrypoint.to_string_lossy();
    crate::frontend::vocab_desugar_pass::desugar_program_vocab_blocks(
        &mut program,
        Some(entrypoint_display.as_ref()),
        &crate::frontend::library_manifest_index::LibraryManifestIndex::default(),
    )?;
    Ok(program.projected_for_features(&std::collections::BTreeSet::new()))
}

/// Build Body IR using the checked provider-operation facts selected for this compilation.
///
/// The ordinary source-only replacement profile has no provider plan and continues to use
/// [`build_body_ir_module_v0`]. Any provider-aware consumer must use this entry point so provider-operation admission
/// is projected from an integrity-checked manifest rather than supplied as a handwritten lowering catalogue.
pub fn build_body_ir_module_v0_with_provider_plan(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
    provider_plan: &ProviderPlan,
) -> Result<bir::BodyIrModule, String> {
    let provider_operations = ProviderOperationCatalog::from_provider_plan(provider_plan)?;
    Ok(build_body_ir_module_v0_with_provider_operations(
        program,
        module_path,
        type_info,
        &provider_operations,
    ))
}

/// Build Body IR v0 for a typechecked module, admitting the internally projected provider operations.
///
/// The catalogue is deliberately private to this frontend bridge. Its entries must come from a selected checked
/// [`ProviderPlan`], never from a backend-specific caller or a source-name convention.
///
/// The input contract on `program` and `type_info` is [`build_body_ir_module_v0`]'s, unchanged: a desugared,
/// feature-projected, typechecked module. The catalogue widens which *calls* lower, never which source surface is
/// admitted, so a caller that skips the desugar pass violates the contract here exactly as it would there.
fn build_body_ir_module_v0_with_provider_operations(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
    provider_operations: &ProviderOperationCatalog,
) -> bir::BodyIrModule {
    let module_identity = body_ir_module_identity(module_path);
    let module_id = CompilerNodeId::module(module_identity.clone());
    let function_default_sources = collect_function_default_sources(program);
    let local_function_declarations = collect_local_function_declarations(program);
    let nominal_declarations = collect_local_nominal_declarations(program, &module_identity, type_info);
    let local_nominal_declarations = nominal_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalNominalDeclarations>();
    let fieldless_enum_declarations = collect_local_fieldless_enum_declarations(program, &module_identity, type_info);
    let local_fieldless_enum_declarations = fieldless_enum_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalFieldlessEnumDeclarations>();
    let value_enum_declarations = collect_local_value_enum_declarations(program, &module_identity, type_info);
    let local_value_enum_declarations = value_enum_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalValueEnumDeclarations>();
    let lowering_facts = BodyIrLoweringFacts {
        type_info,
        function_default_sources: &function_default_sources,
        local_function_declarations: &local_function_declarations,
        local_nominal_declarations: &local_nominal_declarations,
        local_fieldless_enum_declarations: &local_fieldless_enum_declarations,
        local_value_enum_declarations: &local_value_enum_declarations,
        module_identity: &module_identity,
        provider_operations,
    };
    let mut bodies = program
        .declarations
        .iter()
        .flat_map(|decl| -> Vec<bir::Body> {
            match &decl.node {
                ast::Declaration::Function(function) => {
                    vec![lower_function_body(function, decl.span, &lowering_facts)]
                }
                ast::Declaration::Model(model) => lower_owner_method_bodies(
                    &model.methods,
                    &model.name,
                    owner_self_type(&model.name, &model.type_params),
                    &lowering_facts,
                ),
                ast::Declaration::Class(class) => lower_owner_method_bodies(
                    &class.methods,
                    &class.name,
                    owner_self_type(&class.name, &class.type_params),
                    &lowering_facts,
                ),
                ast::Declaration::Trait(trait_decl) => lower_owner_method_bodies(
                    &trait_decl.methods,
                    &trait_decl.name,
                    IncanType::SelfType,
                    &lowering_facts,
                ),
                // A newtype's receiver is the newtype itself, not its underlying type: a method reading the wrapped
                // value goes through the nominal receiver. `rusttype` is a flag on the same declaration rather than a
                // separate kind, so its methods take this arm too: one with a body (a `for Trait` implementation, say)
                // lowers like any other newtype method, and one without contributes nothing through
                // `lower_method_body`'s existing `body: None` check.
                ast::Declaration::Newtype(newtype) => lower_owner_method_bodies(
                    &newtype.methods,
                    &newtype.name,
                    owner_self_type(&newtype.name, &newtype.type_params),
                    &lowering_facts,
                ),
                ast::Declaration::Enum(enum_decl) => lower_owner_method_bodies(
                    &enum_decl.methods,
                    &enum_decl.name,
                    owner_self_type(&enum_decl.name, &enum_decl.type_params),
                    &lowering_facts,
                ),
                _ => Vec::new(),
            }
        })
        .collect::<Vec<_>>();
    apply_top_level_input_contract_refusal(program, &mut bodies);
    bir::BodyIrModule {
        module_id,
        nominal_declarations,
        fieldless_enum_declarations,
        value_enum_declarations,
        bodies,
    }
}

/// Make a broken caller's raw top-level vocabulary declaration fail every executable body rather than disappearing.
///
/// `BodyIrModule` intentionally remains a total lowering result, so it cannot return a module-level error. The
/// visible fail-closed representation is therefore an `Unsupported` marker at the original declaration span in
/// every lowered body. An executor selecting any source body stops before evaluating source statements, while a
/// declaration-only module still has no executable body to select.
fn apply_top_level_input_contract_refusal(program: &ast::Program, bodies: &mut [bir::Body]) {
    let Some((description, span)) = program.declarations.iter().find_map(|declaration| {
        refusals::unsupported_top_level_declaration_label(&declaration.node)
            .map(|description| (description, hir_span(declaration.span)))
    }) else {
        return;
    };

    for body in bodies {
        body.block.stmts.insert(
            0,
            bir::Statement {
                kind: bir::StatementKind::Unsupported {
                    description: description.clone(),
                },
                span,
            },
        );
    }
}

/// Source-declared ordinary default expressions for each top-level function in this module.
///
/// Body-IR lowering needs this small source map only while it lowers a local `partial target(...)` into a forwarding
/// closure: the checked callable signature retains availability but not the executable default expression. The map
/// never leaves this frontend boundary; the resulting [`bir::CallableParamDefault::Source`] stores only Body IR.
type FunctionDefaultSources = HashMap<String, Vec<FunctionDefaultSource>>;

/// Exact spans of this source module's top-level function declarations, grouped by source spelling.
///
/// The typechecker exposes an intentionally wider overload surface that can include imports and aliases. Direct
/// Body-IR dispatch only admits a declaration physically represented by this module, so lowering retains this
/// small source-local map long enough to attach the chosen declaration identity to each named call.
type LocalFunctionDeclarations = HashMap<String, Vec<ast::Span>>;

/// Plain source-local models whose checked declaration layout is retained for direct nominal execution.
///
/// This frontend map intentionally contains only non-generic, behavior-free models. It is used only while lowering
/// a checked constructor call to attach the exact declaration identity and selected field layout. The direct executor
/// compares that target snapshot with the resulting [`bir::NominalDeclaration`] before binding slots. Classes,
/// trait-adopting models, and models carrying methods/properties/aliases are absent rather than being approximated
/// as inert field bags.
type LocalNominalDeclarations = HashMap<String, bir::NominalDeclaration>;

/// Source-local fieldless normal enums whose canonical unit variants are retained for direct comparison.
///
/// This map exists only while lowering. The executor receives `BodyIrModule::fieldless_enum_declarations` and
/// revalidates exact enum/member identities there, so a source spelling never selects an imported or aliased enum.
type LocalFieldlessEnumDeclarations = HashMap<String, bir::FieldlessEnumDeclaration>;

/// Source-local RFC 032 value enums whose canonical scalar members are retained for direct execution.
///
/// This map is lowering-only. The executor receives `BodyIrModule::value_enum_declarations` and verifies retained
/// enum/member identities there, so imports, aliases, ordinary enums, and non-retained same-spelling forms never
/// become direct runtime targets.
type LocalValueEnumDeclarations = HashMap<String, bir::ValueEnumDeclaration>;

/// Borrowed module facts shared by every body lowerer.
///
/// These facts are collected once from checked source and remain frontend-only: emitted Body IR carries only the
/// identities and representations a later direct executor needs. Keeping the bundle explicit avoids widening any
/// individual lowering helper's parameter surface as profiles add one bounded source-local fact at a time.
struct BodyIrLoweringFacts<'type_info, 'source> {
    type_info: &'type_info TypeCheckInfo,
    function_default_sources: &'source FunctionDefaultSources,
    local_function_declarations: &'source LocalFunctionDeclarations,
    local_nominal_declarations: &'source LocalNominalDeclarations,
    local_fieldless_enum_declarations: &'source LocalFieldlessEnumDeclarations,
    local_value_enum_declarations: &'source LocalValueEnumDeclarations,
    module_identity: &'source str,
    /// Provider operations this compilation admits, keyed by canonical identity rather than by any spelling.
    provider_operations: &'source ProviderOperationCatalog,
}

/// Source facts a synthesized local partial needs for one target parameter.
#[derive(Clone)]
struct FunctionDefaultSource {
    /// The target parameter's original span.
    param_span: ast::Span,
    /// The target's ordinary source default, if it declared one.
    default: Option<ast::Spanned<ast::Expr>>,
}

/// Determine whether a model can carry the small direct-replacement declaration fact.
///
/// This is deliberately a source-local data-model shape, not a general nominal-semantics predicate. The replacement
/// runtime cannot execute model decorators, trait behavior, methods, field aliases, or generic substitution without
/// facts that Body IR does not retain. Field defaults remain represented by each construction's checked binding, so a
/// fully supplied construction may execute while any omitted default still refuses at that constructor's span.
pub(crate) fn is_direct_replacement_plain_model(model: &ast::ModelDecl) -> bool {
    model.decorators.is_empty()
        && model.type_params.is_empty()
        && model.traits.is_empty()
        && model.method_aliases.is_empty()
        && model.method_partials.is_empty()
        && model.properties.is_empty()
        && model.methods.is_empty()
        && model.fields.iter().all(|field| field.node.metadata.alias.is_none())
}

/// Determine whether an enum carries the narrow source-local fieldless normal-enum declaration fact.
///
/// This excludes every declaration form whose behavior needs additional semantic representation: scalar value enums,
/// payload construction, aliases, trait dispatch, custom methods, decorators, and generic substitution. The direct
/// runtime can therefore materialize only a canonical unit carrier and compare its retained identity.
pub(crate) fn is_direct_replacement_fieldless_enum(enum_decl: &ast::EnumDecl) -> bool {
    enum_decl.decorators.is_empty()
        && enum_decl.type_params.is_empty()
        && enum_decl.value_type.is_none()
        && enum_decl.traits.is_empty()
        && enum_decl.variant_aliases.is_empty()
        && enum_decl.methods.is_empty()
        && enum_decl
            .variants
            .iter()
            .all(|variant| variant.node.fields.is_empty() && variant.node.value.is_none())
}

/// Determine whether an enum carries the narrow source-local RFC 032 scalar declaration fact.
///
/// This predicate intentionally excludes aliases and all behavior-bearing forms even when they are source-valid:
/// the direct executor may validate only a canonical literal member and the compiler-provided `.value()` extraction,
/// not trait dispatch, custom methods, alias canonicalization, generic substitution, or payload construction.
pub(crate) fn is_direct_replacement_value_enum(enum_decl: &ast::EnumDecl) -> bool {
    enum_decl.decorators.is_empty()
        && enum_decl.type_params.is_empty()
        && enum_decl.value_type.is_some()
        && enum_decl.traits.is_empty()
        && enum_decl.variant_aliases.is_empty()
        && enum_decl.methods.is_empty()
        && enum_decl.variants.iter().all(|variant| {
            variant.node.fields.is_empty()
                && matches!(
                    variant.node.value.as_ref().map(|value| &value.node),
                    Some(ast::ValueEnumLiteral::Int(_) | ast::ValueEnumLiteral::Str(_))
                )
        })
}

/// Render a module path into the same module identity spelling [`crate::frontend::hir`] uses, so declaration ids
/// line up between the two representations.
fn body_ir_module_identity(module_path: &[String]) -> String {
    incan_semantics_core::module_identity_for_path(module_path)
}

/// Convert an AST byte-offset span into a Body IR source span.
const fn hir_span(span: ast::Span) -> HirSourceSpan {
    HirSourceSpan::new(span.start, span.end)
}

/// Per-function lowering state: fresh local/scope allocation, current name bindings, and accumulated body-level
/// facts (runtime requirements, panic facts, which locals have been moved out of their declaring scope).
struct BodyBuilder<'type_info, 'source> {
    type_info: &'type_info TypeCheckInfo,
    /// Source defaults for top-level partial targets, retained only until they lower into Body IR.
    function_default_sources: &'source FunctionDefaultSources,
    /// Exact declarations physically present in this module, used only to retain same-module call identities.
    local_function_declarations: &'source LocalFunctionDeclarations,
    /// Source-local plain-model declarations, used only to retain an exact constructor target identity.
    local_nominal_declarations: &'source LocalNominalDeclarations,
    /// Source-local fieldless normal-enum declarations, used only to retain exact unit-member target identities.
    local_fieldless_enum_declarations: &'source LocalFieldlessEnumDeclarations,
    /// Source-local RFC 032 value-enum declarations, used only to retain an exact member target identity.
    local_value_enum_declarations: &'source LocalValueEnumDeclarations,
    /// Owning module identity used to construct a source-span declaration identity without consulting a backend.
    module_identity: &'source str,
    /// Provider operations this compilation admits, consulted only by canonical identity (see `provider_ops`).
    provider_operations: &'source ProviderOperationCatalog,
    /// Checked return type of the function/method currently being lowered, used only to retain `?` error routing.
    owner_return_type: IncanType,
    locals: Vec<bir::LocalDecl>,
    scopes: Vec<bir::ScopeInfo>,
    /// Current source-name -> local binding.
    ///
    /// A plain inferred assignment reuses an active binding; `let` and `mut` replace this map entry with an explicit
    /// shadow. Nested lowering paths snapshot and restore the map at their lexical boundary, so branch/loop/arm
    /// names remain available to their Body-IR statements without leaking into following source.
    bindings: HashMap<String, bir::LocalId>,
    /// Canonical source binding -> frame local. This is the semantic lookup used for every resolver-proven read or
    /// write; [`Self::bindings`] remains only as a lexical bookkeeping projection for lowering constructs that
    /// introduce and restore source names.
    identity_bindings: HashMap<CanonicalSymbolId, bir::LocalId>,
    /// Names lowering could not resolve to a tracked local (e.g. module-level `const`/`static`), reused across
    /// repeated reads instead of allocating a fresh external local per read.
    external_locals: HashMap<String, bir::LocalId>,
    /// Remaining textual reads for each tracked (non-temporary) local, seeded at declaration time by counting
    /// `Ident` occurrences of its name in the declaring scope's statement suffix (see [`count_reads_in_stmts`]).
    /// Decremented on every read; a decrement that reaches zero selects [`bir::OwnershipFact::Move`].
    remaining_reads: HashMap<bir::LocalId, usize>,
    /// Locals whose value has been moved out via a full-value (non-projected) read, so scope-exit drop insertion
    /// skips them.
    moved_out: HashSet<bir::LocalId>,
    /// Locals whose current value was built by `lower_range_value` or copied from another such local. A checked
    /// `Range[T]` spelling alone is not a layout contract: parameters, call results, imports, and user
    /// declarations can use it without the four-field `AggregateKind::Range` representation. Only this
    /// source-local provenance permits a later `for` loop to project range fields.
    materialized_range_locals: HashSet<bir::LocalId>,
    /// Stack of the innermost-to-outermost enclosing loop's `break`-value target, pushed/popped by every loop-
    /// lowering path (`while`, `for`, and value-producing `loop` expressions) around its own body. `Some(local)`
    /// means the innermost loop is a value-producing `loop:` expression (see [`Self::lower_loop_expr`]) whose
    /// `break value` statements should assign into `local` instead of carrying the value on the `Break` statement
    /// itself; `None` means the innermost loop does not produce a value (`while`/`for`, which never legally see a
    /// `break value` today, or a `loop:` expression's own synthetic exit checks). Always non-empty while lowering
    /// any loop body, so [`Self::lower_break`] can look up the innermost target with `.last()`.
    loop_break_targets: Vec<Option<bir::LocalId>>,
    runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    panic_facts: Vec<bir::PanicFact>,
    next_local: u32,
    next_scope: u32,
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Start a fresh builder for one function body, with no locals, scopes, or accumulated facts yet.
    fn new(lowering_facts: &BodyIrLoweringFacts<'type_info, 'source>, owner_return_type: IncanType) -> Self {
        Self {
            type_info: lowering_facts.type_info,
            function_default_sources: lowering_facts.function_default_sources,
            local_function_declarations: lowering_facts.local_function_declarations,
            local_nominal_declarations: lowering_facts.local_nominal_declarations,
            local_fieldless_enum_declarations: lowering_facts.local_fieldless_enum_declarations,
            local_value_enum_declarations: lowering_facts.local_value_enum_declarations,
            module_identity: lowering_facts.module_identity,
            provider_operations: lowering_facts.provider_operations,
            owner_return_type,
            locals: Vec::new(),
            scopes: Vec::new(),
            bindings: HashMap::new(),
            identity_bindings: HashMap::new(),
            external_locals: HashMap::new(),
            remaining_reads: HashMap::new(),
            moved_out: HashSet::new(),
            materialized_range_locals: HashSet::new(),
            loop_break_targets: Vec::new(),
            runtime_requirements: Vec::new(),
            panic_facts: Vec::new(),
            next_local: 0,
            next_scope: 0,
        }
    }

    // ---- Scopes and locals ----

    /// Allocate a fresh lexical scope with the given `parent`, recording it in `scopes` for later span lookup.
    fn new_scope(&mut self, parent: Option<bir::ScopeId>, span: HirSourceSpan) -> bir::ScopeId {
        let id = bir::ScopeId(self.next_scope);
        self.next_scope += 1;
        self.scopes.push(bir::ScopeInfo { id, parent, span });
        id
    }

    /// Look up the source span recorded for `scope`, or a zero-width span if the id is unknown (defensive default;
    /// every scope this builder hands out is always recorded in `scopes` first).
    fn scope_span(&self, scope: bir::ScopeId) -> HirSourceSpan {
        self.scopes
            .iter()
            .find(|info| info.id == scope)
            .map(|info| info.span)
            .unwrap_or(HirSourceSpan::new(0, 0))
    }

    /// Resolve the expression type recorded by the typechecker for `span`, or [`IncanType::Unknown`] when v0 has no
    /// resolved type available (an explicit unknown rather than a guessed default).
    fn resolve_ty(&self, span: ast::Span) -> IncanType {
        self.type_info
            .expr_type(span)
            .map(semantic_type_from_resolved)
            .unwrap_or(IncanType::Unknown)
    }

    /// Declare a new user-facing local (parameter or source binding), seeding its last-use countdown from the
    /// number of `Ident` reads of `name` found in `remaining` (the declaring block's statement suffix, or a loop
    /// body for per-iteration bindings). Defaults to [`bir::LocalOrigin::UserBinding`]; callers that declare a
    /// parameter overwrite the origin afterward.
    fn declare_new_local(
        &mut self,
        name: String,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        remaining: &[ast::Spanned<ast::Statement>],
    ) -> bir::LocalId {
        let total_reads = count_reads_in_stmts(&name, remaining);
        self.declare_new_local_with_reads(name, ty, scope, span, total_reads)
    }

    /// Declare a new user-facing local with an already-computed last-use countdown, for declaration sites whose
    /// "remaining reads" context is not a plain statement suffix -- currently only comprehension/generator `for`
    /// clause bindings (see `Self::lower_comprehension_clauses`), whose remaining context is a tail of
    /// [`ast::ComprehensionClause`]s plus a terminal element/key/value expression, not
    /// [`ast::Statement`]s. [`Self::declare_new_local`] is a thin wrapper over this that seeds `total_reads` from a
    /// statement suffix via [`count_reads_in_stmts`].
    fn declare_new_local_with_reads(
        &mut self,
        name: String,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        total_reads: usize,
    ) -> bir::LocalId {
        let source_span = ast::Span::new(span.start, span.end);
        let identity = self.type_info.resolved_write_identity(source_span, &name).cloned();
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: Some(name.clone()),
            identity: identity.clone(),
            ty,
            origin: bir::LocalOrigin::UserBinding,
            scope,
            span,
        });
        self.bindings.insert(name, id);
        if let Some(identity) = identity {
            self.identity_bindings.insert(identity, id);
        }
        self.remaining_reads.insert(id, total_reads);
        id
    }

    /// Declare a method's `self`/`mut self` receiver as a [`bir::LocalOrigin::Receiver`] local, bound under the
    /// name `"self"` in [`Self::bindings`] exactly like an ordinary local so [`Self::place_for_name`] resolves
    /// `self` reads without a separate lookup path.
    ///
    /// Unlike [`Self::declare_new_local`], no last-use countdown is seeded: a receiver is always a Rust-level
    /// reference (`&self`/`&mut self`), so nothing about it can be "used up" the way an owned local's remaining
    /// reads can — see the receiver carve-out in [`Self::ownership_fact_for_place`], which decides the ownership
    /// fact for every `self` read before that countdown would ever be consulted.
    fn declare_receiver_local(
        &mut self,
        ty: IncanType,
        mutable: bool,
        scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> bir::LocalId {
        let source_span = ast::Span::new(span.start, span.end);
        let identity = self.type_info.resolved_write_identity(source_span, "self").cloned();
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: Some("self".to_string()),
            identity: identity.clone(),
            ty,
            origin: bir::LocalOrigin::Receiver { mutable },
            scope,
            span,
        });
        self.bindings.insert("self".to_string(), id);
        if let Some(identity) = identity {
            self.identity_bindings.insert(identity, id);
        }
        id
    }

    /// Allocate a compiler-introduced temporary. Temporaries are always consumed exactly once, immediately after
    /// creation (by construction of the flattening lowering below), so they are excluded from last-use tracking and
    /// scope-exit drop insertion — see [`Self::temp_operand`] and [`Self::insert_scope_drops`].
    fn new_temp(&mut self, ty: IncanType, scope: bir::ScopeId, span: HirSourceSpan) -> bir::LocalId {
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: None,
            identity: None,
            ty,
            origin: bir::LocalOrigin::Temporary,
            scope,
            span,
        });
        id
    }

    /// Resolve one source reference from the canonical identity recorded by typechecking.
    ///
    /// A proven local identity must select a frame local with that same identity. Proven `const`/`static` references
    /// become canonical global places. Any other proven identity that has no Body IR value representation returns
    /// `None`, so the caller emits an explicit unsupported node instead of silently changing meaning through a
    /// spelling lookup. Only a genuinely unproven reference may use the legacy `External` recovery local.
    fn place_for_name(&mut self, name: &str, span: ast::Span, ty: &IncanType) -> Option<bir::Place> {
        if let Some(identity) = self.type_info.resolved_identity(span).cloned() {
            if let Some(&id) = self.identity_bindings.get(&identity) {
                return Some(bir::Place::from_local(id));
            }
            return self.global_place(identity, ty.clone()).map(bir::Place::from_global);
        }

        Some(bir::Place::from_local(
            self.external_local_for_name(name, hir_span(span)),
        ))
    }

    /// Select a canonical module-storage root when `identity` denotes a `const` or `static`.
    fn global_place(&self, identity: CanonicalSymbolId, ty: IncanType) -> Option<bir::GlobalPlace> {
        let write_policy = match identity.kind {
            SemanticSourceTargetKind::Const => bir::GlobalWritePolicy::ReadOnly,
            SemanticSourceTargetKind::Static => {
                let declared_here = identity
                    .module_path()
                    .is_some_and(|path| body_ir_module_identity(path) == self.module_identity);
                if declared_here {
                    bir::GlobalWritePolicy::Rebindable
                } else {
                    bir::GlobalWritePolicy::ProjectionOnly
                }
            }
            _ => return None,
        };
        Some(bir::GlobalPlace {
            identity,
            ty,
            write_policy,
        })
    }

    /// Allocate the explicit recovery local for a reference whose resolver supplied no identity.
    fn external_local_for_name(&mut self, name: &str, span: HirSourceSpan) -> bir::LocalId {
        if let Some(&id) = self.external_locals.get(name) {
            return id;
        }
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: Some(name.to_string()),
            identity: None,
            ty: IncanType::Unknown,
            origin: bir::LocalOrigin::External,
            scope: bir::ScopeId(0),
            span,
        });
        self.external_locals.insert(name.to_string(), id);
        id
    }

    // ---- Ownership facts ----

    /// Select the Duckborrower fact and last-use marker for reading `place`.
    ///
    /// Projected reads (`.field`, `[index]`) never move: v0 does not track partial-move state, so a non-Copy
    /// projected read always borrows rather than risking an unsound move out of a place the surrounding code still
    /// owns. A bare read of a [`bir::LocalOrigin::Receiver`] local (`self`/`mut self`) never moves either, for a
    /// stronger reason than the projected case: a receiver is always a Rust-level reference at the emission
    /// boundary, so moving a non-Copy value out of it would not even compile — the only sound way to produce an
    /// owned value from it is to clone (mirrors the existing backend ownership planner's treatment of non-Copy
    /// `self` reads in `src/backend/ir/ownership.rs`, which this module's own docs cite as precedent). Every other
    /// bare local read decrements its remaining-reads countdown; reaching zero selects `Move` (and records the
    /// local as moved for [`Self::insert_scope_drops`]), otherwise `Clone`. A local with no tracked countdown (an
    /// [`bir::LocalOrigin::External`] reference) gets the explicit [`bir::OwnershipFact::Unknown`].
    ///
    /// Note that [`count_reads_in_stmts`] counts a `.field`/`[index]` occurrence of a name toward that local's
    /// total the same as a bare occurrence, but only bare reads ever decrement the countdown here. A local read
    /// only through projections therefore never reaches zero and always reads `Clone` on its final bare use rather
    /// than `Move` — an over-seeded, never-decremented countdown biases toward `Clone`, not toward an unsound
    /// `Move`, consistent with this module's documented last-use approximation.
    fn ownership_fact_for_place(&mut self, place: &bir::Place, ty: &IncanType) -> (bir::OwnershipFact, bool) {
        let is_copy = ty.abi_v0_facts().ownership.is_trivially_copy();
        if !place.projection.is_empty() {
            let fact = if is_copy {
                bir::OwnershipFact::Copy
            } else {
                bir::OwnershipFact::Borrow
            };
            return (fact, false);
        }
        let Some(local) = place.local_id() else {
            return (
                if is_copy {
                    bir::OwnershipFact::Copy
                } else {
                    bir::OwnershipFact::Clone
                },
                false,
            );
        };
        if self.is_receiver_local(local) {
            let fact = if is_copy {
                bir::OwnershipFact::Copy
            } else {
                bir::OwnershipFact::Clone
            };
            return (fact, false);
        }
        if is_copy {
            if let Some(remaining) = self.remaining_reads.get_mut(&local) {
                *remaining = remaining.saturating_sub(1);
            }
            return (bir::OwnershipFact::Copy, false);
        }
        let Some(remaining) = self.remaining_reads.get_mut(&local) else {
            return (bir::OwnershipFact::Unknown, false);
        };
        *remaining = remaining.saturating_sub(1);
        if *remaining == 0 {
            self.moved_out.insert(local);
            (bir::OwnershipFact::Move, true)
        } else {
            (bir::OwnershipFact::Clone, false)
        }
    }

    /// Whether `local` is a method's `self`/`mut self` receiver, per its recorded [`bir::LocalOrigin`].
    fn is_receiver_local(&self, local: bir::LocalId) -> bool {
        self.locals
            .get(local.index())
            .is_some_and(|decl| matches!(decl.origin, bir::LocalOrigin::Receiver { .. }))
    }

    /// Build the operand for a freshly created temporary's single, immediate use.
    fn temp_operand(&self, local: bir::LocalId, ty: &IncanType) -> bir::Operand {
        let fact = if ty.abi_v0_facts().ownership.is_trivially_copy() {
            bir::OwnershipFact::Copy
        } else {
            bir::OwnershipFact::Move
        };
        bir::Operand::place(bir::Place::from_local(local), fact, true)
    }

    /// Record a runtime/helper requirement for this body, deduplicated and kept in first-seen order (see
    /// [`bir::Body::runtime_requirements`] for why lowering relies on traversal order rather than sorting).
    fn record_runtime_requirement(&mut self, requirement: AbiV0RuntimeRequirement) {
        if !self.runtime_requirements.contains(&requirement) {
            self.runtime_requirements.push(requirement);
        }
    }

    /// Emit explicit `Drop` statements, in reverse declaration order, for every non-Copy `UserBinding`/`Parameter`
    /// local declared directly in `scope` that was never moved out. This is scoped to locals declared *directly* in
    /// this block — it does not attempt cross-branch or early-return/break drop-obligation dataflow, which needs
    /// full control-flow analysis out of scope for v0 (see [`incan_semantics_core::body_ir`] module docs).
    fn insert_scope_drops(&mut self, stmts: &mut Vec<bir::Statement>, scope: bir::ScopeId) {
        let span = self.scope_span(scope);
        let candidates: Vec<bir::LocalId> = self
            .locals
            .iter()
            .rev()
            .filter(|local| local.scope == scope)
            .filter(|local| {
                matches!(
                    local.origin,
                    bir::LocalOrigin::UserBinding | bir::LocalOrigin::Parameter
                )
            })
            .filter(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
            .map(|local| local.id)
            .collect();
        for id in candidates {
            if self.moved_out.contains(&id) {
                continue;
            }
            stmts.push(bir::Statement {
                kind: bir::StatementKind::Drop { local: id },
                span,
            });
        }
    }

    /// Push a [`bir::StatementKind::Unsupported`] statement carrying a short diagnostic `description`, so an
    /// unmodeled source construct still leaves a total, structurally valid statement rather than being dropped.
    fn push_unsupported_stmt(&self, description: String, span: HirSourceSpan, out: &mut Vec<bir::Statement>) {
        out.push(bir::Statement {
            kind: bir::StatementKind::Unsupported { description },
            span,
        });
    }

    /// Emit an `Unsupported` marker statement and return a handle operand for it, so callers evaluating an
    /// unsupported expression in value position still get a structurally valid [`bir::Operand`] to thread onward.
    fn unsupported_operand(
        &mut self,
        description: String,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let temp = self.new_temp(IncanType::Unknown, scope, span);
        self.push_unsupported_stmt(description, span, out);
        bir::Operand::place(bir::Place::from_local(temp), bir::OwnershipFact::Unknown, true)
    }

    // ---- Rvalue / call helpers ----

    /// Allocate a fresh temporary, push an `Assign` statement giving it `rvalue`'s value, and return an operand for
    /// that temporary's single, immediate use (see [`Self::temp_operand`]). The common tail shared by every
    /// expression-lowering path that needs to flatten a computed value into a place before it can be read again.
    fn push_assign_temp(
        &mut self,
        rvalue: bir::Rvalue,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let temp = self.new_temp(ty.clone(), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(temp),
                rvalue,
            },
            span,
        });
        self.temp_operand(temp, &ty)
    }

    /// Allocate a fresh temporary, push a `Call` statement storing its result there, and return an operand for that
    /// temporary's single, immediate use — the call-lowering counterpart to [`Self::push_assign_temp`].
    #[allow(clippy::too_many_arguments)]
    fn push_call_temp(
        &mut self,
        callee: bir::Callee,
        args: Vec<bir::ArgumentElement>,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        may_panic: bool,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let temp = self.new_temp(ty.clone(), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Call {
                destination: Some(bir::Place::from_local(temp)),
                callee,
                args,
                may_panic,
            },
            span,
        });
        self.temp_operand(temp, &ty)
    }

    /// Build the boolean negation of `operand` as a fresh temporary (`not operand`), used to turn a loop's
    /// continuation condition into its complementary exit condition for the leading conditional `Break`.
    fn negate_operand(
        &mut self,
        operand: bir::Operand,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        self.push_assign_temp(
            bir::Rvalue::UnaryOp(bir::UnOp::Not, operand),
            IncanType::Primitive(IncanPrimitiveType::Bool),
            scope,
            span,
            out,
        )
    }

    // ---- Statements ----
}

// ============================================================================
// Comprehension desugaring helpers
// ============================================================================

// ============================================================================
// Free helper functions
// ============================================================================

/// Map a surface unary operator to Body IR's unary operator set. Exhaustive: all three surface unary operators have
/// a direct Body IR equivalent.
const fn lower_unary_op(op: ast::UnaryOp) -> bir::UnOp {
    match op {
        ast::UnaryOp::Neg => bir::UnOp::Neg,
        ast::UnaryOp::Not => bir::UnOp::Not,
        ast::UnaryOp::Invert => bir::UnOp::Invert,
    }
}

/// Register the callable-value contracts and private mechanisms owned by Body IR lowering.
///
/// This is deliberately adjacent to [`BodyBuilder::lower_closure`] and [`BodyBuilder::lower_partial`], rather than
/// a row in the compatibility collector. The replacement executor still refuses local callable targets; that fact
/// stays explicit in the collected evidence and does not make either feature execution-complete.
pub(crate) fn replacement_compatibility_body_ir_contribution()
-> crate::replacement_compatibility::ReplacementCompatibilityContribution {
    use crate::replacement_compatibility::{
        feature_requirement_link, implementation_requirement, local_implementation_contribution,
        planned_feature_at_boundary,
    };

    local_implementation_contribution(
        "frontend.body-ir.callable-values",
        "src/frontend/body_ir.rs",
        "fn replacement_compatibility_body_ir_contribution",
        vec![
            planned_feature_at_boundary(
                "call.partial-binding",
                "Partial presets capture at construction, remain overrideable defaults, and preserve named/positional binding rules.",
                988,
                "Body IR and closed #1152 carry the source and callable-runtime substrate; open #988 owns the direct local callable forms that remain visibly refused.",
                "src/frontend/typechecker/check_expr/calls.rs",
                "fn check_call",
                "fn lower_call",
                "fn execute_call",
            ),
            planned_feature_at_boundary(
                "call.stored-callables",
                "Stored closures and partials retain lexical capture timing, ownership, and isolated local call frames.",
                988,
                "Closed #1152 delivered the coherent callable-frame substrate; open #988 owns broadening the local callable targets that direct execution still refuses.",
                "src/frontend/typechecker/check_expr/calls.rs",
                "fn check_call",
                "fn lower_call",
                "fn execute_call",
            ),
        ],
        vec![
            implementation_requirement(
                "call.argument-binder",
                "Parameter binding preserves positional, named, default, preset, variadic, and diagnostic rules.",
                "typechecker partial projection and replacement call runtime",
                "partial/default typechecker and Body-IR tests",
                "Binding slots are shared call machinery, not a user feature.",
            ),
            implementation_requirement(
                "captures.lexical-environments",
                "Closure and partial capture reads occur at construction time with explicit ownership.",
                "Body IR closure lowering and replacement runtime",
                "closure/partial capture timing regressions",
                "Lexical environments are private runtime state.",
            ),
        ],
        Vec::new(),
        vec![
            feature_requirement_link("call.partial-binding", "call.argument-binder"),
            feature_requirement_link("call.partial-binding", "captures.lexical-environments"),
            feature_requirement_link("call.stored-callables", "call.frames"),
            feature_requirement_link("call.stored-callables", "captures.lexical-environments"),
        ],
    )
}

mod match_;

mod closures;

mod async_;

mod literals;

mod calls;

mod operators;

mod expr;

mod defaults;

mod assertions;

mod comprehensions;

mod control_flow;

mod stmt;

mod free_vars;
mod provider_ops;
mod refusals;

mod reads;

mod collect;

mod bodies;

mod primitives;

mod args;

use bodies::*;
use collect::*;
use provider_ops::{ProviderOperationCatalog, ProviderOperationRecord};
use reads::*;

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod tuple_destructure_interop_tests {
    use super::IncanType;
    use super::refusals::unsupported_tuple_destructure;

    /// Lowering must apply the same accepted-shape rule as the typechecker to interop values (#1132).
    ///
    /// A blanket `RustInteropPath` exemption here would leave the original defect reachable through interop: an
    /// opaque Rust value would lower to a `.0`/`.1` projection and fail as raw `rustc` output.
    #[test]
    fn opaque_rust_interop_values_refuse_to_lower_a_tuple_destructure() {
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("String".to_string()), 2).is_some(),
            "an opaque Rust value must not lower to a tuple field projection"
        );
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("std::vec::Vec<u8>".to_string()), 2).is_some(),
            "a Rust generic that is not a tuple must not lower to a tuple field projection"
        );
        // `(String)` is a parenthesised `String`, not a one-element tuple, so a single-name destructure must not
        // lower to `.0` against it.
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("(String)".to_string()), 1).is_some(),
            "a parenthesised Rust type has no `.0` field and must refuse to lower"
        );
        // The genuine one-element spelling still lowers.
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("(String,)".to_string()), 1).is_none(),
            "`(String,)` is a real one-element tuple and must keep lowering"
        );
    }

    /// The readable tuple spelling the stdlib relies on must still lower, so the refusal stays narrow.
    #[test]
    fn readable_rust_tuple_values_still_lower_a_tuple_destructure() {
        assert!(
            unsupported_tuple_destructure(
                &IncanType::RustInteropPath("(String,incan_stdlib::json::JsonValue)".to_string()),
                2
            )
            .is_none(),
            "`std.json` destructures a `rust::HashMap` item and must keep lowering"
        );
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("(String,JsonValue)".to_string()), 3).is_some(),
            "a Rust tuple of the wrong arity must still be refused"
        );
    }
}
