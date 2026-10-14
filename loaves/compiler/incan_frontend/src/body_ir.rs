//! Frontend bridge from typechecked AST function bodies into Body IR v0.
//!
//! Declaration-level HIR ([`crate::hir`]) does not model statements or expressions at all (see its module docs), so
//! Body IR v0 lowers directly from `ast::FunctionDecl` bodies plus [`TypeCheckInfo`], rather than from a hypothetical
//! body-shaped HIR that does not exist yet. Every [`Body`](incan_semantics_core::body_ir::Body) this module produces
//! carries a [`CompilerNodeId`] identical to the one [`crate::hir::build_hir_v0`] would assign the same function's
//! [`crate::hir`] declaration, so the two can be correlated by id without threading a [`crate::hir`] value through this
//! API.
//!
//! Body IR v0 lowers a representative, explicitly documented subset of the language surface (see
//! [`incan_semantics_core::body_ir`] module docs for the full rationale). Statements fully lowered: assignment
//! (inferred/let/mutable/reassignment), field/index assignment (including their pre-desugared compound `<op>=` forms),
//! compound assignment (`x <op>= y`), tuple unpacking, multi-target (lvalue) tuple assignment, chained assignment,
//! `return`, `if`/`elif`/`else`, `loop:`, `while`, `for` (both a `start..end` range and a general iterable -- builtin
//! collections or a resolved `__iter__`/`__next__` protocol, including the fallible `for item in iterable?:` form),
//! expression statements, statement-position `yield value` (see [`BodyBuilder::lower_stmt_into`] and
//! [`bir::Body::is_generator`]), all three RFC 018 `assert` forms -- plain condition, `assert value is P` (whose
//! bindings stay live for the rest of the enclosing block), and `assert call() raises E` (see
//! [`BodyBuilder::lower_assert`]) -- `pass`, `break` (including a value-producing `break` inside a `loop` expression),
//! `continue`. Expressions fully lowered: identifiers, literals (int/float/decimal/bool/string/bytes),
//! arithmetic/comparison/boolean binary operators and all three unary operators, calls and method calls (including
//! named, out-of-order, defaulted, and explicitly generic argument spellings -- see [`BodyBuilder::lower_call`]), field
//! access, indexing, slicing, parenthesization, range values, tuples, list/dict/set literals (list and dict spread
//! entries included; set literals have no spread spelling), `model`/`class` construction (named-only at the source
//! level, bound to declared field order -- see [`BodyBuilder::lower_nominal_construction`]), expression-position
//! `if`/`loop`, `try` (`?`), f-strings, list/dict comprehensions, lazy generator expressions, closure literals, partial
//! callables (see [`BodyBuilder::lower_closure`]/[`BodyBuilder::lower_partial`] for how captures are computed and
//! represented explicitly rather than left implicit), and `match` (see [`BodyBuilder::lower_match`] for how patterns
//! are lowered and their bindings scoped).
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
//! Source `unsafe:` regions retain an acknowledgment receipt, their source span, and the exact operations they
//! cover. They introduce no Incan scope. Consumers must validate and deliberately admit the acknowledgment before
//! lowering its operations; it neither grants provider capabilities nor bypasses another unsupported construct.
//! See [`BodyBuilder::lower_unsafe_region`].
//!
//! One coverage limit is silent rather than marked, and it is deliberate. Expression-position `yield` (the two-way
//! send/receive protocol) is a stub in the existing Rust-emission backend too, so there is no behavior to preserve;
//! the typechecker rejects a bare `yield` with no value before lowering runs. Model, class, trait-default, newtype,
//! and enum method bodies all lower through [`lower_owner_method_bodies`].

use std::collections::{HashMap, HashSet};

use incan_lang::lang::keywords::KeywordId;
use incan_semantics_core::SurfaceFeatureKey;
use incan_semantics_core::body_ir as bir;
use incan_semantics_core::{
    AbiV0RuntimeRequirement, CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanCallableParam,
    IncanCallableParamKind, IncanPrimitiveType, IncanType, SemanticSourceTargetKind, rust_tuple_arity,
};

use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};

use crate::ast;
use crate::provider::ProviderPlan;
use crate::symbols::{CallableParam, ResolvedType};
use crate::typechecker::{
    FixedUnpackPlan, IdentKind, ResolvedOperatorKind, TypeCheckInfo, ValidatedNewtypeCoercionInfo,
    semantic_type_from_resolved,
};

/// Build Body IR v0 for every top-level function declaration and every non-abstract class/model/trait method in a
/// typechecked module.
///
/// `ast::Declaration::Function` items each produce one [`bir::Body`], matching the [`CompilerNodeId`]
/// [`crate::hir::build_hir_v0`] assigns the corresponding declaration (see that function's docs).
/// `ast::Declaration::Model`/`Class`/`Trait` items additionally contribute one [`bir::Body`] per non-abstract method
/// (#1102) — abstract methods (`body: None`, trait requirements with no implementation) contribute nothing, since there
/// is no body to lower. Method [`CompilerNodeId`]s are *not* assigned by [`crate::hir::build_hir_v0`] today
/// (declaration-level HIR only assigns ids to top-level declarations), so this function constructs its own method ids
/// by scoping the method name under its owning declaration's name — see [`lower_method_body`].
///
/// # Input contract
///
/// `program` must be a **desugared, feature-projected, typechecked** module, and `type_info` must be the checker
/// output for exactly that projected program. Every caller owes the same preparation the legacy pipeline performs
/// before emission (`CompilationSession::parse_source` in `loaves/compiler/incan_driver/src/session.rs`: parse, then
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
/// What must cross it: ordinary declarations, statements, and expressions, including the async surface (`await`, `race
/// for`), which is genuine language surface no desugarer removes.
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
    build_body_ir_module_v0_with_provider_operations(
        program,
        module_path,
        type_info,
        &ProviderOperationCatalog::new(),
        &[],
    )
}

/// Lower local checked source with canonical type contexts decoded from its selected package artifacts.
///
/// Context only supplies the declaring compilation's type layout. References and argument bindings still come from
/// the consumer's existing typecheck facts, and imported declarations are matched by canonical identity.
pub fn build_body_ir_module_v0_with_executable_context(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
    context: &[bir::BodyIrModule],
) -> bir::BodyIrModule {
    build_body_ir_module_v0_with_provider_operations(
        program,
        module_path,
        type_info,
        &ProviderOperationCatalog::new(),
        context,
    )
}

/// Checked layouts of the stdlib models and classes one module names, each owned by a compiled SDK package.
///
/// A consumer that links the SDK crates carries these as package layouts beside the module's own Body IR: every
/// field keeps its declared identity on the owner's package origin, so the records validate like published package
/// layouts, while the crate that compiled them keeps their storage, derives and methods.
/// Concrete SDK constructors returned by selected trait defaults count even when legacy exposes only their trait
/// interface; their source-verified layouts grant no permission to execute a missing default body.
pub fn library_nominal_declarations(type_info: &TypeCheckInfo) -> Vec<bir::NominalDeclaration> {
    let mut layouts = library_nominals::collect(type_info);
    for layout in library_opaque_returns::layouts(type_info) {
        if !layouts.iter().any(|existing| existing.canonical == layout.canonical) {
            layouts.push(layout);
        }
    }
    for layout in &mut layouts {
        for (selected, implications) in incan_lang::lang::derives::DERIVE_IMPLICATIONS {
            if layout
                .derives
                .iter()
                .any(|name| name == incan_lang::lang::derives::as_str(*selected))
            {
                for implied in *implications {
                    let name = incan_lang::lang::derives::as_str(*implied).to_owned();
                    if !layout.derives.contains(&name) {
                        layout.derives.push(name);
                    }
                }
            }
        }
    }
    layouts
}

/// SDK enum contracts travel in declaration-only modules under their native owner's physical identity.
/// Consumer bodies retain selected variant and payload facts but never claim ownership of the SDK declaration.
pub fn library_enum_declarations(type_info: &TypeCheckInfo) -> Vec<bir::EnumDeclaration> {
    runtime_enums::collect(type_info)
}

/// Source-validated stdlib constant values retained by the Body IR collector for this checked module.
/// Native consumers use these exact identities to distinguish retained literals from constants requiring SDK
/// publication. Missing values grant no permission to bypass publication or evaluate another source declaration.
pub fn library_constant_declarations(type_info: &TypeCheckInfo) -> Vec<bir::ConstantDeclaration> {
    library_constants::collect(type_info)
}

/// Crate-call records for the stdlib functions and model or class methods one module calls, retaining their type
/// parameters.
///
/// Each record names the compiled SDK crate item and the signature that crate compiled, with owned parameters and a
/// shared or exclusive receiver. A consumer linking the SDK crates calls the item only after checking that signature
/// against native metadata.
pub fn library_call_delegations(type_info: &TypeCheckInfo) -> Vec<bir::StdlibDelegation> {
    library_calls::collect(type_info)
}

/// The package-origin models, classes, enums and newtypes one module uses, including those it reaches only through a
/// stdlib signature, a checked expression type, or a selected trait default's concrete constructor. A consumer
/// resolves the ones its linked SDK packages publish.
pub fn library_type_identities(type_info: &TypeCheckInfo) -> Vec<CanonicalSymbolId> {
    let mut cache = crate::typechecker::stdlib_loader::StdlibAstCache::new();
    let mut identities = library_nominals::type_identities(type_info, &mut cache);
    identities.extend(library_opaque_returns::identities(type_info));
    identities.into_iter().collect()
}

/// Project exact SDK union owner and payload records after source modules and their selected SDK layouts are joined.
///
/// Only the provider plan's admitted emitted-definition table grants a native alias; semantic shape or a plausible hash
/// alone never establishes ownership. Missing or unsupported records remain absent for fail-closed lowering.
pub fn library_native_union_declarations(
    plan: &ProviderPlan,
    modules: &[bir::BodyIrModule],
) -> Result<Vec<bir::NativeUnionDeclaration>, String> {
    library_unions::collect(plan, modules)
}

/// A type leaf whose retained canonical identity must survive Result-side settlement.
enum RetainedTypeLeaf {
    /// A source or package nominal binding.
    Named(String),
    /// A builtin runtime carrier with exact root binding proof.
    NativeBuiltin(incan_semantics_core::RuntimeBuiltinType),
}

/// Collect the identity-bearing leaves of `ty` with their structural paths below `path`, through generic arguments and
/// tuple items, as `ResultVariant::has_complete_canonical_types` walks them.
fn named_leaf_paths(ty: &IncanType, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, RetainedTypeLeaf)>) {
    match ty {
        IncanType::Named(name) => out.push((path.clone(), RetainedTypeLeaf::Named(name.clone()))),
        IncanType::NativeBuiltin(builtin) => out.push((path.clone(), RetainedTypeLeaf::NativeBuiltin(builtin.clone()))),
        IncanType::Tuple(items) | IncanType::Generic { args: items, .. } => {
            for (index, item) in items.iter().enumerate() {
                path.push(index);
                named_leaf_paths(item, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Prepare one manifest-free parsed module so it satisfies [`build_body_ir_module_v0`]'s input contract.
///
/// This lives beside the boundary it governs rather than inside any one caller, because every manifest-free caller owes
/// Body IR the same debt. The parity corpus and source-observable comparison route use this helper; a caller that skips
/// it hands lowering a program the legacy path would never have produced, which is the divergence #1166 closes.
///
/// The legacy pipeline owes Body IR a desugared, feature-projected program, and it pays that debt at parse time:
/// `CompilationSession::parse_source` (`loaves/compiler/incan_driver/src/session.rs`) parses, runs
/// `vocab_desugar_pass::desugar_program_vocab_blocks`, then projects the result through the session's active
/// package features — all before the program is typechecked or lowered. A manifest-free caller has no package
/// feature graph, so it applies the same two steps with the empty feature projection. It must reject an explicit
/// package-feature selection rather than using this helper to approximate one.
///
/// Ordering matters beyond the pair itself. The caller applies this immediately after parsing, ahead of the backend's
/// `validate_direct_body_profile` and typechecking, because both of those must see the projected program: an import
/// behind an inactive feature is not part of this compilation, and refusing it as an unsupported profile boundary would
/// report a declaration the build does not contain.
///
/// # Errors
///
/// Returns the desugar pass's own diagnostics unchanged. A scoped-DSL surface whose owning library manifest is
/// unavailable is refused there, at that boundary, and this deliberately adds no second diagnostic for it.
pub fn apply_body_ir_input_contract(
    mut program: crate::ast::Program,
    entrypoint: &std::path::Path,
) -> Result<crate::ast::Program, Vec<crate::diagnostics::CompileError>> {
    let entrypoint_display = entrypoint.to_string_lossy();
    crate::vocab_desugar_pass::desugar_program_vocab_blocks(
        &mut program,
        Some(entrypoint_display.as_ref()),
        &crate::library_manifest_index::LibraryManifestIndex::default(),
    )?;
    Ok(program.projected_for_features(&std::collections::BTreeSet::new()))
}

/// Build Body IR using the checked provider-operation facts selected for this compilation.
///
/// The ordinary source-only replacement profile has no provider plan and continues to use
/// [`build_body_ir_module_v0`]. Any provider-aware consumer must use this entry point so provider-operation admission
/// is projected from an integrity-checked manifest rather than supplied as a handwritten lowering catalog.
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
        &[],
    ))
}

/// Build Body IR v0 for a typechecked module, admitting the internally projected provider operations.
///
/// The catalog is deliberately private to this frontend bridge. Its entries must come from a selected checked
/// [`ProviderPlan`], never from a backend-specific caller or a source-name convention.
///
/// The input contract on `program` and `type_info` is [`build_body_ir_module_v0`]'s, unchanged: a desugared,
/// feature-projected, typechecked module. The catalog widens which *calls* lower, never which source surface is
/// admitted, so a caller that skips the desugar pass violates the contract here exactly as it would there.
fn build_body_ir_module_v0_with_provider_operations(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
    provider_operations: &ProviderOperationCatalog,
    context: &[bir::BodyIrModule],
) -> bir::BodyIrModule {
    let module_identity = body_ir_module_identity(module_path);
    let module_id = CompilerNodeId::module(module_identity.clone());
    let constant_declarations = collect::collect_constants(program, type_info);
    let library_constants = library_constant_declarations(type_info);
    let published_constants = context
        .iter()
        .flat_map(|module| module.constant_declarations.iter())
        .chain(constant_declarations.iter().filter(|declaration| {
            matches!(
                declaration.value,
                bir::Constant::FrozenAggregate(..) | bir::Constant::Int(_)
            )
        }))
        .chain(library_constants.iter())
        .map(|declaration| (declaration.canonical.clone(), declaration.value.clone()))
        .collect::<HashMap<_, _>>();
    let function_default_sources = collect_function_default_sources(program, type_info);
    let local_function_declarations = collect_local_function_declarations(program);
    let enum_declarations = collect_local_enum_declarations(program, &module_identity, type_info);
    let runtime_enum_declarations = runtime_enums::collect(type_info);
    let local_enum_declarations = enum_declarations
        .iter()
        .chain(runtime_enum_declarations.iter())
        .chain(context.iter().flat_map(|module| module.enum_declarations.iter()))
        .map(|declaration| (declaration.canonical.clone(), declaration.clone()))
        .collect::<HashMap<_, _>>();
    let local_receiver_methods = collect_local_receiver_methods(program, type_info);
    let mut nominal_declarations = collect_local_nominal_declarations(program, &module_identity, type_info);
    let mut local_nominal_declarations = nominal_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalNominalDeclarations>();
    let fieldless_enum_declarations = collect_local_fieldless_enum_declarations(program, &module_identity, type_info);
    let mut local_fieldless_enum_declarations = fieldless_enum_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalFieldlessEnumDeclarations>();
    let value_enum_declarations = collect_local_value_enum_declarations(program, &module_identity, type_info);
    let mut local_value_enum_declarations = value_enum_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalValueEnumDeclarations>();
    // Stdlib layouts are selected like package layouts: by the checked canonical identity of a construction, pattern or
    // field, never by name. They stay out of this module's own declarations; the SDK crate owns their storage.
    for declaration in library_nominals::collect(type_info) {
        local_nominal_declarations.insert(declaration.canonical.render_compact(), declaration);
    }
    for module in context {
        for declaration in &module.nominal_declarations {
            local_nominal_declarations.insert(declaration.canonical.render_compact(), declaration.clone());
        }
        for declaration in &module.fieldless_enum_declarations {
            local_fieldless_enum_declarations.insert(declaration.canonical.render_compact(), declaration.clone());
        }
        for declaration in &module.value_enum_declarations {
            local_value_enum_declarations.insert(declaration.canonical.render_compact(), declaration.clone());
        }
    }
    let published_callable_returns = collect::collect_published_callable_returns(context);
    let runtime_type_conflicts = parameter_types::runtime_name_conflicts(context);
    let published_methods = published_methods::collect(context);
    let lowering_facts = BodyIrLoweringFacts {
        local_enum_declarations: &local_enum_declarations,
        published_methods: &published_methods,
        published_callable_returns: &published_callable_returns,
        published_constants: &published_constants,
        type_info,
        function_default_sources: &function_default_sources,
        local_function_declarations: &local_function_declarations,
        local_receiver_methods: &local_receiver_methods,
        local_nominal_declarations: &local_nominal_declarations,
        local_fieldless_enum_declarations: &local_fieldless_enum_declarations,
        local_value_enum_declarations: &local_value_enum_declarations,
        runtime_type_conflicts: &runtime_type_conflicts,
        module_identity: &module_identity,
        provider_operations,
        rust_module: program.rust_module_path.as_ref().map(|path| path.node.as_str()),
    };
    defaults::attach_nominal_field_defaults(program, &mut nominal_declarations, &lowering_facts);
    defaults::attach_method_partial_defaults(program, &mut nominal_declarations, &lowering_facts);
    let mut bodies = program
        .declarations
        .iter()
        .flat_map(|decl| -> Vec<bir::Body> {
            match &decl.node {
                ast::Declaration::Partial(partial) => {
                    vec![partials::lower_source_partial(partial, decl.span, &lowering_facts)]
                }
                ast::Declaration::Function(function) => {
                    vec![lower_function_body(function, decl.span, &lowering_facts)]
                }
                ast::Declaration::Model(model) => lower_owner_member_bodies(
                    &model.methods,
                    &model.properties,
                    &model.name,
                    owner_self_type(&model.name, &model.type_params),
                    &lowering_facts,
                ),
                ast::Declaration::Class(class) => lower_owner_member_bodies(
                    &class.methods,
                    &class.properties,
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
    let mut static_declarations = collect::collect_statics(program, &lowering_facts);
    let decorated_bindings =
        decorated_bindings::collect(program, &lowering_facts, &mut bodies, &mut static_declarations);
    apply_top_level_input_contract_refusal(program, &mut bodies);
    for body in &mut bodies {
        representation_facts::retain(body);
    }
    static_declarations.sort_by_key(|declaration| declaration.canonical.declaration_span.start);
    let mut module = bir::BodyIrModule {
        decorated_bindings,
        entrypoint_effects: entrypoint_effects::collect(program),
        constant_declarations,
        type_alias_declarations: collect::collect_type_aliases(program, type_info),
        trait_declarations: collect::collect_local_trait_declarations(program, type_info),
        trait_implementations: collect::collect_local_trait_implementations(
            program,
            type_info,
            &nominal_declarations,
            &enum_declarations,
        ),
        enum_declarations,
        stdlib_delegations: stdlib_delegations::collect(type_info),
        native_union_declarations: Vec::new(),
        static_declarations,
        module_id,
        nominal_declarations,
        fieldless_enum_declarations,
        value_enum_declarations,
        bodies,
    };
    let opaque_carriers = opaque_returns::collect(&module, type_info);
    library_opaque_returns::retain(&mut module, type_info);
    // Retain source trait annotations before local opaque propagation replaces return types with model witnesses.
    trait_values::retain_inputs(&mut module, program, type_info, context);
    trait_returns::retain_returns(&mut module, program, type_info, context);
    opaque_returns::retain(&mut module, &opaque_carriers);
    module
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

/// Source-local checked models and classes with canonical layouts.
///
/// Unsupported inheritance and callable aliases never enter this constructor registry; checked field
/// aliases select canonical storage.
/// Computed properties contribute getter bodies rather than layout fields.
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
    /// Checked local and published payload layouts indexed by their exact declaring identity.
    local_enum_declarations: &'source HashMap<CanonicalSymbolId, bir::EnumDeclaration>,
    /// Canonical provider-owned immutable method signatures available at the checked consumer boundary.
    published_methods: &'source [published_methods::PublishedMethod],
    /// Closed return carriers retained by a callable's providing module, keyed by its proven identity.
    published_callable_returns: &'source HashMap<CanonicalSymbolId, IncanType>,
    published_constants: &'source HashMap<CanonicalSymbolId, bir::Constant>,
    type_info: &'type_info TypeCheckInfo,
    /// Checked provider type names that cannot be distinguished from an ambient builtin by a bare signature leaf.
    runtime_type_conflicts: &'source std::collections::BTreeSet<String>,
    function_default_sources: &'source FunctionDefaultSources,
    local_function_declarations: &'source LocalFunctionDeclarations,
    local_receiver_methods: &'source [LocalReceiverMethod],
    local_nominal_declarations: &'source LocalNominalDeclarations,
    local_fieldless_enum_declarations: &'source LocalFieldlessEnumDeclarations,
    local_value_enum_declarations: &'source LocalValueEnumDeclarations,
    module_identity: &'source str,
    /// Provider operations this compilation admits, keyed by canonical identity rather than by any spelling.
    provider_operations: &'source ProviderOperationCatalog,
    /// The file's checked `rust.module(...)` path, which an `@rust.extern` declaration delegates into (#2023).
    rust_module: Option<&'source str>,
}

/// Source facts a synthesized local partial needs for one target parameter.
#[derive(Clone)]
struct FunctionDefaultSource {
    /// The target parameter's original span.
    param_span: ast::Span,
    /// The target's ordinary source default, if it declared one.
    default: Option<ast::Spanned<ast::Expr>>,
}

/// One physical immutable instance method whose checked signature can close a caller-open receiver.
struct LocalReceiverMethod {
    /// Checked nominal declaration containing this method.
    owner: CanonicalSymbolId,
    /// Source slot spelling; alias projection uses the checker's existing table.
    name: String,
    /// Physical method span selecting its checked signature and canonical target.
    span: ast::Span,
}

/// Determine whether a model can carry the small direct-replacement declaration fact.
///
/// This is deliberately a source-local data-model shape, not a general nominal-semantics predicate. The replacement
/// runtime admits builtin Rust derives, checked JSON protocol derives, and field aliases without serde renames, but
/// cannot execute other model decorators. Ordered owner and method binders remain placeholders for checked concrete
/// instance selection; adopted trait slots, including those of a trait adopted with checked type arguments, are
/// retained in the module implementation registry. Field defaults have
/// declaration-owned deferred computations; each construction's checked binding selects the omitted slots without
/// repeating source expressions at call sites.
pub fn is_direct_replacement_plain_model(model: &ast::ModelDecl) -> bool {
    model
        .decorators
        .iter()
        .all(|decorator| is_direct_replacement_model_derive(&decorator.node))
        && has_direct_replacement_model_shape(model, None)
}

/// Admit serde derives only after checked derive metadata resolves all selections to canonical supported paths, Copy
/// only after checking accepted it for every field, and `@rust.derive(...)` only when checking classified every derive
/// it names as a Rust builtin retained among the model's derives. A field alias crosses the boundary in the retained
/// declaration, so a serde form can rename the field as legacy does.
pub fn is_direct_replacement_checked_model(model: &ast::ModelDecl, type_info: &TypeCheckInfo) -> bool {
    if is_direct_replacement_plain_model(model) {
        return true;
    }
    has_checked_native_derives(
        &model.decorators,
        type_info.declarations.body_ir_model_derives.get(&model.name),
        type_info.declarations.rust_builtin_derive_models.contains(&model.name),
    ) && has_direct_replacement_model_shape(model, Some(type_info))
}

/// Admit a model or class whose decorators are all the bare builtin `derive` and whose checked native selection
/// names only derives the direct runtime retains.
///
/// Checking has already resolved module-qualified JSON protocol derives to their canonical serde paths, so the
/// selection, not the decorator spelling, decides admission. A declaration without a checked selection stays refused.
/// `rust_builtins` also admits `@rust.derive(...)` when checking recorded that it names only Rust builtin derives,
/// which the selection then already contains.
fn has_checked_native_derives(
    decorators: &[ast::Spanned<ast::Decorator>],
    selection: Option<&Vec<String>>,
    rust_builtins: bool,
) -> bool {
    let Some(names) = selection else {
        return false;
    };
    decorators.iter().all(|decorator| {
        let derive = &decorator.node;
        is_lint_suppression_decorator(derive)
            || (derive.name == "derive"
                && derive.type_args.is_empty()
                && (derive.path.segments == ["derive"]
                    || (rust_builtins && derive.path.segments == ["rust", "derive"])))
    }) && names.iter().all(|name| {
        matches!(
            name.as_str(),
            "Debug"
                | "Clone"
                | "Copy"
                | "Eq"
                | "PartialEq"
                | "Hash"
                | "Ord"
                | "PartialOrd"
                | "Default"
                | "Display"
                | "FieldInfo"
                | "IncanClass"
                | "serde::Serialize"
                | "serde::Deserialize"
        )
    })
}

/// Admit only the builtin `@staticmethod` and `@classmethod` method markers and `@rust.allow` lint suppressions.
///
/// Checking already reflects both markers in the method's facts: a static method has no receiver, and a
/// classmethod's `cls` is a checked type token whose constructions resolve to the owner. Every other method decorator
/// rebinds or wraps the method and stays refused.
pub fn is_direct_replacement_method_decorator(decorator: &ast::Decorator) -> bool {
    is_lint_suppression_decorator(decorator)
        || (decorator.args.is_empty()
            && decorator.type_args.is_empty()
            && matches!(
                incan_lang::lang::decorators::from_segments(&decorator.path.segments),
                Some(
                    incan_lang::lang::decorators::DecoratorId::StaticMethod
                        | incan_lang::lang::decorators::DecoratorId::ClassMethod
                )
            ))
}

/// Determine whether a decorator is an `@rust.allow` lint suppression.
///
/// A suppression only silences Rust lints on the item legacy emits. It changes no runtime behavior, and the direct
/// route emits no Rust source item for it to apply to, so it never blocks a declaration's admission.
pub fn is_lint_suppression_decorator(decorator: &ast::Decorator) -> bool {
    incan_lang::lang::decorators::from_segments(&decorator.path.segments)
        == Some(incan_lang::lang::decorators::DecoratorId::RustAllow)
}

/// Determine whether a nominal declaration uses at most one of type parameters, a generic trait adoption and method
/// aliases.
///
/// Each has parity evidence alone; any two together stay refused until that combination does.
fn has_at_most_one_open_shape_feature(type_parameters: bool, generic_adoption: bool, aliases: bool) -> bool {
    [type_parameters, generic_adoption, aliases]
        .iter()
        .filter(|present| **present)
        .count()
        <= 1
}

/// Determine whether every decorator on one method is a builtin marker [`is_direct_replacement_method_decorator`]
/// admits.
fn has_direct_replacement_method_decorators(method: &ast::MethodDecl) -> bool {
    method
        .decorators
        .iter()
        .all(|decorator| is_direct_replacement_method_decorator(&decorator.node))
}

/// Resolve the method a same-owner method partial targets: the method its target names, or the one a method alias of
/// that name rebinds. `None` unless exactly one method of the owner answers.
pub(crate) fn method_partial_target<'a>(
    methods: &'a [ast::Spanned<ast::MethodDecl>],
    aliases: &[ast::Spanned<ast::MethodAliasDecl>],
    partial: &ast::MethodPartialDecl,
) -> Option<&'a ast::Spanned<ast::MethodDecl>> {
    let target = aliases
        .iter()
        .find(|alias| alias.node.name == partial.target)
        .map_or(partial.target.as_str(), |alias| alias.node.target.as_str());
    let mut candidates = methods.iter().filter(|method| method.node.name == target);
    match (candidates.next(), candidates.next()) {
        (Some(method), None) => Some(method),
        _ => None,
    }
}

/// Determine whether every method partial of a nominal owner has a retained default frame: each partial targets,
/// directly or through a method alias, exactly one non-generic receiver method whose parameters its presets name.
/// Owner parameters close with the owner's retained layout and preset frame at each checked instantiation.
///
/// A call of such a partial is a call of its target method whose omitted presets are defaulted slots, so the frame
/// carrying the presets as defaults is the whole executable fact (see [`bir::MethodPartialDefaults`]).
pub fn has_direct_replacement_method_partials(
    _owner_type_params: &[ast::TypeParam],
    methods: &[ast::Spanned<ast::MethodDecl>],
    aliases: &[ast::Spanned<ast::MethodAliasDecl>],
    partials: &[ast::Spanned<ast::MethodPartialDecl>],
) -> bool {
    partials.iter().all(|partial| {
        method_partial_target(methods, aliases, &partial.node).is_some_and(|method| {
            method.node.receiver.is_some()
                && method.node.type_params.is_empty()
                && method.node.body.is_some()
                && partial.node.args.iter().all(|preset| {
                    method
                        .node
                        .params
                        .iter()
                        .any(|parameter| parameter.node.name == preset.name)
                })
        })
    })
}

/// Determine whether every computed property has a checked getter body; abstract properties remain refused.
/// Owner parameters stay in the immutable receiver frame, and property reads call the getter without adding layout
/// fields.
pub fn has_direct_replacement_properties(
    _owner_type_params: &[ast::TypeParam],
    properties: &[ast::Spanned<ast::PropertyDecl>],
) -> bool {
    properties.iter().all(|property| property.node.body.is_some())
}

/// Keep the structural admission boundary shared by builtin and checked serde derive selections.
/// Same-type method aliases keep the checked target identity and use its retained method body; they add no native
/// layout or wrapper declaration. Method partials retain deferred preset frames
/// ([`has_direct_replacement_method_partials`]) and computed properties getter bodies
/// ([`has_direct_replacement_properties`]). Without checked facts, methods may carry only builtin static and class
/// method markers. Checked nongeneric models also admit proven method replacement chains, retaining their original
/// callables and initialized bindings.
fn has_direct_replacement_model_shape(model: &ast::ModelDecl, type_info: Option<&TypeCheckInfo>) -> bool {
    has_direct_replacement_method_partials(
        &model.type_params,
        &model.methods,
        &model.method_aliases,
        &model.method_partials,
    ) && has_direct_replacement_properties(&model.type_params, &model.properties)
        && model.methods.iter().all(|method| {
            has_direct_replacement_method_decorators(&method.node)
                || model.type_params.is_empty()
                    && type_info
                        .is_some_and(|info| decorated_bindings::supports_method(&method.node, &model.name, info))
        })
}

/// Admit only the bare builtin derive decorator whose complete native trait selection Body IR retains.
/// Qualified, bundled, foreign, and semantic derives need their own checked expansion facts and remain refused. Display
/// retains its checked selection and uses the implied Debug rendering.
pub fn is_direct_replacement_model_derive(decorator: &ast::Decorator) -> bool {
    decorator.name == "derive"
        && decorator.path.segments == ["derive"]
        && decorator.type_args.is_empty()
        && decorator.args.iter().all(|argument| {
            matches!(argument, ast::DecoratorArg::Positional(value) if matches!(&value.node,
                ast::Expr::Ident(name) if matches!(name.as_str(), "Debug" | "Clone" | "Eq" | "PartialEq" | "Hash" | "Ord" | "PartialOrd" | "Default" | "Display")))
        })
}

/// Admit raw tuple wrappers with checked native derives, owner and method parameters, and trait adoptions.
///
/// A trait adopted with type arguments is instantiated from the checker-recorded arguments. Integer primitive
/// constraints retain their checked predicates at construction sites; source-local nongeneric hooks retain canonical
/// calls followed by checked-result extraction. Noninteger constraints and validated serde deserialization remain
/// outside this profile until their ingress is represented. Same-type method aliases, and short rebindings naming one
/// of the newtype's own methods, keep the checked target identity, as aliases on models do, unless combined with a
/// generic trait adoption or owner parameters; lint suppressions change nothing at run time. Associated type targets
/// are resolved by checking into the retained method frames and trait implementation identities. Interop edges and
/// method partials require additional representation facts and remain outside this profile. Bare and module-qualified
/// JSON protocol derives use the checker's canonical serde selections, as on models.
pub fn is_direct_replacement_checked_newtype(newtype: &ast::NewtypeDecl, type_info: &TypeCheckInfo) -> bool {
    let selections = newtype_derives::checked_newtype_derives(newtype, type_info);
    let Some(facts) = type_info.declarations.newtype_construction.get(&newtype.name) else {
        return false;
    };
    if (!facts.constraints.is_empty() || facts.checked_constructor.is_some())
        && selections
            .iter()
            .flatten()
            .chain(facts.automatic_derives.iter())
            .any(|selection| selection == "serde::Deserialize")
    {
        return false;
    }
    !newtype.is_rusttype
        && has_checked_native_derives(&newtype.decorators, selections.as_ref(), false)
        && newtype.rebindings.iter().all(|rebinding| {
            matches!(&rebinding.node.target.node, ast::Expr::Ident(target)
                if newtype.methods.iter().any(|method| method.node.name == *target))
        })
        && has_at_most_one_open_shape_feature(
            !newtype.type_params.is_empty(),
            newtype
                .traits
                .iter()
                .any(|adoption| !adoption.node.type_args.is_empty()),
            !newtype.method_aliases.is_empty() || !newtype.rebindings.is_empty(),
        )
        && newtype.method_partials.is_empty()
        && newtype.interop_edges.is_empty()
        && newtype
            .methods
            .iter()
            .all(|method| has_direct_replacement_method_decorators(&method.node))
}

/// Admit source classes whose fields and method bodies have complete direct-route facts.
///
/// Ordered owner parameters and method parameters are retained for checked instance scheduling, and trait
/// adoptions, including ones with checked type arguments, are retained in the implementation registry. Type
/// parameters, generic trait adoptions and same-type method aliases keep their independent checked identities and
/// can occur together. Derives use the checked selection shared with models, field defaults the declaration-owned
/// deferred computations models use, and
/// builtin `@staticmethod` and `@classmethod` markers the checker's method facts, and method partials deferred preset
/// frames and computed properties getter bodies, as on models; lint suppressions change nothing at run time.
/// Local concrete instance inheritance uses checked parent, field and nearest-dispatch facts; other inheritance stays
/// refused. Checked nongeneric method replacement chains retain originals and initialized bindings.
pub fn is_direct_replacement_checked_class(class: &ast::ClassDecl, type_info: &TypeCheckInfo) -> bool {
    has_checked_native_derives(
        &class.decorators,
        type_info.declarations.class_derives.get(&class.name),
        false,
    ) && (class.extends.is_none()
        || type_info
            .declarations
            .body_ir_class_inheritance
            .contains_key(&class.name))
        && has_direct_replacement_method_partials(
            &class.type_params,
            &class.methods,
            &class.method_aliases,
            &class.method_partials,
        )
        && has_direct_replacement_properties(&class.type_params, &class.properties)
        && class.declarative_members.is_empty()
        && class.fields.iter().all(|field| field.node.metadata.alias.is_none())
        && class.methods.iter().all(|method| {
            has_direct_replacement_method_decorators(&method.node)
                || class.type_params.is_empty()
                    && decorated_bindings::supports_method(&method.node, &class.name, type_info)
        })
}

/// Determine whether an enum carries the narrow source-local fieldless normal-enum declaration fact.
///
/// This excludes every declaration form whose behavior needs additional semantic representation: scalar value enums,
/// payload construction, aliases, trait dispatch, custom methods, decorators, and generic substitution. The direct
/// runtime can therefore materialize only a canonical unit carrier and compare its retained identity.
pub fn is_direct_replacement_fieldless_enum(enum_decl: &ast::EnumDecl) -> bool {
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

/// Admit checked value-enum layouts with supported ordinary/static bodies, and normal enum layouts with undecorated
/// methods and trait adoptions but without generic substitution in methods. An enum with type parameters is admitted
/// only without user methods or trait adoptions; its ordered binders and payload placeholders close through checked
/// layout instantiation. Variant aliases keep the selected canonical variant identity rather than adding a second
/// payload slot.
///
/// A normal enum's methods are ordinary bodies over its retained layout, and the trait slots its adopted local traits
/// fill are retained in the module implementation registry, as for a model. A value enum also retains its separate
/// canonical raw-value registry; its unit layout alone never authorizes scalar extraction. Lint suppressions change
/// nothing at run time.
pub fn is_direct_native_enum(value: &ast::EnumDecl) -> bool {
    is_direct_replacement_value_enum(value)
        || ((value.type_params.is_empty() || (value.methods.is_empty() && value.traits.is_empty()))
            && value.value_type.is_none()
            && value.methods.iter().all(|method| method.node.decorators.is_empty())
            && value
                .decorators
                .iter()
                .all(|decorator| is_bare_derive(&decorator.node) || is_lint_suppression_decorator(&decorator.node)))
}

/// Whether checking selected only derives the native route realizes for a source enum: Rust's builtin derives and
/// serde's, which a value enum writes through its raw values as legacy does and any other enum derives.
/// A normal enum's checked Display selection uses Incan's borrowed Debug formatting path, never a Rust derive.
///
/// The enum predicates admit any bare `@derive(...)`; this checked selection decides which of those reach a plan.
pub fn has_direct_native_enum_derives(value: &ast::EnumDecl, type_info: &TypeCheckInfo) -> bool {
    type_info
        .declarations
        .enum_derives
        .get(&value.name)
        .is_some_and(|names| {
            names.iter().all(|name| {
                (name == "Display" && value.value_type.is_none())
                    || matches!(
                        name.as_str(),
                        "Debug"
                            | "Clone"
                            | "Copy"
                            | "PartialEq"
                            | "Eq"
                            | "PartialOrd"
                            | "Ord"
                            | "Hash"
                            | "serde::Serialize"
                            | "serde::Deserialize"
                    )
            })
        })
}

/// The plain `@derive(...)` decorator, not a qualified or Rust derive spelled with the same final name.
fn is_bare_derive(decorator: &ast::Decorator) -> bool {
    decorator.name == "derive" && decorator.path.segments == ["derive"] && decorator.type_args.is_empty()
}

/// Determine whether an enum carries the narrow source-local RFC 032 scalar declaration fact.
///
/// The checked native enum layout and its derives accompany this scalar registry. Immutable ordinary and static
/// methods contribute independent checked bodies and do not change the variants or raw values. Trait dispatch,
/// aliases, generic substitution, payloads, and other method shapes remain excluded rather than inferred from a layout.
pub fn is_direct_replacement_value_enum(enum_decl: &ast::EnumDecl) -> bool {
    enum_decl
        .decorators
        .iter()
        .all(|decorator| is_bare_derive(&decorator.node))
        && enum_decl.type_params.is_empty()
        && enum_decl.value_type.is_some()
        && enum_decl.traits.is_empty()
        && enum_decl.variant_aliases.is_empty()
        && enum_decl
            .methods
            .iter()
            .all(|method| has_direct_value_enum_method_shape(&method.node))
        && enum_decl.variants.iter().all(|variant| {
            variant.node.fields.is_empty()
                && matches!(
                    variant.node.value.as_ref().map(|value| &value.node),
                    Some(ast::ValueEnumLiteral::Int(_) | ast::ValueEnumLiteral::Str(_))
                )
        })
}

/// Admit only concrete immutable or static enum bodies whose receiver contract already crosses the ordinary method
/// boundary.
///
/// Class receivers, mutable receivers, async bodies, and method binders need separate proof; their presence must not
/// silently broaden the scalar registry's source admission. Decorators are checked by the shared builtin registry.
fn has_direct_value_enum_method_shape(method: &ast::MethodDecl) -> bool {
    method.body.is_some()
        && method.type_params.is_empty()
        && !method.is_async()
        && method.receiver != Some(ast::Receiver::Mutable)
        && has_direct_replacement_method_decorators(method)
        && !method.decorators.iter().any(|decorator| {
            incan_lang::lang::decorators::from_segments(&decorator.node.path.segments)
                == Some(incan_lang::lang::decorators::DecoratorId::ClassMethod)
        })
}

/// Render a module path into the same module identity spelling [`crate::hir`] uses, so declaration ids line up between
/// the two representations.
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
    /// Locals whose selected SDK enum payload layout replaces legacy's opaque Rust pattern placeholder.
    native_enum_payload_bindings: HashSet<bir::LocalId>,
    /// The same retained layouts used to close constructor destinations and pattern payload facts.
    local_enum_declarations: &'source HashMap<CanonicalSymbolId, bir::EnumDeclaration>,
    /// Provider-owned method records; the Incan consumer receives their selected identity rather than names to
    /// resolve.
    published_methods: &'source [published_methods::PublishedMethod],
    /// Provider-owned closed return carriers; a private import alias is never a consumer-side layout key.
    published_callable_returns: &'source HashMap<CanonicalSymbolId, IncanType>,
    /// Immutable values decoded from the selected package representation, indexed by canonical declaration.
    published_constants: &'source HashMap<CanonicalSymbolId, bir::Constant>,
    type_info: &'type_info TypeCheckInfo,
    /// Canonical provider context that requires an explicit refusal for ambiguous bare builtin spellings.
    runtime_type_conflicts: &'source std::collections::BTreeSet<String>,
    /// Source defaults for top-level partial targets, retained only until they lower into Body IR.
    function_default_sources: &'source FunctionDefaultSources,
    /// Exact declarations physically present in this module, used only to retain same-module call identities.
    local_function_declarations: &'source LocalFunctionDeclarations,
    /// Physical source methods available when a retained payload closes a legacy caller-open receiver.
    local_receiver_methods: &'source [LocalReceiverMethod],
    /// Checked local and selected package models, used to retain exact constructor identity and layout.
    local_nominal_declarations: &'source LocalNominalDeclarations,
    /// Checked local and selected package fieldless enums, used to retain exact unit-member target identities.
    local_fieldless_enum_declarations: &'source LocalFieldlessEnumDeclarations,
    /// Checked local and selected package value enums, used to retain exact member target identity.
    local_value_enum_declarations: &'source LocalValueEnumDeclarations,
    /// Owning module identity used to construct a source-span declaration identity without consulting a backend.
    module_identity: &'source str,
    /// Provider operations this compilation admits, consulted only by canonical identity (see `provider_ops`).
    provider_operations: &'source ProviderOperationCatalog,
    /// Checked return type of the function/method currently being lowered, used only to retain `?` error routing.
    owner_return_type: IncanType,
    /// Explicit owner and callable binders whose checked types remain placeholders at this boundary.
    type_parameters: Vec<String>,
    /// Checked newtype owner of the current method, authorizing legacy's raw construction within its implementation.
    newtype_implementation: Option<CanonicalSymbolId>,
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
    /// Canonical storage currently selected by a direct local static binding; reads retain a global place rather than
    /// a snapshot.
    static_aliases: HashMap<bir::LocalId, bir::GlobalPlace>,
    /// Locals originally declared as storage bindings, which can detach to values and later select another static.
    static_binding_locals: HashSet<bir::LocalId>,
    /// Names lowering could not resolve to a tracked local (e.g. module-level `const`/`static`), reused across
    /// repeated reads instead of allocating a fresh external local per read.
    external_locals: HashMap<String, bir::LocalId>,
    /// Remaining textual reads for each tracked (non-temporary) local, seeded at declaration time by counting
    /// `Ident` occurrences of its name in the declaring scope's statement suffix (see [`count_reads_in_stmts`]).
    /// Decremented on every read; a decrement that reaches zero selects [`bir::OwnershipFact::Move`].
    remaining_reads: HashMap<bir::LocalId, usize>,
    /// Source-annotated function locals whose Rust representation is a function pointer rather than an inferred item.
    function_pointer_locals: Vec<bir::LocalId>,
    /// Local bindings the checker proved hold capturing callable values rather than function pointers.
    closure_holding_locals: Vec<bir::LocalId>,
    /// Locals whose value has been moved out via a full-value (non-projected) read, so scope-exit drop insertion
    /// skips them.
    moved_out: HashSet<bir::LocalId>,
    /// `mut` parameters of non-`Copy` type: the callee borrows the caller's value (#2022), so it never drops one.
    pub(super) borrowed_parameters: HashSet<bir::LocalId>,
    /// Rest parameters hold callee-owned packs, even though their checked storage types are collections.
    pub(super) owned_rest_parameters: HashSet<bir::LocalId>,
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
    /// Checked operations that run before `continue` reaches the innermost loop header. Counting ranges retain their
    /// increment here; ordinary loops retain an empty sequence.
    loop_continue_actions: Vec<Vec<bir::Statement>>,
    runtime_requirements: Vec<AbiV0RuntimeRequirement>,
    panic_facts: Vec<bir::PanicFact>,
    /// Source-span types that replace the checker's recorded ones while one evaluation of a chained value written per
    /// target is lowered. The checker checks such a value once per target but records one type per span, so each
    /// evaluation installs its own target's types here and clears them afterwards; it is empty everywhere else.
    type_overrides: HashMap<(usize, usize), IncanType>,
    /// Only a checker-planned shared method chain may rewrite its callable receiver signature positions.
    shared_method_chain: bool,
    /// Whether the pattern being lowered matches its scrutinee in place: an arm changes a part of a writable place
    /// through a name the pattern binds (#1561), so each such name borrows that part mutably instead of reading a
    /// copy.
    in_place_pattern: bool,
    /// Source-span types of `Ok(...)` and `Err(...)` values whose open `Result` side the checker settled where the
    /// value flows, on the call parameter, closure or comprehension it flows into rather than on the value's own span
    /// (see [`Self::settle_open_result_value`]).
    settled_types: HashMap<(usize, usize), IncanType>,
    /// Spans of values whose newtype coercion is being lowered, so lowering the wrapped value itself does not apply
    /// the coercion again (see `lower_newtype_coercion`).
    coercions_in_progress: HashSet<(usize, usize)>,
    next_local: u32,
    next_scope: u32,
}

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Start a fresh builder for one function body, with no locals, scopes, or accumulated facts yet.
    fn new(lowering_facts: &BodyIrLoweringFacts<'type_info, 'source>, owner_return_type: IncanType) -> Self {
        Self {
            native_enum_payload_bindings: HashSet::new(),
            local_enum_declarations: lowering_facts.local_enum_declarations,
            published_methods: lowering_facts.published_methods,
            published_callable_returns: lowering_facts.published_callable_returns,
            type_info: lowering_facts.type_info,
            runtime_type_conflicts: lowering_facts.runtime_type_conflicts,
            function_default_sources: lowering_facts.function_default_sources,
            local_function_declarations: lowering_facts.local_function_declarations,
            local_receiver_methods: lowering_facts.local_receiver_methods,
            local_nominal_declarations: lowering_facts.local_nominal_declarations,
            local_fieldless_enum_declarations: lowering_facts.local_fieldless_enum_declarations,
            local_value_enum_declarations: lowering_facts.local_value_enum_declarations,
            published_constants: lowering_facts.published_constants,
            module_identity: lowering_facts.module_identity,
            provider_operations: lowering_facts.provider_operations,
            owner_return_type,
            type_parameters: Vec::new(),
            newtype_implementation: None,
            locals: Vec::new(),
            scopes: Vec::new(),
            bindings: HashMap::new(),
            identity_bindings: HashMap::new(),
            static_aliases: HashMap::new(),
            static_binding_locals: HashSet::new(),
            external_locals: HashMap::new(),
            remaining_reads: HashMap::new(),
            function_pointer_locals: Vec::new(),
            closure_holding_locals: Vec::new(),
            moved_out: HashSet::new(),
            borrowed_parameters: HashSet::new(),
            owned_rest_parameters: HashSet::new(),
            materialized_range_locals: HashSet::new(),
            loop_break_targets: Vec::new(),
            loop_continue_actions: Vec::new(),
            runtime_requirements: Vec::new(),
            panic_facts: Vec::new(),
            type_overrides: HashMap::new(),
            shared_method_chain: false,
            in_place_pattern: false,
            settled_types: HashMap::new(),
            coercions_in_progress: HashSet::new(),
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

    /// Retain the checker's concrete executable constructor/call type when an open caller context masks it; otherwise
    /// use the contextual expression type. Missing facts remain [`IncanType::Unknown`], never inferred from operands.
    /// Standard-library Self calls use their additive declared-result fact before the legacy expression type, so
    /// enclosing operators can validate the same type that the call itself retains.
    /// Canonical local Result reads retain closed constructor channels without replacing fixed contextual channels.
    /// While one evaluation of a chained value written per target is lowered, that target's types for the value's
    /// nodes come first (see [`Self::type_overrides`]), then the settled type of a `Result` constructor whose open
    /// side its destination fixes (see [`Self::settled_types`]).
    /// A checked lexical read whose contextual type remains caller-open takes its active local's retained type by
    /// full canonical binding identity. This carries a concrete matched payload through the arm without changing
    /// legacy expression facts or resolving a variable by spelling.
    fn resolve_ty(&self, span: ast::Span) -> IncanType {
        if let Some(ty) = self.type_overrides.get(&(span.start, span.end)) {
            return ty.clone();
        }
        if let Some(ty) = self.settled_types.get(&(span.start, span.end)) {
            return ty.clone();
        }
        let native = self.type_info.body_ir_newtypes.expression_type(span, self.type_info);
        let checked = native
            .or_else(|| {
                self.type_info
                    .expressions
                    .body_ir_expr_types
                    .get(&(span.start, span.end))
            })
            .or_else(|| {
                self.type_info
                    .calls
                    .body_ir_constructor_types
                    .get(&(span.start, span.end))
            })
            .or_else(|| {
                self.type_info
                    .calls
                    .body_ir_call_result_types
                    .get(&(span.start, span.end))
            })
            .or_else(|| {
                self.type_info
                    .calls
                    .inferred_constructor_types
                    .get(&(span.start, span.end))
            })
            .or_else(|| {
                self.type_info
                    .calls
                    .inferred_call_result_types
                    .get(&(span.start, span.end))
            })
            .or_else(|| {
                self.type_info
                    .traits
                    .self_method_result_types
                    .get(&(span.start, span.end))
            })
            .or_else(|| self.type_info.expr_type(span))
            .or_else(|| {
                self.type_info
                    .calls
                    .body_ir_native_argument_types
                    .get(&(span.start, span.end))
            })
            .filter(|ty| native.is_some() || !self.type_info.body_ir_newtypes.controls_type(ty))
            .or_else(|| self.type_info.expr_type(span))
            .map(|ty| self.checked_type(ty))
            .unwrap_or(IncanType::Unknown);
        let checked = self.retained_result_read_type(span, checked);
        if let Some(identity) = self.type_info.resolved_identity(span)
            && let Some(local) = self
                .bindings
                .values()
                .filter(|id| self.native_enum_payload_bindings.contains(id))
                .filter_map(|id| self.locals.get(id.index()))
                .find(|local| local.identity.as_ref() == Some(identity))
        {
            return local.ty.clone();
        }
        if self.leaves_type_open(&checked)
            && let Some(identity) = self.type_info.resolved_identity(span)
            && let Some(local) = self
                .bindings
                .values()
                .filter_map(|id| self.locals.get(id.index()))
                .find(|local| local.identity.as_ref() == Some(identity) && !self.leaves_type_open(&local.ty))
        {
            return local.ty.clone();
        }
        checked
    }

    /// Whether `ty` leaves a part unresolved in this body: an unknown or still-inferred type, or a type parameter the
    /// body does not declare, which checking left uninstantiated where a call's result was used.
    fn leaves_type_open(&self, ty: &IncanType) -> bool {
        match ty {
            IncanType::Unknown | IncanType::Infer => true,
            // An imported native type has a qualified owner. The SDK's unresolved bare T is a placeholder,
            // and must yield to the closed checked carrier that contains it rather than masquerade as a type.
            IncanType::RustInteropPath(name) if name == "T" => !self.type_parameters.contains(name),
            IncanType::TypeVar(name) => !self.type_parameters.contains(name),
            IncanType::Generic { args, .. } | IncanType::Tuple(args) => {
                args.iter().any(|arg| self.leaves_type_open(arg))
            }
            IncanType::Ref(inner) | IncanType::RefMut(inner) => self.leaves_type_open(inner),
            IncanType::Function { params, return_type } => {
                params.iter().any(|param| self.leaves_type_open(&param.ty)) || self.leaves_type_open(return_type)
            }
            _ => false,
        }
    }

    /// Retain, for an `Ok(...)` or `Err(...)` value and each list, set, dict or tuple literal holding one, the type
    /// its destination settled for the side the constructor leaves open (#1561), and the destination's union for a
    /// side the constructor fixes as one of that union's members.
    ///
    /// The checker settles such a side where the value flows: a generic call's argument takes the parameter type the
    /// call's other arguments or the enclosing function's `Result` side fix, a closure's result takes the closure's
    /// settled return type, and a list comprehension's element the comprehension's element type. It records the
    /// settled type on the destination but keeps the open side on the constructor's own span, which the legacy
    /// backend pins from the destination (`pin_settled_closure_result_constructor` and its siblings in
    /// `loaves/compiler/incan_ir/src/lower/expr/helpers.rs`). Body IR builds the constructor with the settled type
    /// instead, through [`Self::settled_types`]. An open destination, a value of another shape, and a part whose
    /// shape differs from the destination's settle nothing.
    fn settle_open_result_value(&mut self, value: &ast::Spanned<ast::Expr>, destination: &IncanType) {
        if self.leaves_type_open(destination) {
            return;
        }
        if self.settle_enum_result_value(value, destination) {
            return;
        }
        match &value.node {
            ast::Expr::Paren(inner) => self.settle_open_result_value(inner, destination),
            ast::Expr::Call(callee, type_args, args)
                if type_args.is_empty()
                    && args.len() == 1
                    && self.intrinsic_result_constructor(callee, value.span).is_some() =>
            {
                self.settle_open_value_type(value.span, destination);
            }
            ast::Expr::List(entries) => {
                let Some([member]) = primitives::collection_member_types(destination, CollectionTypeId::List) else {
                    return;
                };
                for entry in entries {
                    if let ast::ListEntry::Element(element) = entry {
                        self.settle_open_result_value(element, member);
                    }
                }
                self.settle_open_value_type(value.span, destination);
            }
            ast::Expr::Set(elements) => {
                let Some([member]) = primitives::collection_member_types(destination, CollectionTypeId::Set) else {
                    return;
                };
                for element in elements {
                    self.settle_open_result_value(element, member);
                }
                self.settle_open_value_type(value.span, destination);
            }
            ast::Expr::Dict(entries) => {
                let Some([key_ty, value_ty]) = primitives::collection_member_types(destination, CollectionTypeId::Dict)
                else {
                    return;
                };
                for entry in entries {
                    if let ast::DictEntry::Pair(key, member) = entry {
                        self.settle_open_result_value(key, key_ty);
                        self.settle_open_result_value(member, value_ty);
                    }
                }
                self.settle_open_value_type(value.span, destination);
            }
            ast::Expr::Tuple(items) => {
                let IncanType::Tuple(item_types) = destination else {
                    return;
                };
                if items.len() != item_types.len() {
                    return;
                }
                for (item, item_ty) in items.iter().zip(item_types) {
                    self.settle_open_result_value(item, item_ty);
                }
                self.settle_open_value_type(value.span, destination);
            }
            _ => {}
        }
    }

    /// Retain `destination`'s parts for the parts the type recorded at `span` leaves open, and its unions for the
    /// members `span` fixes in their place, when the result is closed (see [`Self::settle_open_result_value`] and
    /// [`Self::fill_open_type_parts`]). A span with no recorded type, or one the destination changes nothing in, keeps
    /// what it has.
    fn settle_open_value_type(&mut self, span: ast::Span, destination: &IncanType) {
        let recorded = self.resolve_ty(span);
        if matches!(recorded, IncanType::Unknown) {
            return;
        }
        let settled = self.fill_open_type_parts(&recorded, destination);
        if settled != recorded && !self.leaves_type_open(&settled) {
            self.settled_types.insert((span.start, span.end), settled);
        }
    }

    /// The canonical identities of a constructed `Result`'s named leaves when its sides were settled at a destination
    /// (see [`Self::settle_open_result_value`]): the checker's identity at a path it recorded, otherwise the module
    /// binding the leaf names. A leaf with neither stays absent, so the construction refuses its payload identity
    /// rather than guessing one.
    pub(super) fn settled_result_identities(
        &self,
        result_ty: &IncanType,
        recorded: &std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId>,
    ) -> std::collections::BTreeMap<Vec<usize>, CanonicalSymbolId> {
        let mut leaves = Vec::new();
        if let IncanType::Generic { args, .. } = result_ty {
            for (index, side) in args.iter().enumerate() {
                named_leaf_paths(side, &mut vec![index], &mut leaves);
            }
        }
        leaves
            .into_iter()
            .filter_map(|(path, leaf)| {
                let identity = match leaf {
                    RetainedTypeLeaf::Named(name) => recorded
                        .get(&path)
                        .or_else(|| self.type_info.declarations.named_type_identities.get(&name)),
                    RetainedTypeLeaf::NativeBuiltin(builtin) => recorded.get(&path).or_else(|| {
                        self.type_info
                            .declarations
                            .body_ir_native_type_identities
                            .get(builtin.binding_name())
                            .filter(|identity| {
                                incan_semantics_core::RuntimeBuiltinType::from_identity(identity).as_ref()
                                    == Some(&builtin)
                            })
                    }),
                };
                identity.cloned().map(|identity| (path, identity))
            })
            .collect()
    }

    /// Fill each part of `recorded` this body leaves open with the matching part of `settled`, keeping every part
    /// `recorded` fixes and every part whose shape differs from `settled`'s.
    ///
    /// A fixed part that is one member of the union `settled` declares in its place takes that union: legacy builds
    /// `Ok("fine")` for a `Result[int | str, str]` destination as the union's `str` member.
    fn fill_open_type_parts(&self, recorded: &IncanType, settled: &IncanType) -> IncanType {
        match (recorded, settled) {
            (IncanType::Unknown | IncanType::Infer, _) => settled.clone(),
            (IncanType::TypeVar(name), _) if !self.type_parameters.contains(name) => settled.clone(),
            (_, IncanType::Generic { base, args })
                if base == "Union" && recorded != settled && args.iter().any(|member| member == recorded) =>
            {
                settled.clone()
            }
            (
                IncanType::Generic { base, args },
                IncanType::Generic {
                    base: settled_base,
                    args: settled_args,
                },
            ) if base == settled_base && args.len() == settled_args.len() => IncanType::Generic {
                base: base.clone(),
                args: args
                    .iter()
                    .zip(settled_args)
                    .map(|(arg, settled_arg)| self.fill_open_type_parts(arg, settled_arg))
                    .collect(),
            },
            (IncanType::Tuple(items), IncanType::Tuple(settled_items)) if items.len() == settled_items.len() => {
                IncanType::Tuple(
                    items
                        .iter()
                        .zip(settled_items)
                        .map(|(item, settled_item)| self.fill_open_type_parts(item, settled_item))
                        .collect(),
                )
            }
            _ => recorded.clone(),
        }
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
    /// [`ast::ComprehensionClause`]s plus a terminal element/key/value expression, not [`ast::Statement`]s.
    /// [`Self::declare_new_local`] is a thin wrapper over this that seeds `total_reads` from a statement suffix via
    /// [`count_reads_in_stmts`].
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
    /// fact for every `self` read before that countdown would ever be consulted. Computed getters use their separate
    /// Body IR-only receiver provenance when the legacy write table has no implicit receiver entry.
    fn declare_receiver_local(
        &mut self,
        ty: IncanType,
        mutable: bool,
        scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> bir::LocalId {
        let source_span = ast::Span::new(span.start, span.end);
        let identity = self
            .type_info
            .resolved_write_identity(source_span, "self")
            .cloned()
            .or_else(|| {
                self.type_info
                    .declarations
                    .body_ir_property_bindings
                    .get(&(source_span.start, source_span.end))
                    .and_then(|binding| binding.receiver_identity.clone())
            });
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

    /// Allocate a compiler-introduced temporary. Flattened expression temporaries are consumed once and omitted from
    /// last-use tracking and scope-exit drop insertion. A normalized construct that reuses a value must explicitly seed
    /// its read count and consume its final owner, as chained assignment does; loop state carries explicit poll and
    /// cursor reads instead. See [`Self::temp_operand`] and [`Self::insert_scope_drops`].
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
    /// spelling lookup. Checked union member reads carry an explicit payload projection while the local retains its
    /// storage type. Only a genuinely unproven reference may use the legacy `External` recovery local.
    /// A synthetic truthiness call shares this span with its receiver, whose separate Body IR identity selects the
    /// value here while the unchanged legacy reference still selects the method.
    /// Checked static decorator factories likewise retain their receiver's canonical identity and carrier separately
    /// from their selected method at a shared synthetic span.
    fn place_for_name(&mut self, name: &str, span: ast::Span, ty: &IncanType) -> Option<bir::Place> {
        if let Some(receiver) = self
            .type_info
            .declarations
            .body_ir_decorator_static_receivers
            .get(&(span.start, span.end))
            .filter(|receiver| receiver.binding_name == name)
        {
            return self
                .global_place(receiver.identity.clone(), self.checked_type(&receiver.ty))
                .map(bir::Place::from_global);
        }
        if let Some(identity) = self
            .type_info
            .protocols
            .body_ir_truthiness_receivers
            .get(&(span.start, span.end))
            .or_else(|| self.type_info.resolved_identity(span))
            .or_else(|| {
                self.type_info
                    .calls
                    .body_ir_native_argument_identities
                    .get(&(span.start, span.end))
            })
            .cloned()
        {
            if let Some(&id) = self.identity_bindings.get(&identity) {
                if let Some(global) = self.static_aliases.get(&id) {
                    return Some(bir::Place::from_global(global.clone()));
                }
                let mut place = bir::Place::from_local(id);
                // The local keeps its storage type; a read keeps the checker's narrower member as an explicit
                // projection.
                if let Some(local) = self.locals.get(id.index()) {
                    carrier_projection::retain_checked_narrowing(&mut place, &local.ty, ty);
                }
                return Some(place);
            }
            return self.global_place(identity, ty.clone()).map(bir::Place::from_global);
        }

        Some(bir::Place::from_local(
            self.external_local_for_name(name, hir_span(span)),
        ))
    }

    /// The storage a `module.NAME` member access reads, when its base is a module binding and the checker resolved the
    /// member to a `const` or `static` (`math.PI`, `helpers.DEFAULT_LABEL`).
    ///
    /// A module binding is a namespace, not a value, so the access is not a field of anything: it is the member's own
    /// canonical global place, the same one a bare imported name would read.
    fn module_member_place(&self, base: &ast::Spanned<ast::Expr>, access_span: ast::Span) -> Option<bir::Place> {
        if !self
            .type_info
            .resolved_identity(base.span)
            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Module)
        {
            return None;
        }
        let identity = self.type_info.resolved_identity(access_span)?.clone();
        self.global_place(identity, self.resolve_ty(access_span))
            .map(bir::Place::from_global)
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
    /// `self` reads in `loaves/compiler/incan_emit/src/ownership.rs`, which this module's own docs cite as precedent).
    /// A bare read of caller-owned parameter storage, identified by [`Self::is_borrowed_parameter`], never moves for
    /// the same reason: a collection parameter is a shared binding, or a mutable borrow when declared `mut` (RFC 129).
    /// An owned value read from it clones; passing it on to a `mut` parameter instead retains a reborrow fact.
    /// Every other bare local read decrements its remaining-reads countdown; reaching zero selects `Move` (and
    /// records the local as moved for [`Self::insert_scope_drops`]), otherwise `Clone`. A local with no tracked
    /// countdown (an [`bir::LocalOrigin::External`] reference) gets the explicit [`bir::OwnershipFact::Unknown`].
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
        if self.is_receiver_local(local) || self.is_borrowed_parameter(local) {
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

    /// Identify caller-owned parameter storage, including shared collection bindings (RFC 129).
    ///
    /// An ordinary list, set, or dictionary parameter borrows caller storage. A rest parameter owns its checked
    /// pack instead, so last-use reads may move it and scope exit drops it.
    fn is_borrowed_parameter(&self, local: bir::LocalId) -> bool {
        if self.owned_rest_parameters.contains(&local) {
            return false;
        }
        self.borrowed_parameters.contains(&local)
            || self.locals.get(local.index()).is_some_and(|decl| {
                matches!(decl.origin, bir::LocalOrigin::Parameter)
                    && matches!(&decl.ty, IncanType::Generic { base, .. } if matches!(collections::from_str(base), Some(CollectionTypeId::List | CollectionTypeId::Set | CollectionTypeId::Dict)))
            })
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
            if self.moved_out.contains(&id) || self.is_borrowed_parameter(id) {
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
        self.unsupported_typed_operand(description, IncanType::Unknown, scope, span, out)
    }

    /// Preserve a known expression carrier on a refusal placeholder, so frame admission cannot mask the named marker.
    /// The marker must abort execution before this uninitialized operand is consumed; its ownership remains unknown.
    fn unsupported_typed_operand(
        &mut self,
        description: String,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let temp = self.new_temp(ty, scope, span);
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

mod match_;

mod newtype_derives;

mod newtype_constraints;
mod newtype_field_ingress;
mod newtype_hooks;
mod newtype_ingress;
mod newtype_sum;
mod validation_errors;

mod closures;
mod local_partials;

mod async_;

mod literals;

mod calls;
mod library_calls;
mod library_constants;
mod library_defaults;
mod library_nominals;
mod library_opaque_returns;
mod library_unions;
mod runtime_enums;
pub(crate) mod runtime_nominals;
mod runtime_storage;
mod stdlib_delegations;

mod operators;

mod expr;

mod defaults;

mod assertions;

mod comprehensions;

mod control_flow;
mod iteration_protocol;

mod stmt;

pub(crate) mod free_vars;
mod properties;
mod provider_ops;
mod refusals;

mod reads;

mod carrier_projection;
mod collect;
mod collection_context;
mod decorated_bindings;
mod entrypoint_effects;
mod published_methods;
mod representation_facts;
mod result_channels;
mod static_aliases;

mod bodies;
mod enum_types;
mod opaque_returns;
mod parameter_types;
mod partial_closures;
mod partials;
mod text_codecs;
mod trait_returns;
mod trait_values;

mod primitives;

mod args;
mod fixed_spreads;
mod numeric_destinations;
mod rest_args;
mod variadics;

use bodies::*;
use collect::*;
use provider_ops::{ProviderOperationCatalog, ProviderOperationRecord};
use reads::*;

#[cfg(test)]
pub mod tests;

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
        // `(String)` is a parenthesized `String`, not a one-element tuple, so a single-name destructure must not
        // lower to `.0` against it.
        assert!(
            unsupported_tuple_destructure(&IncanType::RustInteropPath("(String)".to_string()), 1).is_some(),
            "a parenthesized Rust type has no `.0` field and must refuse to lower"
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
                &IncanType::RustInteropPath("(String,incan_std_data::json::JsonValue)".to_string()),
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
