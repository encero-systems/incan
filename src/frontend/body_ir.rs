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
//! assignment, `return`, `if`/`elif`/`else`, `while`, `for` (both a `start..end` range and a general iterable --
//! builtin collections or a resolved `__iter__`/`__next__` protocol, including the fallible `for item in
//! iterable?:` form), expression statements, statement-position `yield value` (see [`BodyBuilder::lower_stmt_into`]
//! and [`bir::Body::is_generator`]), `assert`, `pass`, `break` (including a value-producing `break` inside a `loop`
//! expression), `continue`. Expressions fully lowered: identifiers, literals (int/float/decimal/bool/string),
//! arithmetic/comparison/boolean binary operators and all three unary operators, calls and method calls (including
//! named, out-of-order, defaulted, and explicitly generic argument spellings -- see [`BodyBuilder::lower_call`]),
//! field access, indexing, slicing, parenthesization, tuples, list/dict/set literals (list and dict spread entries
//! included; set literals have no spread spelling), `model`/`class`
//! construction (named-only at the source level, bound to declared field order -- see
//! [`BodyBuilder::lower_nominal_construction`]), expression-position `if`/`loop`, `try` (`?`), f-strings,
//! list/dict comprehensions, lazy generator expressions, closure literals, partial callables (see
//! [`BodyBuilder::lower_closure`]/[`BodyBuilder::lower_partial`] for how captures
//! are computed and represented explicitly rather than left implicit), and `match` (see [`BodyBuilder::lower_match`]
//! for how patterns are lowered and their bindings scoped).
//!
//! Everything else lowers to an explicit `Statement::Unsupported` / `Operand::Unknown` node rather than panicking,
//! so the model stays total over real programs. That residue is not a short tail, and #1101 tracks it as named
//! remaining work rather than as an implied "almost everything" claim: a spread in a `model`/`class` construction,
//! which refuses as an unresolved field layout because the typechecker records no field binding for it; a spread
//! with no statically proven shape against a callee whose fixed signature *is* resolvable, whose arity no stage can
//! establish; a spread to a locally held callable value; the `**`, bitwise, shift, `in`/`not
//! in`, and `is`/`is not` operators and their compound forms; `if let`/`while let` conditions and destructuring
//! comprehension/generator clauses; statement-position `loop:`; `unsafe:` regions; `await` and `race for`; bytes
//! literals and a `Range` used as a value outside a `for` header; the pattern and `raises` `assert` forms; and
//! vocab/scoped-DSL surface nodes, which reach this module only when a caller skips the desugar pass the legacy
//! pipeline runs first. The sub-issues are #1158 through #1167, plus #1172 for evaluable callable defaults.
//!
//! Two coverage limits are silent rather than marked, and both are deliberate. Expression-position `yield` (the
//! two-way send/receive protocol) is a stub in the existing Rust-emission backend too, so there is no behavior to
//! preserve; the typechecker rejects a bare `yield` with no value before lowering runs. Newtype and enum method
//! bodies produce no [`bir::Body`] at all rather than an `Unsupported` one (#1163) -- see
//! [`lower_owner_method_bodies`].

use std::collections::{HashMap, HashSet};

use incan_core::lang::keywords::KeywordId;
use incan_semantics_core::SurfaceFeatureKey;
use incan_semantics_core::body_ir as bir;
use incan_semantics_core::{
    AbiV0RuntimeRequirement, CompilerNodeId, HirSourceSpan, IncanCallableParam, IncanCallableParamKind,
    IncanPrimitiveType, IncanType, rust_tuple_arity,
};

use incan_core::lang::surface::constructors::{self, ConstructorId};
use incan_core::lang::types::collections::{self, CollectionTypeId};

use crate::frontend::ast;
use crate::frontend::symbols::{CallableParam, ResolvedType};
use crate::frontend::typechecker::{
    FixedUnpackPlan, IdentKind, ResolvedOperatorKind, TypeCheckInfo, semantic_type_from_resolved,
};

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
pub fn build_body_ir_module_v0(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
) -> bir::BodyIrModule {
    let module_identity = body_ir_module_identity(module_path);
    let module_id = CompilerNodeId::module(module_identity.clone());
    let function_default_sources = collect_function_default_sources(program);
    let local_function_declarations = collect_local_function_declarations(program);
    let nominal_declarations = collect_local_nominal_declarations(program, &module_identity);
    let local_nominal_declarations = nominal_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalNominalDeclarations>();
    let fieldless_enum_declarations = collect_local_fieldless_enum_declarations(program, &module_identity);
    let local_fieldless_enum_declarations = fieldless_enum_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalFieldlessEnumDeclarations>();
    let value_enum_declarations = collect_local_value_enum_declarations(program, &module_identity);
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
    };
    let bodies = program
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
                _ => Vec::new(),
            }
        })
        .collect();
    bir::BodyIrModule {
        module_id,
        nominal_declarations,
        fieldless_enum_declarations,
        value_enum_declarations,
        bodies,
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
/// a checked constructor call to attach an exact declaration identity; the resulting [`bir::NominalDeclaration`]
/// records are the direct executor's sole layout authority. Classes, trait-adopting models, and models carrying
/// methods/properties/aliases are absent rather than being approximated as inert field bags.
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
}

/// Source facts a synthesized local partial needs for one target parameter.
#[derive(Clone)]
struct FunctionDefaultSource {
    /// The target parameter's original span.
    param_span: ast::Span,
    /// The target's ordinary source default, if it declared one.
    default: Option<ast::Spanned<ast::Expr>>,
}

/// Collect the source expressions a synthesized local partial needs to retain target defaults in Body IR.
fn collect_function_default_sources(program: &ast::Program) -> FunctionDefaultSources {
    program
        .declarations
        .iter()
        .filter_map(|decl| match &decl.node {
            ast::Declaration::Function(function) => Some((
                function.name.clone(),
                function
                    .params
                    .iter()
                    .map(|param| FunctionDefaultSource {
                        param_span: param.span,
                        default: param.node.default.clone(),
                    })
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

/// Collect the exact source spans eligible for same-module direct named-call dispatch.
fn collect_local_function_declarations(program: &ast::Program) -> LocalFunctionDeclarations {
    let mut declarations = LocalFunctionDeclarations::new();
    for declaration in &program.declarations {
        if let ast::Declaration::Function(function) = &declaration.node {
            declarations
                .entry(function.name.clone())
                .or_default()
                .push(declaration.span);
        }
    }
    declarations
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

/// Retain directly executable model declarations in source order.
///
/// Constructor argument binding already comes from the typechecker; this adds only the source-local declaration
/// identity and canonical raw field order the direct runtime otherwise could not establish without reopening AST or
/// typechecker state. This deliberately does not retain a general nominal registry.
fn collect_local_nominal_declarations(program: &ast::Program, module_identity: &str) -> Vec<bir::NominalDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Model(model) = &declaration.node else {
                return None;
            };
            is_direct_replacement_plain_model(model).then(|| bir::NominalDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_span(
                    module_identity,
                    declaration.span.start,
                    declaration.span.end,
                ),
                name: model.name.clone(),
                fields: model.fields.iter().map(|field| field.node.name.clone()).collect(),
                type_parameter_count: model.type_params.len(),
            })
        })
        .collect()
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

/// Retain exact source-local fieldless normal-enum declaration and unit-member facts in source order.
///
/// Only this registry reaches the direct runtime. It deliberately has no payload layouts, aliases, match facts, or
/// source-symbol lookup facility, so its existence cannot widen into general enum execution by spelling alone.
fn collect_local_fieldless_enum_declarations(
    program: &ast::Program,
    module_identity: &str,
) -> Vec<bir::FieldlessEnumDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Enum(enum_decl) = &declaration.node else {
                return None;
            };
            is_direct_replacement_fieldless_enum(enum_decl).then(|| bir::FieldlessEnumDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_span(
                    module_identity,
                    declaration.span.start,
                    declaration.span.end,
                ),
                name: enum_decl.name.clone(),
                variants: enum_decl
                    .variants
                    .iter()
                    .map(|variant| bir::FieldlessEnumVariantDeclaration {
                        direct_declaration_id: CompilerNodeId::declaration_span(
                            module_identity,
                            variant.span.start,
                            variant.span.end,
                        ),
                        name: variant.node.name.clone(),
                    })
                    .collect(),
            })
        })
        .collect()
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

/// Retain exact source-local RFC 032 value-enum declaration and canonical literal-member facts in source order.
///
/// A later direct executor receives only this Body-IR registry. It does not reopen AST/typechecker state to resolve
/// a `Name.Member` spelling, so lowering returns no record for imports, aliases, ordinary enums, or declarations
/// whose shape cannot truthfully support the generated scalar `.value()` surface.
fn collect_local_value_enum_declarations(
    program: &ast::Program,
    module_identity: &str,
) -> Vec<bir::ValueEnumDeclaration> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let ast::Declaration::Enum(enum_decl) = &declaration.node else {
                return None;
            };
            if !is_direct_replacement_value_enum(enum_decl) {
                return None;
            }
            let backing = match enum_decl.value_type.as_ref().map(|value| value.node) {
                Some(ast::ValueEnumType::Int) => bir::ValueEnumBacking::Int,
                Some(ast::ValueEnumType::Str) => bir::ValueEnumBacking::Str,
                None => return None,
            };
            let variants = enum_decl
                .variants
                .iter()
                .filter_map(|variant| {
                    let raw_value = match variant.node.value.as_ref().map(|value| &value.node) {
                        Some(ast::ValueEnumLiteral::Int(value)) if matches!(backing, bir::ValueEnumBacking::Int) => {
                            bir::Constant::Int(value.value)
                        }
                        Some(ast::ValueEnumLiteral::Str(value)) if matches!(backing, bir::ValueEnumBacking::Str) => {
                            bir::Constant::Str(value.clone())
                        }
                        _ => return None,
                    };
                    Some(bir::ValueEnumVariantDeclaration {
                        direct_declaration_id: CompilerNodeId::declaration_span(
                            module_identity,
                            variant.span.start,
                            variant.span.end,
                        ),
                        name: variant.node.name.clone(),
                        raw_value,
                    })
                })
                .collect::<Vec<_>>();
            (variants.len() == enum_decl.variants.len()).then(|| bir::ValueEnumDeclaration {
                direct_declaration_id: CompilerNodeId::declaration_span(
                    module_identity,
                    declaration.span.start,
                    declaration.span.end,
                ),
                name: enum_decl.name.clone(),
                backing,
                variants,
            })
        })
        .collect()
}

/// Lower every non-abstract method in `methods` (owned by the class/model/trait named `owner_name`) into one
/// [`bir::Body`] each, skipping abstract methods (`body: None`). `receiver_ty` is the typechecker-equivalent type
/// for a declared receiver: a concrete nominal type for models/classes or [`IncanType::SelfType`] for trait defaults.
///
/// Newtype and enum declarations also carry a `methods` field in the AST (see `crates/incan_syntax/src/ast/
/// decls.rs`), but #1102's own scope names only class/model/trait bodies, so this function is deliberately not
/// called for those two declaration kinds. #1163 owns extending it. Until then this is the module's only *silent*
/// coverage gap: every other unsupported construct leaves a `StatementKind::Unsupported` or `Operand::Unknown`
/// marker behind, while a newtype or enum method produces no [`bir::Body`] at all, so a consumer counting bodies
/// reads a program using one as fully represented.
fn lower_owner_method_bodies(
    methods: &[ast::Spanned<ast::MethodDecl>],
    owner_name: &str,
    receiver_ty: IncanType,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> Vec<bir::Body> {
    methods
        .iter()
        .filter_map(|method| lower_method_body(&method.node, method.span, owner_name, &receiver_ty, lowering_facts))
        .collect()
}

/// Render a module path into the same module identity spelling [`crate::frontend::hir`] uses, so declaration ids
/// line up between the two representations.
fn body_ir_module_identity(module_path: &[String]) -> String {
    if module_path.is_empty() {
        "<module>".to_string()
    } else {
        module_path.join("::")
    }
}

/// Convert an AST byte-offset span into a Body IR source span.
const fn hir_span(span: ast::Span) -> HirSourceSpan {
    HirSourceSpan::new(span.start, span.end)
}

/// Return the checked payload types of an intrinsic `Result[ok, error]` carrier.
///
/// This is deliberately a narrow query over the typechecker-owned semantic type. The Body-IR lowerer uses it only
/// to retain facts which direct execution cannot reconstruct: which intrinsic constructor is being formed, which
/// pattern payload is being bound, and whether `?` preserves the enclosing error type exactly. It does not infer
/// a conversion or admit a differently shaped generic carrier.
fn result_type_parts(ty: &IncanType) -> Option<(&IncanType, &IncanType)> {
    let IncanType::Generic { base, args } = ty else {
        return None;
    };
    (collections::from_str(base) == Some(CollectionTypeId::Result)).then_some(())?;
    match args.as_slice() {
        [ok_type, error_type] => Some((ok_type, error_type)),
        _ => None,
    }
}

/// Return just the checked error channel for an intrinsic `Result` carrier.
fn result_error_type(ty: &IncanType) -> Option<&IncanType> {
    result_type_parts(ty).map(|(_, error_type)| error_type)
}

/// Map only the compiler-owned intrinsic constructor spellings to Body-IR result variants.
fn result_variant_kind(name: &str) -> Option<bir::ResultVariantKind> {
    match constructors::from_str(name) {
        Some(ConstructorId::Ok) => Some(bir::ResultVariantKind::Ok),
        Some(ConstructorId::Err) => Some(bir::ResultVariantKind::Err),
        _ => None,
    }
}

/// Lower one function declaration's body into Body IR v0.
fn lower_function_body(
    function: &ast::FunctionDecl,
    decl_span: ast::Span,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> bir::Body {
    let decl_id = CompilerNodeId::declaration(lowering_facts.module_identity, &function.name);
    let direct_call_id =
        CompilerNodeId::declaration_span(lowering_facts.module_identity, decl_span.start, decl_span.end);
    // The bare-name map is a compatibility projection and collapses top-level overloads. A body is one physical
    // declaration, so its parameter types must come from the same span-keyed fact the direct-call identity uses.
    let binding = lowering_facts
        .type_info
        .declarations
        .function_bindings_by_span
        .get(&(decl_span.start, decl_span.end));
    let owner_return_type = binding
        .map(|binding| semantic_type_from_resolved(&binding.return_type))
        .unwrap_or(IncanType::Unknown);

    let mut builder = BodyBuilder::new(lowering_facts, owner_return_type);
    let root_scope = builder.new_scope(None, hir_span(decl_span));

    let mut param_locals = Vec::with_capacity(function.params.len());
    for (index, param) in function.params.iter().enumerate() {
        let ty = binding
            .and_then(|b| b.params.get(index))
            .map(|p| semantic_type_from_resolved(&p.ty))
            .unwrap_or(IncanType::Unknown);
        let local = builder.declare_new_local(
            param.node.name.clone(),
            ty,
            root_scope,
            hir_span(param.span),
            &function.body,
        );
        builder.locals[local.index()].origin = bir::LocalOrigin::Parameter;
        param_locals.push(local);
    }

    let mut params = Vec::with_capacity(function.params.len());
    for (param, local) in function.params.iter().zip(param_locals.iter().copied()) {
        let ty = builder.locals[local.index()].ty.clone();
        params.push(bir::CallableParam {
            local,
            name: param.node.name.clone(),
            ty,
            span: hir_span(param.span),
            default: builder.lower_callable_default(param.node.default.as_ref(), root_scope),
        });
    }

    let mut stmts = Vec::new();
    builder.lower_block_into(&function.body, root_scope, &mut stmts);
    builder.insert_scope_drops(&mut stmts, root_scope);

    if builder
        .locals
        .iter()
        .any(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
    {
        builder.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
    }

    bir::Body {
        decl_id,
        direct_call_id,
        name: function.name.clone(),
        span: hir_span(decl_span),
        locals: builder.locals,
        params,
        param_locals,
        scopes: builder.scopes,
        block: bir::Block {
            scope: root_scope,
            stmts,
        },
        runtime_requirements: builder.runtime_requirements,
        panic_facts: builder.panic_facts,
        is_async: function.is_async(),
    }
}

/// Lower one method declaration's body into Body IR v0, or `None` for an abstract method (`body: None` — a trait
/// requirement with no implementation, which has no body to lower).
///
/// Ordinary (non-receiver) method parameters declare with the resolved type the typechecker recorded in
/// [`DeclarationArtifacts::method_bindings_by_span`](
/// crate::frontend::typechecker::type_info::DeclarationArtifacts::method_bindings_by_span), keyed by this method's
/// own declaration span (#1121) — mirroring exactly how [`lower_function_body`] consumes `function_bindings` for
/// top-level `def` parameters. This lookup can only miss (falling back to [`IncanType::Unknown`], matching
/// `lower_function_body`'s own fallback) when the typechecker genuinely produced no fact for this declaration, such
/// as a method belonging to a declaration kind excluded from `TypeChecker::check_method_with_self_ty`'s call sites;
/// it is not the normal path for an ordinarily checked method. This does not change the accuracy of ownership facts
/// computed for actual *reads* of those parameters inside the body: those go through [`BodyBuilder::resolve_ty`] at
/// each read's own span, which is populated uniformly for every checked expression regardless of whether it sits in
/// a function or a method body.
///
/// The `self`/`mut self` receiver, when present, is declared as the body's first local (before ordinary
/// parameters) via [`BodyBuilder::declare_receiver_local`], typed with the typechecker-equivalent `receiver_ty`.
/// A method with `receiver: None` (a static/associated method) lowers with no receiver local at all, identically
/// in shape to a free function's body; its ordinary parameters still resolve through the same binding lookup.
fn lower_method_body(
    method: &ast::MethodDecl,
    decl_span: ast::Span,
    owner_name: &str,
    receiver_ty: &IncanType,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> Option<bir::Body> {
    let body_stmts = method.body.as_ref()?;

    // Method names are not unique across a module the way top-level function names are (two classes can each
    // declare a method named `new`), so the method's CompilerNodeId is scoped under its owning declaration's name
    // rather than reusing `CompilerNodeId::declaration(module_identity, &method.name)` directly.
    let decl_id = CompilerNodeId::declaration(
        lowering_facts.module_identity,
        &format!("{owner_name}::{}", method.name),
    );
    let direct_call_id =
        CompilerNodeId::declaration_span(lowering_facts.module_identity, decl_span.start, decl_span.end);
    let binding = lowering_facts
        .type_info
        .declarations
        .method_bindings_by_span
        .get(&(decl_span.start, decl_span.end));
    let owner_return_type = binding
        .map(|binding| semantic_type_from_resolved(&binding.return_type))
        .unwrap_or(IncanType::Unknown);

    let mut builder = BodyBuilder::new(lowering_facts, owner_return_type);
    let root_scope = builder.new_scope(None, hir_span(decl_span));

    let mut params = Vec::with_capacity(method.params.len() + 1);
    let mut param_locals = Vec::with_capacity(method.params.len() + 1);
    if let Some(receiver) = method.receiver {
        let mutable = matches!(receiver, ast::Receiver::Mutable);
        let self_local = builder.declare_receiver_local(receiver_ty.clone(), mutable, root_scope, hir_span(decl_span));
        param_locals.push(self_local);
        params.push(bir::CallableParam {
            local: self_local,
            name: "self".to_string(),
            ty: receiver_ty.clone(),
            span: hir_span(decl_span),
            default: bir::CallableParamDefault::Required,
        });
    }

    let mut ordinary_param_locals = Vec::with_capacity(method.params.len());
    for (index, param) in method.params.iter().enumerate() {
        let ty = binding
            .and_then(|b| b.params.get(index))
            .map(|p| semantic_type_from_resolved(&p.ty))
            .unwrap_or(IncanType::Unknown);
        let local = builder.declare_new_local(
            param.node.name.clone(),
            ty,
            root_scope,
            hir_span(param.span),
            body_stmts,
        );
        builder.locals[local.index()].origin = bir::LocalOrigin::Parameter;
        param_locals.push(local);
        ordinary_param_locals.push(local);
    }

    for (param, local) in method.params.iter().zip(ordinary_param_locals) {
        let ty = builder.locals[local.index()].ty.clone();
        params.push(bir::CallableParam {
            local,
            name: param.node.name.clone(),
            ty,
            span: hir_span(param.span),
            default: builder.lower_callable_default(param.node.default.as_ref(), root_scope),
        });
    }

    let mut stmts = Vec::new();
    builder.lower_block_into(body_stmts, root_scope, &mut stmts);
    builder.insert_scope_drops(&mut stmts, root_scope);

    if builder
        .locals
        .iter()
        .any(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
    {
        builder.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
    }

    Some(bir::Body {
        decl_id,
        direct_call_id,
        name: method.name.clone(),
        span: hir_span(decl_span),
        locals: builder.locals,
        params,
        param_locals,
        scopes: builder.scopes,
        block: bir::Block {
            scope: root_scope,
            stmts,
        },
        runtime_requirements: builder.runtime_requirements,
        panic_facts: builder.panic_facts,
        is_async: method.is_async(),
    })
}

/// Reconstruct the concrete `self` type for a method declared on `owner_name`, mirroring how
/// `check_method_with_self_ty` (`src/frontend/typechecker/check_decl.rs`) derives its own `self` binding's type:
/// a bare [`IncanType::Named`] for a non-generic owner, or an [`IncanType::Generic`] instantiated with the owner's
/// own type parameters (as type variables) for a generic owner. That typechecker-side resolved type is transient
/// checker state, not persisted anywhere in [`TypeCheckInfo`], so lowering rebuilds the equivalent type directly
/// from the AST rather than depending on a lookup table that does not exist.
fn owner_self_type(owner_name: &str, owner_type_params: &[ast::TypeParam]) -> IncanType {
    if owner_type_params.is_empty() {
        IncanType::Named(owner_name.to_string())
    } else {
        IncanType::Generic {
            base: owner_name.to_string(),
            args: owner_type_params
                .iter()
                .map(|type_param| IncanType::TypeVar(type_param.name.clone()))
                .collect(),
        }
    }
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
    /// Checked return type of the function/method currently being lowered, used only to retain `?` error routing.
    owner_return_type: IncanType,
    locals: Vec<bir::LocalDecl>,
    scopes: Vec<bir::ScopeInfo>,
    /// Current source-name -> local binding. Later bindings of the same name (new `let`/`mut` assignments) shadow
    /// earlier ones, matching the source-level scoping `BindingKind::Inferred`/`Let`/`Mutable` produce.
    bindings: HashMap<String, bir::LocalId>,
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
            owner_return_type,
            locals: Vec::new(),
            scopes: Vec::new(),
            bindings: HashMap::new(),
            external_locals: HashMap::new(),
            remaining_reads: HashMap::new(),
            moved_out: HashSet::new(),
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
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: Some(name.clone()),
            ty,
            origin: bir::LocalOrigin::UserBinding,
            scope,
            span,
        });
        self.bindings.insert(name, id);
        self.remaining_reads.insert(id, total_reads);
        id
    }

    /// Declare a method's `self`/`mut self` receiver as a [`bir::LocalOrigin::Receiver`] local, bound under the
    /// name `"self"` in [`Self::bindings`] exactly like an ordinary local so [`Self::local_for_name`] resolves
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
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: Some("self".to_string()),
            ty,
            origin: bir::LocalOrigin::Receiver { mutable },
            scope,
            span,
        });
        self.bindings.insert("self".to_string(), id);
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
            ty,
            origin: bir::LocalOrigin::Temporary,
            scope,
            span,
        });
        id
    }

    /// Resolve a source identifier to a local, synthesizing a cached [`bir::LocalOrigin::External`] local for names
    /// v0 cannot bind (module-level `const`/`static`, or anything else lowering does not yet track) instead of
    /// panicking on an unresolved name.
    fn local_for_name(&mut self, name: &str, span: HirSourceSpan) -> bir::LocalId {
        if let Some(&id) = self.bindings.get(name) {
            return id;
        }
        if let Some(&id) = self.external_locals.get(name) {
            return id;
        }
        let id = bir::LocalId(self.next_local);
        self.next_local += 1;
        self.locals.push(bir::LocalDecl {
            id,
            name: Some(name.to_string()),
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
        if self.is_receiver_local(place.local) {
            let fact = if is_copy {
                bir::OwnershipFact::Copy
            } else {
                bir::OwnershipFact::Clone
            };
            return (fact, false);
        }
        if is_copy {
            if let Some(remaining) = self.remaining_reads.get_mut(&place.local) {
                *remaining = remaining.saturating_sub(1);
            }
            return (bir::OwnershipFact::Copy, false);
        }
        let Some(remaining) = self.remaining_reads.get_mut(&place.local) else {
            return (bir::OwnershipFact::Unknown, false);
        };
        *remaining = remaining.saturating_sub(1);
        if *remaining == 0 {
            self.moved_out.insert(place.local);
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

    /// Lower every statement in `stmts` into `out`, within `scope`. Statements are lowered in source order and each
    /// one is given the statement suffix that follows it (`&stmts[index + 1..]`), so last-use countdowns seeded by
    /// [`Self::declare_new_local`] only count reads that can still occur after the declaration.
    fn lower_block_into(
        &mut self,
        stmts: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        for (index, stmt) in stmts.iter().enumerate() {
            self.lower_stmt_into(stmt, &stmts[index + 1..], scope, out);
        }
    }

    /// Lower one statement into `out`, dispatching on its AST kind. `remaining` is the statement suffix following
    /// `stmt` in its enclosing block, threaded through to [`Self::lower_assignment`] for last-use seeding. Statement
    /// kinds outside v0's covered subset fall through to an explicit [`Self::push_unsupported_stmt`] rather than
    /// panicking (see this module's module-level docs for the exact covered/uncovered split).
    fn lower_stmt_into(
        &mut self,
        stmt: &ast::Spanned<ast::Statement>,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        let span = hir_span(stmt.span);
        match &stmt.node {
            ast::Statement::Assignment(assignment) => self.lower_assignment(assignment, remaining, scope, span, out),
            ast::Statement::FieldAssignment(field_assignment) => {
                self.lower_field_assignment(field_assignment, scope, span, out)
            }
            ast::Statement::IndexAssignment(index_assignment) => {
                self.lower_index_assignment(index_assignment, scope, span, out)
            }
            ast::Statement::CompoundAssignment(compound_assignment) => {
                self.lower_compound_assignment(compound_assignment, scope, span, out)
            }
            ast::Statement::TupleUnpack(tuple_unpack) => {
                self.lower_tuple_unpack(tuple_unpack, remaining, scope, span, out)
            }
            ast::Statement::TupleAssign(tuple_assign) => self.lower_tuple_assign(tuple_assign, scope, span, out),
            ast::Statement::ChainedAssignment(chained_assignment) => {
                self.lower_chained_assignment(chained_assignment, remaining, scope, span, out)
            }
            ast::Statement::Return(value) => {
                let value = value.as_ref().map(|v| self.lower_expr_to_operand(v, scope, out));
                out.push(bir::Statement {
                    kind: bir::StatementKind::Return { value },
                    span,
                });
            }
            ast::Statement::If(if_stmt) => self.lower_if(if_stmt, scope, span, out),
            ast::Statement::While(while_stmt) => self.lower_while(while_stmt, scope, span, out),
            ast::Statement::For(for_stmt) => self.lower_for(for_stmt, scope, span, out),
            ast::Statement::Expr(expr) => {
                // `yield value` parses as an ordinary expression statement wrapping `ast::Expr::Yield(Some(_))`
                // (there is no separate `ast::Statement::Yield` AST node) -- mirror the existing Rust-emission
                // backend's own `lower_statement` (`src/backend/ir/lower/stmt.rs`), which special-cases this exact
                // shape before falling back to generic expression-statement lowering. A bare `yield` (no value)
                // falls through to the generic `Expr` arm below, same as that backend, and lowers via the
                // expression-position `yield` stub (see the module docs).
                if let ast::Expr::Yield(Some(value)) = &expr.node {
                    self.lower_yield(value, scope, span, out);
                } else {
                    let value = self.lower_expr_to_operand(expr, scope, out);
                    out.push(bir::Statement {
                        kind: bir::StatementKind::Expr { value },
                        span,
                    });
                }
            }
            ast::Statement::Assert(assert_stmt) => self.lower_assert(assert_stmt, scope, span, out),
            ast::Statement::Pass => {}
            ast::Statement::Break(value) => self.lower_break(value.as_ref(), scope, span, out),
            ast::Statement::Continue => out.push(bir::Statement {
                kind: bir::StatementKind::Continue,
                span,
            }),
            other => self.push_unsupported_stmt(unsupported_stmt_label(other), span, out),
        }
    }

    /// Lower an inferred/`let`/`mutable`/reassignment statement. A `Reassign` binding reuses the existing local for
    /// `assignment.name` when one is already bound (falling back to declaring a new one if reassignment targets an
    /// unbound name), while every other binding kind always declares a fresh local — matching source-level shadowing
    /// semantics, where a repeated `let x = ...` introduces a new binding rather than mutating the old one.
    fn lower_assignment(
        &mut self,
        assignment: &ast::AssignmentStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        // A closure value already carries the typechecker's callable shape. A partial retains that full callable
        // type, with captured presets represented as named-overrideable defaults. Positional calls skip those preset
        // slots, and `LocalCallableTarget::binding` records the resulting declaration mapping. Keeping this type on
        // the binding makes the local call contract agree with the `Rvalue::Closure` that creates the value.
        let ty = self
            .callable_value_ty(&assignment.value)
            .unwrap_or_else(|| self.resolve_ty(assignment.value.span));
        let value = self.lower_expr_to_operand(&assignment.value, scope, out);
        let local = match assignment.binding {
            ast::BindingKind::Reassign => self
                .bindings
                .get(&assignment.name)
                .copied()
                .unwrap_or_else(|| self.declare_new_local(assignment.name.clone(), ty, scope, span, remaining)),
            ast::BindingKind::Inferred | ast::BindingKind::Let | ast::BindingKind::Mutable => {
                self.declare_new_local(assignment.name.clone(), ty, scope, span, remaining)
            }
        };
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(local),
                rvalue: bir::Rvalue::Use(value),
            },
            span,
        });
    }

    /// Lower `obj.field = value` (including the compound `obj.field <op>= value` form). The parser already
    /// desugars a compound `FieldAssignmentStmt` so `value` is the full `obj.field <op> rhs` expression
    /// (`crates/incan_syntax/src/parser/stmts.rs`'s `assignment_or_expr_stmt`) -- `fa.compound_op` is purely a
    /// formatter hint for round-tripping `+=` spelling and carries no separate lowering semantics here, so this
    /// only needs to build the write-side place and lower `value` normally.
    fn lower_field_assignment(
        &mut self,
        field_assignment: &ast::FieldAssignmentStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let mut place = self.lower_expr_to_place(&field_assignment.object, scope, out);
        place
            .projection
            .push(bir::PlaceElem::Field(field_assignment.field.clone()));
        let value = self.lower_expr_to_operand(&field_assignment.value, scope, out);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(value),
            },
            span,
        });
    }

    /// Lower `obj[index] = value` (including the compound `obj[index] <op>= value` form, pre-desugared into
    /// `value` by the parser -- see [`Self::lower_field_assignment`]'s docs for the same note on
    /// `IndexAssignmentStmt::compound_op`). The object place is lowered before the index operand, preserving the
    /// established assignment evaluation order in the Rust-emission backend: object, index, then assigned value.
    fn lower_index_assignment(
        &mut self,
        index_assignment: &ast::IndexAssignmentStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let mut place = self.lower_expr_to_place(&index_assignment.object, scope, out);
        let index_operand = self.lower_expr_to_operand(&index_assignment.index, scope, out);
        place.projection.push(bir::PlaceElem::Index(Box::new(index_operand)));
        let value = self.lower_expr_to_operand(&index_assignment.value, scope, out);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(value),
            },
            span,
        });
    }

    /// Lower `name <op>= value` (`x += y`, `x &= y`, ...). Unlike field/index compound assignment, the parser
    /// leaves `ca.value` as the plain right-hand operand rather than pre-desugaring it, so this explicitly reads
    /// `name`'s current value, combines it with `value` via [`Self::lower_binary_from_operands`] (shared with
    /// [`Self::lower_binary`], so string-concat compound assignment routes through the same helper-call machinery
    /// as `+`), and writes the result back. An operator with no Body IR equivalent (see [`lower_binary_op`]) or a
    /// name that is not currently bound (should not happen after a successful typecheck) falls back to an explicit
    /// unsupported placeholder instead of panicking.
    fn lower_compound_assignment(
        &mut self,
        compound_assignment: &ast::CompoundAssignmentStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let Some(&local) = self.bindings.get(&compound_assignment.name) else {
            self.push_unsupported_stmt(
                format!("compound assignment to unbound name `{}`", compound_assignment.name),
                span,
                out,
            );
            return;
        };
        let lhs_ty = self.locals[local.index()].ty.clone();
        let op = compound_assignment.op.binary_op();
        let rhs_ty = self.resolve_ty(compound_assignment.value.span);
        if !Self::binary_op_is_supported(op, &lhs_ty, &rhs_ty) {
            self.push_unsupported_stmt(
                format!("compound assignment operator {:?}", compound_assignment.op),
                span,
                out,
            );
            return;
        }
        let lhs_place = bir::Place::from_local(local);
        let (fact, last_use) = self.ownership_fact_for_place(&lhs_place, &lhs_ty);
        let lhs_operand = bir::Operand::place(lhs_place, fact, last_use);
        let rhs_operand = self.lower_expr_to_operand(&compound_assignment.value, scope, out);
        let result = self.lower_binary_from_operands(
            op,
            &lhs_ty,
            lhs_operand,
            &rhs_ty,
            rhs_operand,
            lhs_ty.clone(),
            scope,
            span,
            out,
        );
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(local),
                rvalue: bir::Rvalue::Use(result),
            },
            span,
        });
    }

    /// Resolve or declare the local for one name bound by a multi-target assignment (tuple unpack or chained
    /// assignment). A `Reassign` binding reuses an existing local exactly like [`Self::lower_assignment`] does for
    /// a plain single-target reassignment; every other binding kind always declares a fresh local, matching
    /// source-level shadowing semantics.
    fn bind_multi_target_name(
        &mut self,
        name: &str,
        ty: IncanType,
        binding: ast::BindingKind,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        remaining: &[ast::Spanned<ast::Statement>],
    ) -> bir::LocalId {
        match binding {
            ast::BindingKind::Reassign => self
                .bindings
                .get(name)
                .copied()
                .unwrap_or_else(|| self.declare_new_local(name.to_string(), ty, scope, span, remaining)),
            ast::BindingKind::Inferred | ast::BindingKind::Let | ast::BindingKind::Mutable => {
                self.declare_new_local(name.to_string(), ty, scope, span, remaining)
            }
        }
    }

    /// Lower `a, b = value` / `let a, b = value` into a sequence of single-target `Assign` statements: materialize
    /// `value` once, then bind each name to the corresponding `.{index}` tuple-field projection off it, in
    /// left-to-right order. Element reads go through the same [`Self::ownership_fact_for_place`] a plain
    /// `.field`/`[index]` read anywhere else in v0 uses, so a non-Copy element borrows rather than moves (v0 does
    /// not track partial-move state out of a place, per [`Self::ownership_fact_for_place`]'s own docs) --
    /// consistent with, not a special case of, that existing policy.
    fn lower_tuple_unpack(
        &mut self,
        tuple_unpack: &ast::TupleUnpackStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let value_ty = self.resolve_ty(tuple_unpack.value.span);
        if let Some(reason) = unsupported_tuple_destructure(&value_ty, tuple_unpack.names.len()) {
            self.push_unsupported_stmt(reason, span, out);
            return;
        }
        let value_operand = self.lower_expr_to_operand(&tuple_unpack.value, scope, out);
        let value_place = self.materialize_operand_to_place(value_operand, value_ty.clone(), scope, span, out);
        let element_types = tuple_element_types(&value_ty, tuple_unpack.names.len());

        for (index, (name, element_ty)) in tuple_unpack.names.iter().zip(&element_types).enumerate() {
            let mut element_place = value_place.clone();
            element_place.projection.push(bir::PlaceElem::Field(index.to_string()));
            let (fact, last_use) = self.ownership_fact_for_place(&element_place, element_ty);
            let element_operand = bir::Operand::place(element_place, fact, last_use);
            let local =
                self.bind_multi_target_name(name, element_ty.clone(), tuple_unpack.binding, scope, span, remaining);
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(local),
                    rvalue: bir::Rvalue::Use(element_operand),
                },
                span,
            });
        }
    }

    /// Lower `t1, t2 = value` where the targets are lvalue expressions (`arr[i], arr[j] = ...`), not new bindings
    /// -- used for swaps and other multi-target reassignments. Materializes `value` once, then reads and
    /// materializes each element into its own fresh temporary *before* writing to any target, so aliased targets
    /// and sources (for example `arr[i], arr[j] = arr[j], arr[i]`) read the pre-assignment values rather than one
    /// another's already-written results. This is genuinely new coverage: the existing Rust-emission backend does
    /// not implement `TupleAssign` at all (`src/backend/ir/lower/stmt.rs` returns a `LoweringError`), so there is
    /// no existing behavior to mirror here -- the evaluation order above is v0's own design, chosen specifically
    /// to make `a, b = b, a` swap correctly.
    fn lower_tuple_assign(
        &mut self,
        tuple_assign: &ast::TupleAssignStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let value_ty = self.resolve_ty(tuple_assign.value.span);
        if let Some(reason) = unsupported_tuple_destructure(&value_ty, tuple_assign.targets.len()) {
            self.push_unsupported_stmt(reason, span, out);
            return;
        }
        let value_operand = self.lower_expr_to_operand(&tuple_assign.value, scope, out);
        let value_place = self.materialize_operand_to_place(value_operand, value_ty.clone(), scope, span, out);
        let element_types = tuple_element_types(&value_ty, tuple_assign.targets.len());

        let mut element_operands = Vec::with_capacity(tuple_assign.targets.len());
        for (index, element_ty) in element_types.iter().enumerate() {
            let mut element_place = value_place.clone();
            element_place.projection.push(bir::PlaceElem::Field(index.to_string()));
            let (fact, last_use) = self.ownership_fact_for_place(&element_place, element_ty);
            let element_operand = bir::Operand::place(element_place, fact, last_use);
            element_operands.push(self.push_assign_temp(
                bir::Rvalue::Use(element_operand),
                element_ty.clone(),
                scope,
                span,
                out,
            ));
        }

        for (target, value) in tuple_assign.targets.iter().zip(element_operands) {
            let place = self.lower_expr_to_place(target, scope, out);
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place,
                    rvalue: bir::Rvalue::Use(value),
                },
                span,
            });
        }
    }

    /// Lower `x = y = z = value` into `z = value; y = <read z>; x = <read y>` (rightmost target first), matching
    /// the direction the existing Rust-emission backend already chose for this same desugar
    /// (`src/backend/ir/lower/stmt.rs`'s `ChainedAssignment` arm).
    fn lower_chained_assignment(
        &mut self,
        chained_assignment: &ast::ChainedAssignmentStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let Some(last_name) = chained_assignment.targets.last() else {
            self.push_unsupported_stmt("empty chained assignment".to_string(), span, out);
            return;
        };
        let value_ty = self.resolve_ty(chained_assignment.value.span);
        let value_operand = self.lower_expr_to_operand(&chained_assignment.value, scope, out);
        let mut prev_local = self.bind_multi_target_name(
            last_name,
            value_ty.clone(),
            chained_assignment.binding,
            scope,
            span,
            remaining,
        );
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(prev_local),
                rvalue: bir::Rvalue::Use(value_operand),
            },
            span,
        });

        // Walk the remaining targets right-to-left, each one reading the local immediately to its right.
        for name in chained_assignment.targets[..chained_assignment.targets.len() - 1]
            .iter()
            .rev()
        {
            // `remaining_reads[prev_local]` was seeded only from statements *after* this whole chained-assignment
            // statement (see `Self::declare_new_local`'s `remaining` parameter) -- it does not know about the
            // synthetic read performed right here, within the very statement that (re)bound `prev_local`. Bump it
            // by one first so the shared `Self::ownership_fact_for_place` decrement below still lands on the
            // correct move/clone decision instead of under-counting by one.
            if let Some(remaining_count) = self.remaining_reads.get_mut(&prev_local) {
                *remaining_count += 1;
            }
            let place = bir::Place::from_local(prev_local);
            let (fact, last_use) = self.ownership_fact_for_place(&place, &value_ty);
            let operand = bir::Operand::place(place, fact, last_use);
            let local = self.bind_multi_target_name(
                name,
                value_ty.clone(),
                chained_assignment.binding,
                scope,
                span,
                remaining,
            );
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(local),
                    rvalue: bir::Rvalue::Use(operand),
                },
                span,
            });
            prev_local = local;
        }
    }

    /// Lower a `break` / `break value` statement. A value routes into the innermost enclosing loop's result place
    /// when that loop is a value-producing `loop:` expression (see [`Self::lower_loop_expr`]) -- otherwise it stays
    /// on the `Break` statement itself, matching [`bir::StatementKind::Break`]'s documented default. The innermost
    /// context comes from [`Self::loop_break_targets`], which every loop-lowering path pushes/pops around its own
    /// body so a `break` always targets the loop it is lexically inside, never an outer one.
    fn lower_break(
        &mut self,
        value: Option<&ast::Spanned<ast::Expr>>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let target = self.loop_break_targets.last().copied().flatten();
        match (value, target) {
            (Some(expr), Some(result_local)) => {
                let operand = self.lower_expr_to_operand(expr, scope, out);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(result_local),
                        rvalue: bir::Rvalue::Use(operand),
                    },
                    span,
                });
                out.push(bir::Statement {
                    kind: bir::StatementKind::Break { value: None },
                    span,
                });
            }
            _ => {
                let operand = value.map(|v| self.lower_expr_to_operand(v, scope, out));
                out.push(bir::Statement {
                    kind: bir::StatementKind::Break { value: operand },
                    span,
                });
            }
        }
    }

    /// Lower a statement-position `yield value` (`ast::Expr::Yield(Some(value))` reached through
    /// [`Self::lower_stmt_into`]'s `ast::Statement::Expr` arm) into a [`bir::StatementKind::Yield`].
    ///
    /// `value` is lowered through the same [`Self::lower_expr_to_operand`] path every other statement's operand
    /// goes through, so ownership facts/last-use tracking apply to a yielded value exactly like any other read.
    /// Records the runtime dependencies the existing Rust-emission backend's own `yield` lowering actually needs
    /// (`__incan_yield.yield_value(..)` on a `GeneratorYield` handle backed by `std::thread::spawn` and
    /// `std::sync::mpsc::sync_channel` -- see `crates/incan_stdlib/src/iter.rs`'s `Generator`/`SpawnedGenerator`):
    /// a named runtime helper (mirroring how [`Self::lower_fstring`] records `"fstring"` without a new
    /// [`bir::HelperOp`] variant, since `Yield` is its own statement kind, not a [`bir::Callee::Helper`] call),
    /// [`AbiV0RuntimeRequirement::HostedStd`] (the spawned-thread/channel machinery is not freestanding-compatible),
    /// and [`AbiV0RuntimeRequirement::Allocator`] (the channel and boxed iterator both allocate).
    fn lower_yield(
        &mut self,
        value: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let operand = self.lower_expr_to_operand(value, scope, out);
        out.push(bir::Statement {
            kind: bir::StatementKind::Yield { value: operand },
            span,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper("generator".to_string()));
        self.record_runtime_requirement(AbiV0RuntimeRequirement::HostedStd);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
    }

    /// Lower `if`/`elif`/`else` into a [`bir::StatementKind::If`] chain. `elif` branches are folded into nested
    /// `else { if ... }` wrappers from the last branch inward (see the inline comment above the fold loop), and an
    /// `if let` pattern condition — not yet modeled by v0 — lowers to an explicit unsupported placeholder instead of
    /// the real branch.
    fn lower_if(
        &mut self,
        if_stmt: &ast::IfStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let ast::Condition::Expr(cond_expr) = &if_stmt.condition else {
            self.push_unsupported_stmt("if-let pattern condition".to_string(), span, out);
            return;
        };
        let cond = self.lower_expr_to_operand(cond_expr, scope, out);

        let then_block = self.lower_branch_block(&if_stmt.then_body, scope, span);
        let mut else_block = if_stmt
            .else_body
            .as_ref()
            .map(|else_body| self.lower_branch_block(else_body, scope, span));

        // Fold `elif` branches into nested `else { if ... }` wrappers, innermost (last elif) first, so the earlier
        // conditions end up evaluated first at the top of the chain once wrapped by the outer `if` pushed below.
        for (elif_cond, elif_body) in if_stmt.elif_branches.iter().rev() {
            let mut wrapper = Vec::new();
            let cond_operand = self.lower_expr_to_operand(elif_cond, scope, &mut wrapper);
            let then_block = self.lower_branch_block(elif_body, scope, span);
            wrapper.push(bir::Statement {
                kind: bir::StatementKind::If {
                    cond: cond_operand,
                    then_block,
                    else_block,
                },
                span,
            });
            else_block = Some(bir::Block { scope, stmts: wrapper });
        }

        out.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond,
                then_block,
                else_block,
            },
            span,
        });
    }

    /// Lower one `if`/`elif`/`else` branch body into its own scoped [`bir::Block`]: allocate a child scope, lower
    /// the statements into it, then insert scope-exit drops. Shared by [`Self::lower_if`]'s then/else/elif bodies
    /// and [`Self::lower_if_expr`]'s then/else bodies, since both need exactly this shape.
    fn lower_branch_block(
        &mut self,
        body: &[ast::Spanned<ast::Statement>],
        parent_scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> bir::Block {
        let branch_scope = self.new_scope(Some(parent_scope), span);
        let mut stmts = Vec::new();
        self.lower_block_into(body, branch_scope, &mut stmts);
        self.insert_scope_drops(&mut stmts, branch_scope);
        bir::Block {
            scope: branch_scope,
            stmts,
        }
    }

    /// Lower an expression-position `if` (`ast::Expr::If`) into the same [`bir::StatementKind::If`] shape
    /// statement-position `if` uses (see [`Self::lower_if`]), reusing [`Self::lower_branch_block`] for both
    /// branches. The typechecker gives an expression-position `if` type `Unit` unconditionally (`check_if_expr` in
    /// `src/frontend/typechecker/check_expr/control_flow.rs` discards any branch value and always returns
    /// `ResolvedType::Unit`) -- unlike a `loop` expression, an `if` expression cannot yet produce a value from its
    /// branches, so its Body IR operand is always the `Unit` constant rather than a place read.
    fn lower_if_expr(
        &mut self,
        if_expr: &ast::IfExpr,
        scope: bir::ScopeId,
        span: ast::Span,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let cond = self.lower_expr_to_operand(&if_expr.condition, scope, out);
        let then_block = self.lower_branch_block(&if_expr.then_body, scope, hir_span_value);
        let else_block = if_expr
            .else_body
            .as_ref()
            .map(|body| self.lower_branch_block(body, scope, hir_span_value));
        out.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond,
                then_block,
                else_block,
            },
            span: hir_span_value,
        });
        bir::Operand::Constant(bir::Constant::Unit)
    }

    /// Lower a value-producing `loop:` expression (`ast::Expr::Loop`) into a [`bir::StatementKind::Loop`] plus a
    /// dedicated result local that every `break value` inside the loop's *own* body (not a nested loop's --
    /// enforced by [`Self::loop_break_targets`]) assigns into before exiting. The typechecker resolves this
    /// expression's type from the union of its `break value` operand types (`check_loop_expr` in
    /// `src/frontend/typechecker/check_expr/control_flow.rs`), so -- unlike an `if` expression, which is always
    /// `Unit` -- a `loop` expression's produced value genuinely comes from its branches and needs this
    /// merge-into-one-place treatment; see [`Self::lower_break`] for the other half of the mechanism.
    fn lower_loop_expr(
        &mut self,
        loop_expr: &ast::LoopExpr,
        scope: bir::ScopeId,
        span: ast::Span,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ty = self.resolve_ty(span);
        let loop_scope = self.new_scope(Some(scope), hir_span_value);
        let result_local = self.new_temp(ty.clone(), loop_scope, hir_span_value);

        self.loop_break_targets.push(Some(result_local));
        let mut body_stmts = Vec::new();
        self.lower_block_into(&loop_expr.body, loop_scope, &mut body_stmts);
        self.insert_scope_drops(&mut body_stmts, loop_scope);
        self.loop_break_targets.pop();

        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts: body_stmts,
                },
            },
            span: hir_span_value,
        });
        self.temp_operand(result_local, &ty)
    }

    /// Lower `while cond: body` into Body IR's single normalized loop shape: a [`bir::StatementKind::Loop`] whose
    /// body opens with `if not cond: break`, followed by the lowered loop body. A `while let` pattern condition —
    /// not yet modeled by v0 — lowers to an explicit unsupported placeholder instead of the real loop.
    fn lower_while(
        &mut self,
        while_stmt: &ast::WhileStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let ast::Condition::Expr(cond_expr) = &while_stmt.condition else {
            self.push_unsupported_stmt("while-let pattern condition".to_string(), span, out);
            return;
        };

        let loop_scope = self.new_scope(Some(scope), span);
        // `while` never produces a value from `break`, so push `None`: a `break` inside this loop's body must
        // resolve to a plain valueless exit even if this `while` is lexically nested inside a value-producing
        // `loop:` expression (see `Self::loop_break_targets`'s docs for why the stack exists).
        self.loop_break_targets.push(None);
        let mut body_stmts = Vec::new();
        let cond_operand = self.lower_expr_to_operand(cond_expr, loop_scope, &mut body_stmts);
        let negated = self.negate_operand(cond_operand, loop_scope, span, &mut body_stmts);
        let break_scope = self.new_scope(Some(loop_scope), span);
        let break_block = bir::Block {
            scope: break_scope,
            stmts: vec![bir::Statement {
                kind: bir::StatementKind::Break { value: None },
                span,
            }],
        };
        body_stmts.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond: negated,
                then_block: break_block,
                else_block: None,
            },
            span,
        });

        self.lower_block_into(&while_stmt.body, loop_scope, &mut body_stmts);
        self.insert_scope_drops(&mut body_stmts, loop_scope);
        self.loop_break_targets.pop();

        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts: body_stmts,
                },
            },
            span,
        });
    }

    /// Lower a `for` statement. `for x in start..end: body` (range-shaped iterables) lowers into a normalized
    /// counting `Loop`, preserving #1103's original range-loop shape unchanged. Every other iterable -- builtin
    /// collections (`List`/`Dict`/`String`) and user-defined iterables implementing the RFC 068 `__iter__`/
    /// `__next__` protocol, including the fallible `for item in iterable?:` form (RFC 115) -- lowers through
    /// [`Self::lower_general_iteration`], sharing its per-clause iteration primitive with comprehensions and
    /// generator expressions (see [`Self::lower_comprehension_clauses`]).
    ///
    /// Both paths accept the same loop-pattern subset the typechecker accepts -- a plain binding, `_`, and
    /// (recursively) a tuple of those, per `TypeChecker::define_for_pattern_bindings` in
    /// `src/frontend/typechecker/check_stmt.rs` (#1125). A plain `for x in ...` binds the produced item directly;
    /// every other shape writes it into a per-iteration temporary that [`Self::bind_for_pattern`] then projects one
    /// real named binding out of per bound name. Any shape outside that subset -- which the typechecker already
    /// rejects with its own diagnostic before lowering ever runs -- lowers to `Unsupported` naming the offending
    /// shape, checked up front so a refusal never leaves half-emitted bindings behind (the same
    /// "check before partially lowering" precedent as [`Self::lower_binary`] and [`Self::lower_match`]). The same
    /// up-front check also refuses a tuple pattern whose produced item is not a tuple of matching arity, so
    /// lowering can never invent `.0`/`.1` projections into a value that has no such fields -- see
    /// [`unsupported_for_pattern`].
    fn lower_for(
        &mut self,
        for_stmt: &ast::ForStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let item_ty = self.resolve_ty(for_stmt.pattern.span);
        if let Some(reason) = unsupported_for_pattern(&for_stmt.pattern.node, &item_ty) {
            self.push_unsupported_stmt(reason, span, out);
            return;
        }
        // The typechecker enters a lexical block scope for the loop header/body, so every binding introduced by the
        // pattern must disappear after the statement. Keep the active lookup map for restoration while leaving the
        // loop locals themselves in Body IR for the loop's statements to reference.
        let enclosing_bindings = self.bindings.clone();
        let ast::Expr::Range { start, end, inclusive } = &for_stmt.iter.node else {
            let loop_scope = self.new_scope(Some(scope), span);
            let item_local = self.declare_for_item_local(&for_stmt.pattern, &item_ty, loop_scope, span, &for_stmt.body);
            self.lower_general_iteration(
                &for_stmt.iter,
                item_local,
                scope,
                loop_scope,
                span,
                out,
                |builder, loop_scope, body_stmts| {
                    builder.bind_for_pattern(
                        &for_stmt.pattern,
                        &item_ty,
                        item_local,
                        loop_scope,
                        &for_stmt.body,
                        body_stmts,
                    );
                    builder.lower_block_into(&for_stmt.body, loop_scope, body_stmts);
                    builder.insert_scope_drops(body_stmts, loop_scope);
                },
            );
            self.bindings = enclosing_bindings;
            return;
        };

        let int_ty = IncanType::Primitive(IncanPrimitiveType::Int);
        let start_operand = self.lower_expr_to_operand(start, scope, out);
        let idx_local = self.new_temp(int_ty.clone(), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(idx_local),
                rvalue: bir::Rvalue::Use(start_operand),
            },
            span,
        });

        let loop_scope = self.new_scope(Some(scope), span);
        // `for` never produces a value from `break` (same reasoning as `while` -- see `Self::lower_while`).
        self.loop_break_targets.push(None);
        let mut body_stmts = Vec::new();

        let end_operand = self.lower_expr_to_operand(end, loop_scope, &mut body_stmts);
        let idx_read = bir::Operand::place(bir::Place::from_local(idx_local), bir::OwnershipFact::Copy, false);
        let cmp_op = if *inclusive { bir::BinOp::Gt } else { bir::BinOp::Ge };
        let cond = self.push_assign_temp(
            bir::Rvalue::BinaryOp(cmp_op, idx_read, end_operand),
            IncanType::Primitive(IncanPrimitiveType::Bool),
            loop_scope,
            span,
            &mut body_stmts,
        );
        let break_scope = self.new_scope(Some(loop_scope), span);
        body_stmts.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond,
                then_block: bir::Block {
                    scope: break_scope,
                    stmts: vec![bir::Statement {
                        kind: bir::StatementKind::Break { value: None },
                        span,
                    }],
                },
                else_block: None,
            },
            span,
        });

        // `for _ in start..end` binds nothing and the range's own index already drives the loop, so it needs no
        // per-iteration item local at all -- unlike the general path, where `IterNext` must still write the polled
        // item somewhere for the poll itself to happen.
        if !matches!(for_stmt.pattern.node, ast::Pattern::Wildcard) {
            let item_local = self.declare_for_item_local(&for_stmt.pattern, &item_ty, loop_scope, span, &for_stmt.body);
            body_stmts.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(item_local),
                    rvalue: bir::Rvalue::Use(bir::Operand::place(
                        bir::Place::from_local(idx_local),
                        bir::OwnershipFact::Copy,
                        false,
                    )),
                },
                span,
            });
            self.bind_for_pattern(
                &for_stmt.pattern,
                &item_ty,
                item_local,
                loop_scope,
                &for_stmt.body,
                &mut body_stmts,
            );
        }

        self.lower_block_into(&for_stmt.body, loop_scope, &mut body_stmts);
        self.insert_scope_drops(&mut body_stmts, loop_scope);

        let one = bir::Operand::Constant(bir::Constant::Int(1));
        let idx_read_for_incr = bir::Operand::place(bir::Place::from_local(idx_local), bir::OwnershipFact::Copy, false);
        let incremented = self.push_assign_temp(
            bir::Rvalue::BinaryOp(bir::BinOp::Add, idx_read_for_incr, one),
            int_ty,
            loop_scope,
            span,
            &mut body_stmts,
        );
        body_stmts.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(idx_local),
                rvalue: bir::Rvalue::Use(incremented),
            },
            span,
        });
        self.loop_break_targets.pop();

        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts: body_stmts,
                },
            },
            span,
        });
        self.bindings = enclosing_bindings;
    }

    /// Declare the local each produced item of a `for` loop is written into.
    ///
    /// A plain `for x in ...` binds the item directly: the item local *is* `x`'s local, so the produced value is
    /// never copied and the loop shape #1103/#1101 established is preserved byte-for-byte. Every other supported
    /// pattern shape has no single name to write into, so the item goes into a temporary that
    /// [`Self::bind_for_pattern`] projects the real bindings out of -- the same "materialize once, then bind each
    /// element off a projection" shape [`Self::lower_tuple_unpack`] already uses for `a, b = value`.
    fn declare_for_item_local(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        item_ty: &IncanType,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        body: &[ast::Spanned<ast::Statement>],
    ) -> bir::LocalId {
        match &pattern.node {
            ast::Pattern::Binding(name) => {
                let total_reads = count_reads_in_stmts(name, body);
                self.declare_new_local_with_reads(name.clone(), item_ty.clone(), loop_scope, span, total_reads)
            }
            _ => self.new_temp(item_ty.clone(), loop_scope, span),
        }
    }

    /// Emit the binding statements a `for` loop's pattern needs against the item local, immediately after the
    /// per-iteration `IterNext` (or, on the range path, after the index copy) has written it.
    ///
    /// A bare [`ast::Pattern::Binding`] emits nothing: [`Self::declare_for_item_local`] already declared the item
    /// local *as* that binding, so there is nothing left to project. Every other shape delegates to
    /// [`Self::bind_for_pattern_fields`], which means every binding that walk reaches is nested under at least one
    /// tuple field and therefore always reads through a projection.
    fn bind_for_pattern(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        item_ty: &IncanType,
        item_local: bir::LocalId,
        loop_scope: bir::ScopeId,
        body: &[ast::Spanned<ast::Statement>],
        out: &mut Vec<bir::Statement>,
    ) {
        if matches!(pattern.node, ast::Pattern::Binding(_)) {
            return;
        }
        let item_place = bir::Place::from_local(item_local);
        self.bind_for_pattern_fields(pattern, item_ty, &item_place, loop_scope, body, out);
    }

    /// Recursively bind one `for`-pattern node against `place`, the (already projected) part of the produced item
    /// it corresponds to, emitting one `Assign` per bound name in source order.
    ///
    /// Iteration binding is *irrefutable*: unlike [`Self::lower_match_pattern`], which builds a [`bir::Pattern`]
    /// for match-arm dispatch, there is nothing here to test or branch on, so this walk emits plain assignments and
    /// deliberately does not reuse that machinery (#1125 names conflating the two as a non-goal). What it does
    /// share is that walk's projection convention -- the zero-based tuple-element index spelled as a
    /// [`bir::PlaceElem::Field`], matching [`Self::lower_tuple_unpack`]'s `.0`/`.1` spelling -- and its
    /// [`tuple_element_types`] source for per-element types, so a nested tuple keeps resolved element types all the
    /// way down and falls back to [`IncanType::Unknown`] per slot only where the resolved type is not a tuple of
    /// the right arity.
    ///
    /// Each element is read through [`Self::ownership_fact_for_place`], exactly as
    /// [`Self::lower_tuple_unpack`] reads its own elements, so a non-Copy element borrows rather than moving out of
    /// a place v0 does not track partial-move state for. Each bound name becomes a real
    /// [`bir::LocalOrigin::UserBinding`] local in `loop_scope`, seeded with its own last-use countdown over the
    /// loop body, so [`Self::insert_scope_drops`] gives every non-Copy binding an explicit per-iteration drop.
    ///
    /// [`unsupported_for_pattern`] has already rejected every shape outside the accepted subset -- and every item
    /// type that is not a tuple of matching arity -- before [`Self::lower_for`] reaches this walk, so the remaining
    /// arms are unreachable in practice; they emit nothing rather than panicking if that invariant is ever violated
    /// by a hand-built AST.
    fn bind_for_pattern_fields(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        expected_ty: &IncanType,
        place: &bir::Place,
        loop_scope: bir::ScopeId,
        body: &[ast::Spanned<ast::Statement>],
        out: &mut Vec<bir::Statement>,
    ) {
        let span = hir_span(pattern.span);
        match &pattern.node {
            ast::Pattern::Wildcard => {}
            ast::Pattern::Binding(name) => {
                let (fact, last_use) = self.ownership_fact_for_place(place, expected_ty);
                let element = bir::Operand::place(place.clone(), fact, last_use);
                let total_reads = count_reads_in_stmts(name, body);
                let local =
                    self.declare_new_local_with_reads(name.clone(), expected_ty.clone(), loop_scope, span, total_reads);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(local),
                        rvalue: bir::Rvalue::Use(element),
                    },
                    span,
                });
            }
            ast::Pattern::Tuple(items) => {
                let element_types = tuple_element_types(expected_ty, items.len());
                for (index, (item, element_ty)) in items.iter().zip(&element_types).enumerate() {
                    let mut field_place = place.clone();
                    field_place.projection.push(bir::PlaceElem::Field(index.to_string()));
                    self.bind_for_pattern_fields(item, element_ty, &field_place, loop_scope, body, out);
                }
            }
            ast::Pattern::Literal(_) | ast::Pattern::Constructor(..) | ast::Pattern::Group(_) | ast::Pattern::Or(_) => {
            }
        }
    }

    /// Lower one general (non-range) iteration: materialize an iterator from `iter_expr` before the loop, then push
    /// a single [`bir::StatementKind::Loop`] whose body opens with a [`bir::StatementKind::IterNext`] writing each
    /// produced item into `pattern_local`, followed by `body_fn`. Shared by [`Self::lower_for`]'s general-iterable
    /// path and [`Self::lower_comprehension_clauses`]'s `for`-clause handling, so builtin-vs-protocol iteration is
    /// resolved in exactly one place rather than twice.
    ///
    /// Looks up [`TypeCheckInfo::protocol_iteration`] at `iter_expr`'s span to decide the [`bir::IterProtocol`]:
    /// `None` means a builtin collection or range, where "the iterator" is modeled as the iterable's own value (no
    /// method dispatch) -- a plain `Assign`; `Some` means a resolved `__iter__`/`__next__` protocol, where the
    /// iterator is obtained via an explicit `iter_method` [`bir::Callee::Method`] call. When the resolved protocol
    /// is fallible (`for item in iterable?:`, RFC 115), `iter_expr` is itself `ast::Expr::Try(inner)` with the `?`
    /// acting as the fallible-poll marker rather than an ordinary `Result` unwrap -- `inner` is lowered directly as
    /// the iterable in that case (matching the existing Rust-emission backend's own `(Expr::Try(inner), Some(_)) =>
    /// lower inner` special case in `src/backend/ir/lower/stmt.rs`), so the marker `?` is not double-lowered through
    /// [`Self::lower_try`]. Any other `Expr::Try` (an ordinary `for item in result_of_iterable?:` unwrap) falls
    /// through to the normal expression-lowering path, which already turns it into a
    /// [`bir::StatementKind::TryPropagate`] ahead of the loop via [`Self::lower_expr_to_place`]'s existing
    /// `Expr::Try` handling -- no special-casing needed for that form.
    ///
    /// The iterable is always read as a [`bir::OwnershipFact::Borrow`], matching
    /// [`Self::lower_method_call`]'s established receiver-borrow precedent (never an unsound move, and consistent
    /// with obtaining an iterator conceptually borrowing its source rather than consuming it at this normalized
    /// level); the materialized iterator local is polled with [`bir::OwnershipFact::MutBorrow`] each iteration,
    /// since polling advances its internal state.
    #[allow(clippy::too_many_arguments)]
    fn lower_general_iteration(
        &mut self,
        iter_expr: &ast::Spanned<ast::Expr>,
        pattern_local: bir::LocalId,
        outer_scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        let protocol = self.type_info.protocol_iteration(iter_expr.span).cloned();
        let fallible = protocol.as_ref().is_some_and(|p| p.fallible_error_type.is_some());
        let effective_iter_expr: &ast::Spanned<ast::Expr> = match (&iter_expr.node, fallible) {
            (ast::Expr::Try(inner), true) => inner,
            _ => iter_expr,
        };

        let iterable_place = self.lower_expr_to_place(effective_iter_expr, outer_scope, out);
        let iterator_ty = match &protocol {
            Some(p) => semantic_type_from_resolved(&p.iterator_type),
            None => self.resolve_ty(effective_iter_expr.span),
        };
        let iterator_local = self.new_temp(iterator_ty, outer_scope, span);
        match &protocol {
            Some(p) => out.push(bir::Statement {
                kind: bir::StatementKind::Call {
                    destination: Some(bir::Place::from_local(iterator_local)),
                    callee: bir::Callee::Method(bir::MethodTarget::synthesized(p.iter_method.clone())),
                    args: fixed_elements(vec![bir::Operand::place(
                        iterable_place,
                        bir::OwnershipFact::Borrow,
                        false,
                    )]),
                    may_panic: false,
                },
                span,
            }),
            None => out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(iterator_local),
                    rvalue: bir::Rvalue::Use(bir::Operand::place(iterable_place, bir::OwnershipFact::Borrow, false)),
                },
                span,
            }),
        }

        self.loop_break_targets.push(None);
        let mut body_stmts = Vec::new();

        let iter_protocol = match &protocol {
            Some(p) => bir::IterProtocol::UserDefined {
                next_method: p.next_method.clone(),
                fallible,
            },
            None => bir::IterProtocol::Builtin,
        };
        body_stmts.push(bir::Statement {
            kind: bir::StatementKind::IterNext {
                destination: bir::Place::from_local(pattern_local),
                iterator: bir::Operand::place(
                    bir::Place::from_local(iterator_local),
                    bir::OwnershipFact::MutBorrow,
                    false,
                ),
                protocol: iter_protocol,
            },
            span,
        });

        body_fn(self, loop_scope, &mut body_stmts);
        self.loop_break_targets.pop();

        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts: body_stmts,
                },
            },
            span,
        });
    }

    /// Lower a list comprehension `[expr for pattern in iter if filter]` into: an empty
    /// `AggregateKind::List` temporary, the desugared clause-chain loop (see
    /// [`Self::lower_comprehension_clauses`]), pushing each accepted element into it via a compiler-synthesized
    /// `push` [`bir::Callee::Method`] call, then a read of the completed list. Only v0's single mirrored
    /// `(pattern, iter, filter)` clause is lowered -- `comp.clauses` is intentionally not consulted, since neither
    /// the typechecker (`check_list_comp` in `src/frontend/typechecker/check_expr/comps.rs`) nor the existing
    /// Rust-emission backend (`src/backend/ir/lower/expr/comprehensions.rs`) reads it either; a list comprehension
    /// with more than one `for` clause is not actually type-checked or emitted as multi-clause today; treating
    /// `comp.clauses` as authoritative here would silently lower a shape nothing else in the pipeline validates.
    fn lower_list_comp(
        &mut self,
        comp: &ast::ListComp,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ty = self.resolve_ty(span);
        let list_local = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(list_local),
                rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::List, Vec::new()),
            },
            span: hir_span_value,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);

        let clauses = single_comprehension_clauses(&comp.pattern, &comp.iter, comp.filter.as_ref());
        let terminal = ComprehensionTerminal::ListPush {
            list_local,
            element: &comp.expr,
        };
        self.lower_scoped_comprehension_clauses(&clauses, &terminal, scope, hir_span_value, out);
        self.temp_operand(list_local, &ty)
    }

    /// Lower a dict comprehension `{key: value for pattern in iter if filter}` the same way
    /// [`Self::lower_list_comp`] lowers a list comprehension, but growing an `AggregateKind::Dict` temporary via a
    /// compiler-synthesized `insert` call. See [`Self::lower_list_comp`]'s docs for why only the single mirrored
    /// clause is lowered, not `comp.clauses`.
    fn lower_dict_comp(
        &mut self,
        comp: &ast::DictComp,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ty = self.resolve_ty(span);
        let dict_local = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(dict_local),
                rvalue: bir::Rvalue::Dict(Vec::new()),
            },
            span: hir_span_value,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);

        let clauses = single_comprehension_clauses(&comp.pattern, &comp.iter, comp.filter.as_ref());
        let terminal = ComprehensionTerminal::DictInsert {
            dict_local,
            key: &comp.key,
            value: &comp.value,
        };
        self.lower_scoped_comprehension_clauses(&clauses, &terminal, scope, hir_span_value, out);
        self.temp_operand(dict_local, &ty)
    }

    /// Lower a generator expression into a distinct, deferred [`bir::Rvalue::Generator`].
    ///
    /// The first `for` source is evaluated exactly once at construction, matching the established legacy
    /// iterator-adapter emitter. Its value and every other needed outer lexical value are then captured into fresh
    /// generator-local bindings. Clause polling, later `for` sources, filters, and element evaluation lower only
    /// into the generator body, so the enclosing body neither materializes the sequence nor runs a deferred effect.
    ///
    /// Body IR currently accepts only plain binding patterns for generator clauses. It rejects a whole generator
    /// expression before evaluating its source when another pattern shape would require a partially represented
    /// deferred binding protocol; that keeps unsupported forms visible rather than approximating them as a list.
    fn lower_generator_expr(
        &mut self,
        generator: &ast::GeneratorExpr,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let Some((first_clause, remaining_clauses)) = generator.clauses.split_first() else {
            return self.unsupported_operand(
                "generator expression without a for clause".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        let ast::ComprehensionClause::For {
            pattern: first_pattern,
            iter: first_iter,
        } = first_clause
        else {
            return self.unsupported_operand(
                "generator expression whose first clause is not a for clause".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        let ast::Pattern::Binding(first_name) = &first_pattern.node else {
            return self.unsupported_operand(
                "generator for-clause pattern is not a simple binding".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        if generator.clauses.iter().any(|clause| {
            matches!(clause, ast::ComprehensionClause::For { pattern, .. }
                if !matches!(pattern.node, ast::Pattern::Binding(_)))
        }) {
            return self.unsupported_operand(
                "generator for-clause pattern is not a simple binding".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }

        // The first source is the legacy adapter chain's eager boundary. Lowering it before creating the rvalue
        // preserves source-visible construction timing; all remaining expression lowering below writes only into
        // `generator_stmts` and therefore happens at poll time.
        let first_protocol = self.type_info.protocol_iteration(first_iter.span).cloned();
        let first_is_fallible = first_protocol
            .as_ref()
            .is_some_and(|protocol| protocol.fallible_error_type.is_some());
        let effective_first_iter: &ast::Spanned<ast::Expr> = match (&first_iter.node, first_is_fallible) {
            (ast::Expr::Try(inner), true) => inner,
            _ => first_iter,
        };
        let source = self.lower_expr_to_operand(effective_first_iter, scope, out);

        let generator_scope = self.new_scope(Some(scope), hir_span_value);
        let source_local = self.new_temp(
            self.resolve_ty(effective_first_iter.span),
            generator_scope,
            hir_span_value,
        );
        self.locals[source_local.index()].origin = bir::LocalOrigin::Captured;

        // Capture every lexical value used after the first source once, at construction. The body cannot read the
        // enclosing place directly after this point, and restoring the full binding map below prevents generator
        // clause/capture names from leaking into the following enclosing statement.
        let enclosing_bindings = self.bindings.clone();
        let free_names = free_vars_in_generator_deferred_body(generator);
        let mut captured_operands = Vec::with_capacity(free_names.len());
        let mut capture_locals = Vec::with_capacity(free_names.len());
        for name in &free_names {
            let Some(&outer_local) = self.bindings.get(name) else {
                // Module/external names remain explicit `External` references when the deferred body is lowered;
                // there is no local value available to capture and rebind here.
                continue;
            };
            let outer_ty = self.locals[outer_local.index()].ty.clone();
            let outer_place = bir::Place::from_local(outer_local);
            let (fact, last_use) = self.ownership_fact_for_place(&outer_place, &outer_ty);
            captured_operands.push(bir::Operand::place(outer_place, fact, last_use));

            let total_reads = count_reads_in_generator_deferred_body(name, generator);
            let capture_local =
                self.declare_new_local_with_reads(name.clone(), outer_ty, generator_scope, hir_span_value, total_reads);
            self.locals[capture_local.index()].origin = bir::LocalOrigin::Captured;
            capture_locals.push(capture_local);
        }

        let first_loop_scope = self.new_scope(Some(generator_scope), hir_span_value);
        let first_total_reads = count_reads_in_expr(first_name, &generator.expr.node)
            + count_reads_in_comprehension_clauses(first_name, remaining_clauses);
        let first_local = self.declare_new_local_with_reads(
            first_name.clone(),
            self.resolve_ty(first_pattern.span),
            first_loop_scope,
            hir_span(first_pattern.span),
            first_total_reads,
        );

        let mut generator_stmts = Vec::new();
        let iterator_ty = match &first_protocol {
            Some(protocol) => semantic_type_from_resolved(&protocol.iterator_type),
            None => self.resolve_ty(effective_first_iter.span),
        };
        let iterator_local = self.new_temp(iterator_ty, generator_scope, hir_span_value);
        match &first_protocol {
            Some(protocol) => generator_stmts.push(bir::Statement {
                kind: bir::StatementKind::Call {
                    destination: Some(bir::Place::from_local(iterator_local)),
                    callee: bir::Callee::Method(bir::MethodTarget::synthesized(protocol.iter_method.clone())),
                    args: fixed_elements(vec![bir::Operand::place(
                        bir::Place::from_local(source_local),
                        bir::OwnershipFact::Borrow,
                        false,
                    )]),
                    may_panic: false,
                },
                span: hir_span_value,
            }),
            None => generator_stmts.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place: bir::Place::from_local(iterator_local),
                    rvalue: bir::Rvalue::Use(bir::Operand::place(
                        bir::Place::from_local(source_local),
                        bir::OwnershipFact::Borrow,
                        false,
                    )),
                },
                span: hir_span_value,
            }),
        }

        self.loop_break_targets.push(None);
        let mut first_loop_stmts = vec![bir::Statement {
            kind: bir::StatementKind::IterNext {
                destination: bir::Place::from_local(first_local),
                iterator: bir::Operand::place(
                    bir::Place::from_local(iterator_local),
                    bir::OwnershipFact::MutBorrow,
                    false,
                ),
                protocol: match &first_protocol {
                    Some(protocol) => bir::IterProtocol::UserDefined {
                        next_method: protocol.next_method.clone(),
                        fallible: first_is_fallible,
                    },
                    None => bir::IterProtocol::Builtin,
                },
            },
            span: hir_span_value,
        }];
        let terminal = ComprehensionTerminal::GeneratorYield {
            element: &generator.expr,
        };
        self.lower_comprehension_clauses(
            remaining_clauses,
            &terminal,
            first_loop_scope,
            hir_span_value,
            &mut first_loop_stmts,
        );
        self.insert_scope_drops(&mut first_loop_stmts, first_loop_scope);
        self.loop_break_targets.pop();
        generator_stmts.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: first_loop_scope,
                    stmts: first_loop_stmts,
                },
            },
            span: hir_span_value,
        });
        self.bindings = enclosing_bindings;

        // `Generator::new` owns a boxed iterator in the legacy runtime, even when every captured source value is
        // Copy-shaped. Record that allocation fact directly rather than relying on incidental temporary locals.
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        let ty = self.resolve_ty(span);
        self.push_assign_temp(
            bir::Rvalue::Generator {
                source,
                captured_operands,
                body: Box::new(bir::GeneratorBody {
                    source_local,
                    capture_locals,
                    stmts: generator_stmts,
                }),
            },
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Lower a comprehension/generator clause chain with bindings that are lexical to that expression. The clause
    /// lowering itself declares each `for` pattern binding through [`Self::declare_new_local_with_reads`] so normal
    /// operand lowering can resolve it. Those bindings must disappear when the expression ends, however: unlike a
    /// statement `for`, a comprehension's `x` in `[x for x in values]` cannot shadow an enclosing `x` in the next
    /// enclosing statement. Preserve the outer lookup map while retaining the locals and ownership facts the nested
    /// lowering legitimately recorded in the Body IR.
    fn lower_scoped_comprehension_clauses(
        &mut self,
        clauses: &[ast::ComprehensionClause],
        terminal: &ComprehensionTerminal<'_>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let enclosing_bindings = self.bindings.clone();
        self.lower_comprehension_clauses(clauses, terminal, scope, span, out);
        self.bindings = enclosing_bindings;
    }

    /// Recursively desugar a comprehension/generator clause chain into nested `Loop`/`If` statements, terminating
    /// in `terminal`'s compiler-synthesized collection-growth call once every clause has been satisfied for one
    /// binding combination. `For` clauses reuse [`Self::lower_general_iteration`] (the same builtin-vs-protocol
    /// iteration primitive [`Self::lower_for`] uses), so comprehensions never duplicate that split. A non-binding
    /// `For` clause pattern lowers to `Unsupported`, matching [`Self::lower_for`]'s own restriction (destructuring
    /// patterns need `match`-shaped compilation, out of scope here).
    fn lower_comprehension_clauses(
        &mut self,
        clauses: &[ast::ComprehensionClause],
        terminal: &ComprehensionTerminal<'_>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let Some((head, tail)) = clauses.split_first() else {
            self.lower_comprehension_terminal(terminal, scope, out);
            return;
        };
        match head {
            ast::ComprehensionClause::If(cond) => {
                let cond_operand = self.lower_expr_to_operand(cond, scope, out);
                let then_scope = self.new_scope(Some(scope), span);
                let mut then_stmts = Vec::new();
                self.lower_comprehension_clauses(tail, terminal, then_scope, span, &mut then_stmts);
                out.push(bir::Statement {
                    kind: bir::StatementKind::If {
                        cond: cond_operand,
                        then_block: bir::Block {
                            scope: then_scope,
                            stmts: then_stmts,
                        },
                        else_block: None,
                    },
                    span,
                });
            }
            ast::ComprehensionClause::For { pattern, iter } => {
                let ast::Pattern::Binding(var_name) = &pattern.node else {
                    self.push_unsupported_stmt(
                        "comprehension for-clause pattern is not a simple binding".to_string(),
                        span,
                        out,
                    );
                    return;
                };
                let var_ty = self.resolve_ty(pattern.span);
                let loop_scope = self.new_scope(Some(scope), span);
                let total_reads = terminal.count_reads(var_name) + count_reads_in_comprehension_clauses(var_name, tail);
                let pattern_local =
                    self.declare_new_local_with_reads(var_name.clone(), var_ty, loop_scope, span, total_reads);
                self.lower_general_iteration(
                    iter,
                    pattern_local,
                    scope,
                    loop_scope,
                    span,
                    out,
                    move |builder, loop_scope, body_stmts| {
                        builder.lower_comprehension_clauses(tail, terminal, loop_scope, span, body_stmts);
                        builder.insert_scope_drops(body_stmts, loop_scope);
                    },
                );
            }
        }
    }

    /// Lower the innermost action of one accepted comprehension/generator binding combination: evaluate the
    /// element (or key/value) expression(s) and push a compiler-synthesized `push`/`insert`
    /// [`bir::Callee::Method`] call growing the target collection. The receiver is read as
    /// [`bir::OwnershipFact::MutBorrow`] since the call mutates the collection in place -- the first real producer
    /// of that fact in this module (every other place read so far has been `Copy`/`Move`/`Clone`/`Borrow`).
    fn lower_comprehension_terminal(
        &mut self,
        terminal: &ComprehensionTerminal<'_>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        match terminal {
            ComprehensionTerminal::ListPush { list_local, element } => {
                let element_operand = self.lower_expr_to_operand(element, scope, out);
                let span = hir_span(element.span);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Call {
                        destination: None,
                        callee: bir::Callee::Method(bir::MethodTarget::synthesized("push")),
                        args: fixed_elements(vec![
                            bir::Operand::place(
                                bir::Place::from_local(*list_local),
                                bir::OwnershipFact::MutBorrow,
                                false,
                            ),
                            element_operand,
                        ]),
                        may_panic: false,
                    },
                    span,
                });
            }
            ComprehensionTerminal::DictInsert { dict_local, key, value } => {
                let key_operand = self.lower_expr_to_operand(key, scope, out);
                let value_operand = self.lower_expr_to_operand(value, scope, out);
                let span = hir_span(value.span);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Call {
                        destination: None,
                        callee: bir::Callee::Method(bir::MethodTarget::synthesized("insert")),
                        args: fixed_elements(vec![
                            bir::Operand::place(
                                bir::Place::from_local(*dict_local),
                                bir::OwnershipFact::MutBorrow,
                                false,
                            ),
                            key_operand,
                            value_operand,
                        ]),
                        may_panic: false,
                    },
                    span,
                });
            }
            ComprehensionTerminal::GeneratorYield { element } => {
                let value = self.lower_expr_to_operand(element, scope, out);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Yield { value },
                    span: hir_span(element.span),
                });
            }
        }
    }

    /// Lower `assert cond[, message]`, recording an [`bir::PanicReason::AssertFailure`] panic fact and a
    /// [`AbiV0RuntimeRequirement::PanicStrategy`] runtime requirement since every assert can panic. The pattern
    /// (`assert value is Some(name)`) and `raises` (`assert call() raises E`) forms are not modeled by v0 and lower
    /// to an explicit unsupported placeholder instead (#1167). The pattern form's placeholder is lossy rather than
    /// merely incomplete: it discards the names the pattern would bind, so a later read of one lowers against a
    /// local this body never declared.
    fn lower_assert(
        &mut self,
        assert_stmt: &ast::AssertStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let ast::AssertKind::Condition(cond_expr) = &assert_stmt.kind else {
            self.push_unsupported_stmt("assert pattern/raises form".to_string(), span, out);
            return;
        };
        let cond = self.lower_expr_to_operand(cond_expr, scope, out);
        let message = assert_stmt
            .message
            .as_ref()
            .map(|m| self.lower_expr_to_operand(m, scope, out));
        self.panic_facts.push(bir::PanicFact {
            span,
            reason: bir::PanicReason::AssertFailure,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::PanicStrategy);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assert {
                cond,
                message,
                may_panic: true,
            },
            span,
        });
    }

    // ---- Callable defaults ----

    /// Lower one source-declared default into a deferred Body-IR computation.
    ///
    /// The ordinary function body may not contain this computation: source defaults run only when the matching
    /// parameter is omitted. While lowering it, callable-local bindings are hidden because the legacy path
    /// materializes source defaults while assembling call arguments, before the callee frame is bound. A default
    /// therefore becomes a closed Body-IR computation or a tagged refusal: a callable-local or other external
    /// source read, every explicitly unsupported Body-IR form, and a default without a usable canonical type fact
    /// refuse at the default expression's own span. The final condition is deliberately fail-closed: Body IR may
    /// not make an unchecked source default executable by reconstructing source semantics. This leaves a direct
    /// consumer no reason to consult AST/HIR/typechecker state or legacy execution.
    fn lower_callable_default(
        &mut self,
        default_expr: Option<&ast::Spanned<ast::Expr>>,
        scope: bir::ScopeId,
    ) -> bir::CallableParamDefault {
        let Some(default_expr) = default_expr else {
            return bir::CallableParamDefault::Required;
        };

        let locals_len = self.locals.len();
        let scopes_len = self.scopes.len();
        let runtime_requirements_len = self.runtime_requirements.len();
        let panic_facts_len = self.panic_facts.len();
        let next_local = self.next_local;
        let next_scope = self.next_scope;
        let saved_remaining_reads = self.remaining_reads.clone();
        let saved_moved_out = self.moved_out.clone();
        let saved_bindings = std::mem::take(&mut self.bindings);
        let saved_external_locals = std::mem::take(&mut self.external_locals);
        let mut stmts = Vec::new();
        let result = self.lower_expr_to_operand(default_expr, scope, &mut stmts);
        let mut unresolved_names: Vec<String> = self.external_locals.keys().cloned().collect();
        unresolved_names.sort();
        self.bindings = saved_bindings;
        self.external_locals = saved_external_locals;

        let refusal = first_unsupported_default_statement(&stmts)
            .or_else(|| {
                (!unresolved_names.is_empty()).then(|| {
                    (
                        hir_span(default_expr.span),
                        format!(
                            "default reads Body-IR-external name(s): {}",
                            unresolved_names.join(", ")
                        ),
                    )
                })
            })
            .or_else(|| {
                self.type_info
                    .validated_newtype_coercion(default_expr.span)
                    .is_some()
                    .then(|| {
                        (
                            hir_span(default_expr.span),
                            "default requires a validated-newtype coercion Body IR does not yet represent".to_string(),
                        )
                    })
            })
            .or_else(|| {
                matches!(
                    self.resolve_ty(default_expr.span),
                    IncanType::Unknown | IncanType::Never
                )
                .then(|| {
                    (
                        hir_span(default_expr.span),
                        "default expression lacks a usable typecheck fact".to_string(),
                    )
                })
            });
        if let Some((span, description)) = refusal {
            self.locals.truncate(locals_len);
            self.scopes.truncate(scopes_len);
            self.runtime_requirements.truncate(runtime_requirements_len);
            self.panic_facts.truncate(panic_facts_len);
            self.next_local = next_local;
            self.next_scope = next_scope;
            self.remaining_reads = saved_remaining_reads;
            self.moved_out = saved_moved_out;
            return bir::CallableParamDefault::Unsupported { span, description };
        }

        bir::CallableParamDefault::Source(bir::DefaultComputation {
            span: hir_span(default_expr.span),
            stmts,
            result,
        })
    }

    // ---- Expressions ----

    /// Lower one expression into an [`bir::Operand`], dispatching on its AST kind and, where evaluation has side
    /// effects or must be flattened (calls, binary/unary ops, aggregates), pushing supporting statements into `out`
    /// first. Expression kinds outside v0's covered subset fall through to [`Self::unsupported_operand`] rather than
    /// panicking (see this module's module-level docs for the exact covered/uncovered split).
    fn lower_expr_to_operand(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let span = hir_span(expr.span);
        match &expr.node {
            ast::Expr::Ident(name) => {
                let place = bir::Place::from_local(self.local_for_name(name, span));
                let ty = self.resolve_ty(expr.span);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            }
            ast::Expr::SelfExpr => {
                // Resolved exactly like `Ident("self")` — see `BodyBuilder::declare_receiver_local`, which binds
                // the receiver under the name "self" so this shares `local_for_name`'s ordinary lookup path. A
                // top-level function body can never actually contain `SelfExpr` (the parser only accepts it inside
                // a method), so this arm's `local_for_name` fallback to an `External` local is purely defensive.
                let place = bir::Place::from_local(self.local_for_name("self", span));
                let ty = self.resolve_ty(expr.span);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            }
            ast::Expr::Literal(lit) => match lower_literal(lit) {
                Some(constant) => bir::Operand::Constant(constant),
                None => self.unsupported_operand("bytes literal".to_string(), scope, span, out),
            },
            ast::Expr::Paren(inner) => self.lower_expr_to_operand(inner, scope, out),
            ast::Expr::Field(base, name) => {
                if let Some(target) = self.local_fieldless_enum_variant_target(base, name) {
                    return self.push_assign_temp(
                        bir::Rvalue::FieldlessEnumVariant(target),
                        self.resolve_ty(expr.span),
                        scope,
                        span,
                        out,
                    );
                }
                if let Some(target) = self.local_value_enum_variant_target(base, name) {
                    return self.push_assign_temp(
                        bir::Rvalue::ValueEnumVariant(target),
                        self.resolve_ty(expr.span),
                        scope,
                        span,
                        out,
                    );
                }
                let mut place = self.lower_expr_to_place(base, scope, out);
                place.projection.push(bir::PlaceElem::Field(name.clone()));
                let ty = self.resolve_ty(expr.span);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            }
            ast::Expr::Index(base, index) => {
                let index_operand = self.lower_expr_to_operand(index, scope, out);
                let mut place = self.lower_expr_to_place(base, scope, out);
                place.projection.push(bir::PlaceElem::Index(Box::new(index_operand)));
                let ty = self.resolve_ty(expr.span);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            }
            ast::Expr::Slice(base, slice) => self.lower_slice(base, slice, expr.span, scope, out),
            ast::Expr::Unary(op, inner) => {
                let un_op = lower_unary_op(*op);
                let operand = self.lower_expr_to_operand(inner, scope, out);
                let ty = self.resolve_ty(expr.span);
                self.push_assign_temp(bir::Rvalue::UnaryOp(un_op, operand), ty, scope, span, out)
            }
            ast::Expr::Binary(lhs, op, rhs) => self.lower_binary(lhs, *op, rhs, expr.span, scope, out),
            ast::Expr::Call(callee, type_args, args) => self.lower_call(callee, type_args, args, expr.span, scope, out),
            ast::Expr::MethodCall(recv, name, type_args, args) => {
                self.lower_method_call(recv, name, type_args, args, expr.span, scope, out)
            }
            ast::Expr::Tuple(items) => self.lower_aggregate(bir::AggregateKind::Tuple, items, expr.span, scope, out),
            ast::Expr::List(entries) => self.lower_list_literal(entries, expr.span, scope, out),
            ast::Expr::Dict(entries) => self.lower_dict(entries, expr.span, scope, out),
            ast::Expr::Set(items) => self.lower_aggregate(bir::AggregateKind::Set, items, expr.span, scope, out),
            ast::Expr::Constructor(name, args) => self.lower_constructor(name, args, expr.span, scope, out),
            ast::Expr::ListComp(comp) => self.lower_list_comp(comp, expr.span, scope, out),
            ast::Expr::DictComp(comp) => self.lower_dict_comp(comp, expr.span, scope, out),
            ast::Expr::Generator(generator) => self.lower_generator_expr(generator, expr.span, scope, out),
            ast::Expr::If(if_expr) => self.lower_if_expr(if_expr, scope, expr.span, out),
            ast::Expr::Loop(loop_expr) => self.lower_loop_expr(loop_expr, scope, expr.span, out),
            ast::Expr::Try(inner) => self.lower_try(inner, expr.span, scope, out),
            ast::Expr::FString(parts) => self.lower_fstring(parts, expr.span, scope, out),
            ast::Expr::Closure(params, body) => self.lower_closure(params, body, expr.span, scope, out),
            ast::Expr::Partial(partial) => self.lower_partial(partial, expr.span, scope, out),
            ast::Expr::Match(subject, arms) => self.lower_match(subject, arms, expr.span, scope, out),
            ast::Expr::Surface(surface) => self.lower_surface_expr(surface, expr.span, scope, out),
            other => self.unsupported_operand(unsupported_expr_label(other), scope, span, out),
        }
    }

    /// Lower an expression that is being used as a place base (the target of `.field`/`[index]` projection or a
    /// bare name), synthesizing a temporary to hold the value when the expression is not itself place-shaped.
    fn lower_expr_to_place(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Place {
        match &expr.node {
            ast::Expr::Ident(name) => bir::Place::from_local(self.local_for_name(name, hir_span(expr.span))),
            ast::Expr::SelfExpr => bir::Place::from_local(self.local_for_name("self", hir_span(expr.span))),
            ast::Expr::Field(base, name) => {
                let mut place = self.lower_expr_to_place(base, scope, out);
                place.projection.push(bir::PlaceElem::Field(name.clone()));
                place
            }
            ast::Expr::Index(base, index) => {
                let index_operand = self.lower_expr_to_operand(index, scope, out);
                let mut place = self.lower_expr_to_place(base, scope, out);
                place.projection.push(bir::PlaceElem::Index(Box::new(index_operand)));
                place
            }
            ast::Expr::Paren(inner) => self.lower_expr_to_place(inner, scope, out),
            _ => {
                let ty = self.resolve_ty(expr.span);
                let operand = self.lower_expr_to_operand(expr, scope, out);
                self.materialize_operand_to_place(operand, ty, scope, hir_span(expr.span), out)
            }
        }
    }

    /// Ensure `operand` is place-shaped, materializing a fresh temporary holding it first if it is a bare constant.
    /// Used wherever a value that has already been lowered to an [`bir::Operand`] needs a [`bir::Place`] to project
    /// further into -- [`Self::lower_expr_to_place`]'s own non-place-shaped fallback, plus tuple-element
    /// extraction for [`Self::lower_tuple_unpack`]/[`Self::lower_tuple_assign`].
    fn materialize_operand_to_place(
        &mut self,
        operand: bir::Operand,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Place {
        match operand {
            bir::Operand::Place(place_operand) => place_operand.place,
            constant @ bir::Operand::Constant(_) => {
                let temp = self.new_temp(ty, scope, span);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(temp),
                        rvalue: bir::Rvalue::Use(constant),
                    },
                    span,
                });
                bir::Place::from_local(temp)
            }
        }
    }

    /// Lower a binary-operator expression. Bails out to an explicit unsupported placeholder *before* evaluating
    /// either operand when `op` has no Body IR v0 handling at all (see [`Self::binary_op_is_supported`]), so an
    /// unsupported operator's sub-expressions are never partially lowered. Otherwise defers to
    /// [`Self::lower_binary_from_operands`] for the actual string-helper-or-plain-binop emission, which is also
    /// shared with [`Self::lower_compound_assignment`].
    fn lower_binary(
        &mut self,
        lhs: &ast::Spanned<ast::Expr>,
        op: ast::BinaryOp,
        rhs: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let lhs_ty = self.resolve_ty(lhs.span);
        let rhs_ty = self.resolve_ty(rhs.span);
        let result_ty = self.resolve_ty(span);

        // A user-defined operator is a method call, not a primitive operation. The typechecker already resolved
        // which dunder this spelling dispatches to, so lowering follows that decision rather than falling through
        // to the primitive operator set -- which would represent `a + b` on two `Vec2` values as machine addition.
        if let Some(dispatch) = self.type_info.resolved_operator_call(span)
            && dispatch.kind == ResolvedOperatorKind::Binary
        {
            let method = dispatch.method.clone();
            return self.lower_operator_dispatch(&method, lhs, rhs, result_ty, scope, hir_span_value, out);
        }

        if !Self::binary_op_is_supported(op, &lhs_ty, &rhs_ty) {
            return self.unsupported_operand(format!("binary operator {op:?}"), scope, hir_span_value, out);
        }
        let lhs_operand = self.lower_expr_to_operand(lhs, scope, out);
        let rhs_operand = self.lower_expr_to_operand(rhs, scope, out);
        self.lower_binary_from_operands(
            op,
            &lhs_ty,
            lhs_operand,
            &rhs_ty,
            rhs_operand,
            result_ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Whether `op` between operands of `lhs_ty`/`rhs_ty` has any Body IR v0 handling (either the string-helper
    /// path or a direct [`bir::BinOp`] mapping). Checked *before* evaluating operand sub-expressions in both
    /// [`Self::lower_binary`] and [`Self::lower_compound_assignment`], so an operator v0 does not model never
    /// causes its operands' side effects (calls, reads) to be lowered on the way to an unsupported placeholder.
    /// Lower a user-defined operator to the dunder method call the typechecker resolved for it.
    ///
    /// RFC 028 lets a type define `__add__`, `__and__`, `__contains__` and friends, and the typechecker records
    /// which method one operator spelling dispatches to. Body IR must follow that decision: representing `a + b` on
    /// two `Vec2` values as [`bir::BinOp::Add`] would claim a primitive machine operation where the source calls a
    /// method, which is a wrong representation rather than an honest refusal — no `Unsupported` marker, nothing for
    /// a consumer to notice.
    ///
    /// The left operand becomes the receiver and the right becomes the single argument, matching how
    /// [`Self::lower_method_call`] arranges an ordinary method call: `args[0]` is the receiver, borrowed. The
    /// binding is [`bir::ArgumentBinding::UnresolvedPositional`] because an operator spelling names no parameter
    /// and this stage resolves no declared slot for it.
    #[allow(clippy::too_many_arguments)]
    fn lower_operator_dispatch(
        &mut self,
        method: &str,
        lhs: &ast::Spanned<ast::Expr>,
        rhs: &ast::Spanned<ast::Expr>,
        result_ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        // Source evaluation observes the receiver before the argument, exactly as for a written method call.
        let receiver_place = self.lower_expr_to_place(lhs, scope, out);
        let receiver = bir::Operand::place(receiver_place, bir::OwnershipFact::Borrow, false);
        let argument = self.lower_expr_to_operand(rhs, scope, out);
        self.push_call_temp(
            bir::Callee::Method(bir::MethodTarget::synthesized(method)),
            vec![bir::ArgumentElement::One(receiver), bir::ArgumentElement::One(argument)],
            result_ty,
            scope,
            span,
            false,
            out,
        )
    }

    fn binary_op_is_supported(op: ast::BinaryOp, lhs_ty: &IncanType, rhs_ty: &IncanType) -> bool {
        (is_string_like(lhs_ty) && is_string_like(rhs_ty) && string_helper_for_binop(op).is_some())
            || lower_binary_op(op).is_some()
    }

    /// Emit the result of a binary operator given already-lowered operands: an explicit [`bir::Callee::Helper`]
    /// call (with runtime requirements recorded) when both operand types are string-like and `op` has a
    /// compiler-owned string helper (see [`string_helper_for_binop`]) -- Body IR's compiler-owned-runtime-operation
    /// requirement (#653 criterion 3) applied to string operators specifically -- otherwise a plain
    /// [`bir::Rvalue::BinaryOp`], with a division/modulo panic fact recorded when [`bir::BinOp::may_panic`] holds.
    /// Callers are expected to have already checked [`Self::binary_op_is_supported`]; an operator with neither
    /// handling still falls back to an explicit unsupported placeholder defensively rather than panicking.
    #[allow(clippy::too_many_arguments)]
    fn lower_binary_from_operands(
        &mut self,
        op: ast::BinaryOp,
        lhs_ty: &IncanType,
        lhs_operand: bir::Operand,
        rhs_ty: &IncanType,
        rhs_operand: bir::Operand,
        result_ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        if is_string_like(lhs_ty)
            && is_string_like(rhs_ty)
            && let Some(helper) = string_helper_for_binop(op)
        {
            self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().to_string()));
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
            return self.push_call_temp(
                bir::Callee::Helper(helper),
                fixed_elements(vec![lhs_operand, rhs_operand]),
                result_ty,
                scope,
                span,
                false,
                out,
            );
        }

        let Some(bin_op) = lower_binary_op(op) else {
            return self.unsupported_operand(format!("binary operator {op:?}"), scope, span, out);
        };
        if bin_op.may_panic() {
            self.panic_facts.push(bir::PanicFact {
                span,
                reason: bir::PanicReason::DivisionOrModulo,
            });
            self.record_runtime_requirement(AbiV0RuntimeRequirement::PanicStrategy);
        }
        self.push_assign_temp(
            bir::Rvalue::BinaryOp(bin_op, lhs_operand, rhs_operand),
            result_ty,
            scope,
            span,
            out,
        )
    }

    /// Lower planned call arguments in written source order, then place them into declaration-slot order.
    ///
    /// Both orders are part of the source contract and they differ whenever a caller writes named arguments out of
    /// declaration order. Argument expressions are therefore lowered here strictly left to right, so the emitted
    /// statement sequence observes written evaluation order, while the returned operand vector is in declaration
    /// order and the returned [`bir::ArgumentBinding`] records which slot each operand fills and where it was
    /// written. A declaration slot the call site never supplied becomes a defaulted slot rather than an operand:
    /// this call site evaluates nothing for it, so it has no ownership fact to record and the default's computation
    /// stays owned by the declaration.
    ///
    /// Because ownership is decided during that written-order pass, each operand's [`bir::OwnershipFact`] and
    /// last-use marker are sequenced by `written_position` and **not** by operand index -- see
    /// [`bir::ArgumentBinding`]'s own docs, which state the invariant a consumer has to honor.
    fn lower_planned_args(
        &mut self,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
        slot_count: usize,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::Operand>, bir::ArgumentBinding), String> {
        // Both planners derive their slots from the same declaration surface they report the count of, so an
        // out-of-range slot is unreachable. It is still refused rather than skipped: dropping the operand while
        // leaving the statements that computed it in `out` would produce a silently wrong call, which is the worst
        // failure mode this node has.
        if let Some((slot, _)) = planned.iter().find(|(slot, _)| *slot >= slot_count) {
            return Err(format!(
                "argument bound to declaration slot {slot} outside the callee's {slot_count} declared slots"
            ));
        }
        let mut lowered: Vec<Option<(bir::Operand, usize)>> = (0..slot_count).map(|_| None).collect();
        for (written_position, (slot, expr)) in planned.iter().enumerate() {
            let operand = self.lower_expr_to_operand(expr, scope, out);
            if let Some(entry) = lowered.get_mut(*slot) {
                *entry = Some((operand, written_position));
            }
        }

        let mut operands = Vec::with_capacity(planned.len());
        let mut arguments = Vec::with_capacity(planned.len());
        let mut defaulted_slots = Vec::new();
        for (slot, entry) in lowered.into_iter().enumerate() {
            match entry {
                Some((operand, written_position)) => {
                    operands.push(operand);
                    arguments.push(bir::BoundArgument { slot, written_position });
                }
                None => defaulted_slots.push(slot),
            }
        }
        Ok((
            operands,
            bir::ArgumentBinding::Resolved {
                arguments,
                defaulted_slots,
            },
        ))
    }

    /// Resolve the declaration surface and exact local identity for a direct named call.
    ///
    /// A direct executable target must be physically represented by this Body-IR module. Imports and unresolved
    /// names deliberately retain their existing call representation with no direct declaration identity, so this
    /// frontend does not turn a source-representation gap into a new source diagnostic. The replacement executor
    /// then refuses those targets at the original call span; only compiler-recognized `range` has a separate
    /// explicit Body-IR builtin target fact.
    ///
    /// Overloads are why this is resolved per call site rather than per name. `function_bindings` is keyed by bare
    /// source name, so for two same-name declarations it holds only one of them; binding a call against the wrong
    /// overload's parameter *names* would silently reorder its arguments, turning an honest refusal into a wrong
    /// answer. The typechecker already records which overload it selected for this call span, so this follows that
    /// decision to the declaration and reads that declaration's own signature. If a name is overloaded but no
    /// selection was recorded, this fails closed rather than picking one.
    fn declared_slots_for_direct_call(&self, name: &str, span: ast::Span) -> Result<DirectCallDeclaration, String> {
        let declarations = &self.type_info.declarations;
        let local_declarations = self.local_function_declarations.get(name);
        let Some(local_declarations) = local_declarations else {
            return Ok(DirectCallDeclaration {
                slots: declarations
                    .function_bindings
                    .get(name)
                    .map(|binding| binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
                direct_call_id: None,
                builtin: (name == "range"
                    && self.type_info.source_target(span).is_none()
                    && !declarations.function_bindings.contains_key(name)
                    && !declarations.function_overloads.contains_key(name))
                .then_some(bir::NamedCallableBuiltin::Range),
            });
        };
        let is_overloaded = local_declarations.len() > 1;

        if is_overloaded {
            let Some(selected) = self.type_info.selected_function_emitted_name(span) else {
                return Err(format!(
                    "call to overloaded function `{name}` whose selected declaration was not resolved"
                ));
            };
            let selected_span = local_declarations.iter().find(|candidate_span| {
                declarations
                    .function_emitted_names
                    .get(&(candidate_span.start, candidate_span.end))
                    .is_some_and(|emitted| emitted == selected)
            });
            let Some(selected_span) = selected_span else {
                return Err(format!(
                    "call to overloaded function `{name}` whose selected declaration could not be located"
                ));
            };
            let Some(binding) = declarations
                .function_bindings_by_span
                .get(&(selected_span.start, selected_span.end))
            else {
                return Err(format!(
                    "call to overloaded function `{name}` whose selected declaration has no checked signature"
                ));
            };
            return Ok(DirectCallDeclaration {
                slots: Some(binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
                direct_call_id: Some(CompilerNodeId::declaration_span(
                    self.module_identity,
                    selected_span.start,
                    selected_span.end,
                )),
                builtin: None,
            });
        }

        let [declaration_span] = local_declarations.as_slice() else {
            return Err(format!(
                "direct call to `{name}` has no unambiguous same-module declaration identity"
            ));
        };
        let Some(binding) = declarations
            .function_bindings_by_span
            .get(&(declaration_span.start, declaration_span.end))
        else {
            return Err(format!(
                "same-module declaration `{name}` has no checked callable signature"
            ));
        };
        Ok(DirectCallDeclaration {
            slots: Some(binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
            direct_call_id: Some(CompilerNodeId::declaration_span(
                self.module_identity,
                declaration_span.start,
                declaration_span.end,
            )),
            builtin: None,
        })
    }

    /// Bind a call's arguments against a declared parameter surface, falling back to positional lowering when there
    /// is none to bind against.
    ///
    /// Shared by the direct-call and method paths so both treat an unresolved or rest-bearing signature the same
    /// way. A rest (`*args`/`**kwargs`) parameter means a written argument no longer corresponds one-to-one with a
    /// declared slot, so those calls keep lowering their arguments — refusing them would drop a delivered language
    /// capability — but record [`bir::ArgumentBinding::UnresolvedPositional`] rather than a slot map this stage did
    /// not compute. Spread arguments lower there, because a spread genuinely has no slot to bind to. A *named*
    /// argument with no spread beside it is still refused: its arity is perfectly well known, so binding it into a
    /// rest parameter is variadic-binding work this issue does not own.
    fn bind_declared_args(
        &mut self,
        callee: &str,
        declared: Option<Vec<DeclaredSlot>>,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        let has_rest = declared
            .as_ref()
            .is_some_and(|slots| slots.iter().any(|slot| slot.is_rest));
        let fixed_slots = declared.filter(|_| !has_rest);
        let Some(slots) = fixed_slots else {
            let elements = self.lower_spread_capable_args(args, scope, out);
            return Ok((elements, bir::ArgumentBinding::UnresolvedPositional));
        };
        // A spread whose shape the typechecker proved is an ordinary fixed-arity call in disguise: `add(*(1, 2))`
        // really is `add(1, 2)`. Expanding it here means it binds through the same declaration-slot planner as any
        // other call, instead of being pushed onto the runtime-arity path it does not belong on.
        let expanded: Vec<ast::CallArg> = args
            .iter()
            .flat_map(|arg| match expand_shaped_spread(self.type_info, arg) {
                Some(expansion) => expansion,
                None => vec![arg.clone()],
            })
            .collect();
        let planned = plan_declared_args(callee, &slots, &expanded)?;
        let (operands, binding) = self
            .lower_planned_args(&planned, slots.len(), scope, out)
            .map_err(|description| format!("{callee}: {description}"))?;
        Ok((fixed_elements(operands), binding))
    }

    /// Resolve a call site's explicit type arguments to semantic types, or describe why they cannot be represented.
    ///
    /// Explicit type arguments are part of a call's resolved identity, so Body IR takes the typechecker's
    /// monomorphized selection rather than re-lowering the written AST type nodes -- which is also the only way a
    /// `_` placeholder resolves to a real type instead of an unknown. A call that wrote type arguments the
    /// typechecker did not resolve is refused by name rather than represented with a guess.
    fn call_site_type_arguments(
        &self,
        span: ast::Span,
        type_args: &[ast::Spanned<ast::Type>],
    ) -> Result<Vec<IncanType>, String> {
        if type_args.is_empty() {
            return Ok(Vec::new());
        }
        let Some(resolved) = self
            .type_info
            .calls
            .call_site_monomorph_type_args
            .get(&(span.start, span.end))
        else {
            return Err("call with unresolved explicit type arguments".to_string());
        };
        Ok(resolved.iter().map(semantic_type_from_resolved).collect())
    }

    /// Lower a `model`/`class` construction into a [`bir::AggregateKind::Constructor`] aggregate.
    ///
    /// Source-level construction is named-only, so the argument-to-field binding is the whole representation
    /// problem. Lowering consumes the typechecker's own recorded decision
    /// ([`TypeCheckInfo::constructor_field_binding`](crate::frontend::typechecker::TypeCheckInfo::constructor_field_binding))
    /// rather than re-resolving field aliases or rediscovering declared field order, both of which live in the
    /// symbol table this stage deliberately cannot reach. Operands are emitted in declared field order while the
    /// argument expressions are lowered in written source order, exactly as for a call.
    fn lower_nominal_construction(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let Some(field_binding) = self.type_info.constructor_field_binding(span).cloned() else {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        };

        // The typechecker records one slot per *written* argument, so a spread -- which supplies an unknown number
        // of fields -- can never appear in a recorded binding. Refuse it by name; #1159 owns spread representation.
        let mut written_exprs = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) | ast::CallArg::Named(_, expr) => written_exprs.push(expr),
                ast::CallArg::PositionalUnpack(_) => {
                    return self.unsupported_operand(
                        format!("construction of `{name}` with a positional argument spread"),
                        scope,
                        hir_span_value,
                        out,
                    );
                }
                ast::CallArg::KeywordUnpack(_) => {
                    return self.unsupported_operand(
                        format!("construction of `{name}` with a keyword argument spread"),
                        scope,
                        hir_span_value,
                        out,
                    );
                }
            }
        }
        if written_exprs.len() != field_binding.argument_slots.len() {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        }

        let planned: Vec<(usize, &ast::Spanned<ast::Expr>)> = field_binding
            .argument_slots
            .iter()
            .copied()
            .zip(written_exprs)
            .collect();
        let (operands, binding) = match self.lower_planned_args(&planned, field_binding.field_count, scope, out) {
            Ok(bound) => bound,
            Err(description) => {
                return self.unsupported_operand(
                    format!("construction of `{name}`: {description}"),
                    scope,
                    hir_span_value,
                    out,
                );
            }
        };
        let ty = self.resolve_ty(span);
        // A constructor field binding proves argument slots, but not that this constructor names one of the plain
        // source-local models this Body-IR module retained. Preserve an identity only from that local registry;
        // imports, aliases, classes, generic models, and absent/malformed names remain represented with `None` so
        // a direct executor can refuse at this construction span rather than guessing from `name`.
        let direct_declaration_id = self.local_nominal_declarations.get(name).and_then(|declaration| {
            (declaration.fields.len() == field_binding.field_count).then(|| declaration.direct_declaration_id.clone())
        });
        self.push_assign_temp(
            bir::Rvalue::Aggregate(
                bir::AggregateKind::Constructor(bir::ConstructorTarget {
                    name: name.to_string(),
                    direct_declaration_id,
                    binding,
                }),
                fixed_elements(operands),
            ),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Lower call arguments positionally, admitting spreads, for a call whose arity is not statically known.
    ///
    /// Every argument keeps its written form: a positional value, a named value, or a spread. None of them can be
    /// resolved to a declared slot here, because a spread supplies an unknown number of arguments at runtime —
    /// which is exactly why the resulting call records [`bir::ArgumentBinding::UnresolvedPositional`] rather than a
    /// slot map asserting a binding nobody checked. A name is preserved on its element rather than discarded, so a
    /// later consumer can still bind it once the arity is known.
    fn lower_spread_capable_args(
        &mut self,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Vec<bir::ArgumentElement> {
        let mut elements = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) => {
                    elements.push(bir::ArgumentElement::One(self.lower_expr_to_operand(expr, scope, out)));
                }
                ast::CallArg::PositionalUnpack(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Sequence, scope, out));
                }
                ast::CallArg::KeywordUnpack(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Mapping, scope, out));
                }
                ast::CallArg::Named(name, expr) => {
                    let operand = self.lower_expr_to_operand(expr, scope, out);
                    elements.push(bir::ArgumentElement::Named {
                        name: name.clone(),
                        operand,
                    });
                }
            }
        }
        elements
    }

    /// Return the exact retained target for a qualified local fieldless normal-enum member, if safe to materialize.
    ///
    /// A bare type-name receiver and source-local registry membership are both required. This leaves ordinary forms
    /// not represented by the registry as generic field accesses that the direct executor visibly refuses, while
    /// preserving exact declaration identities for the one bounded unit-variant carrier profile.
    fn local_fieldless_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        variant_name: &str,
    ) -> Option<bir::FieldlessEnumVariantTarget> {
        let ast::Expr::Ident(enum_name) = &base.node else {
            return None;
        };
        if self.bindings.contains_key(enum_name)
            || !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName))
        {
            return None;
        }
        let declaration = self.local_fieldless_enum_declarations.get(enum_name)?;
        let variant = declaration
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)?;
        Some(bir::FieldlessEnumVariantTarget {
            enum_declaration_id: declaration.direct_declaration_id.clone(),
            variant_declaration_id: variant.direct_declaration_id.clone(),
            enum_name: declaration.name.clone(),
            variant_name: variant.name.clone(),
        })
    }

    /// Return the exact retained target for a qualified local RFC 032 value-enum member, if this spelling is safe to
    /// materialize directly.
    ///
    /// The source-local registry is deliberately the only lookup used here. A function-local binding wins over a
    /// same-spelling declaration, and any import, alias, ordinary enum, payload member, or behavior-bearing enum is
    /// absent from the registry. The resulting rvalue stores both declaration identities for runtime revalidation;
    /// it does not make the spelling itself an execution authority.
    fn local_value_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        variant_name: &str,
    ) -> Option<bir::ValueEnumVariantTarget> {
        let ast::Expr::Ident(enum_name) = &base.node else {
            return None;
        };
        if self.bindings.contains_key(enum_name) {
            return None;
        }
        if !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName)) {
            return None;
        }
        let declaration = self.local_value_enum_declarations.get(enum_name)?;
        let variant = declaration
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)?;
        Some(bir::ValueEnumVariantTarget {
            enum_declaration_id: declaration.direct_declaration_id.clone(),
            variant_declaration_id: variant.direct_declaration_id.clone(),
            enum_name: declaration.name.clone(),
            variant_name: variant.name.clone(),
        })
    }

    /// Lower a call to a locally held callable value, a nominal construction, or a direct named function.
    ///
    /// A bare identifier that resolves to one of this body's locals is deliberately a
    /// [`bir::CallableTarget::Local`] call: it carries the local read's ownership fact, so a closure's lexical
    /// environment is not lost by pretending the identifier were a declaration. Its callable signature also
    /// enforces the stored value's fixed callable contract before any call arguments are lowered. An identifier the
    /// typechecker resolved to a `model`/`class` construction lowers to a constructor aggregate instead of a call
    /// (see [`Self::lower_nominal_construction`]) -- construction is not invocation, and representing it as a call
    /// would invite a consumer to execute it as one. Any other bare identifier remains a direct
    /// [`bir::Callee::Function`] call.
    ///
    /// Every one of those paths binds its arguments through the same [`plan_declared_args`] planner and records the
    /// result as a [`bir::ArgumentBinding`], so named, out-of-order, and defaulted spellings resolve identically
    /// regardless of how the callee was reached. A direct call whose signature the typechecker did not resolve
    /// (notably a builtin) still lowers its arguments faithfully, recording
    /// [`bir::ArgumentBinding::UnresolvedPositional`], and refuses only a named spelling it cannot bind without one.
    /// Argument spreads lower as [`bir::ArgumentElement::Spread`] elements, since a spread has no declared slot to
    /// bind to by construction. A non-identifier callee remains an explicit unsupported form; v0 has no
    /// dynamic-call-target node for it yet.
    fn lower_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ast::Expr::Ident(name) = &callee.node else {
            return self.unsupported_operand("indirect call target".to_string(), scope, hir_span_value, out);
        };
        let name = name.clone();

        // A recorded constructor field binding is the typechecker's own statement that this spelling constructs a
        // nominal value, which is what distinguishes `P(x=1)` from a call to a function that happens to be named
        // `P`. A construction may carry call-site type arguments (`Box[int]()` is accepted), but the typechecker
        // records no monomorphization for them, and the constructed value's own type already carries the resolved
        // arguments -- so this deliberately does not duplicate them on the constructor target rather than claiming
        // construction cannot be generic.
        if self.type_info.constructor_field_binding(span).is_some() {
            return self.lower_nominal_construction(&name, args, span, scope, out);
        }

        // `Ok` and `Err` are intrinsic Result constructors, not ordinary direct calls. Retain that checked
        // distinction explicitly: a same-spelled source binding (a local callable, local function, or imported
        // target) must remain on the normal call path and refuse unless its own direct callable facts are available.
        // The direct runtime never resolves a constructor name dynamically.
        if !self.bindings.contains_key(&name)
            && !self.local_function_declarations.contains_key(&name)
            && self.type_info.source_target(span).is_none()
            && type_args.is_empty()
            && let Some(kind) = result_variant_kind(&name)
        {
            let result_ty = self.resolve_ty(span);
            let Some((ok_type, error_type)) = result_type_parts(&result_ty) else {
                return self.unsupported_operand(
                    format!("intrinsic Result constructor `{name}` without a resolved Result carrier"),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let [ast::CallArg::Positional(payload)] = args else {
                return self.unsupported_operand(
                    format!("intrinsic Result constructor `{name}` requires one positional payload"),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let payload = self.lower_expr_to_operand(payload, scope, out);
            return self.push_assign_temp(
                bir::Rvalue::ResultVariant(bir::ResultVariant {
                    kind,
                    payload,
                    ok_type: ok_type.clone(),
                    error_type: error_type.clone(),
                }),
                result_ty,
                scope,
                hir_span_value,
                out,
            );
        }

        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };

        if let Some(&local) = self.bindings.get(&name) {
            let local_ty = self.locals[local.index()].ty.clone();
            let IncanType::Function { params, return_type: _ } = local_ty else {
                return self.unsupported_operand(
                    format!("call to non-callable local `{name}`"),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let slots: Vec<DeclaredSlot> = params.iter().map(DeclaredSlot::from_semantic_param).collect();
            let planned = match plan_declared_args(&format!("local callable `{name}`"), &slots, args) {
                Ok(planned) => planned,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };

            // Source evaluation observes the callable value before its arguments. The target read also performs the
            // one ownership/last-use decision for that lexical environment, which `CallableTarget::Local` preserves
            // for a later executor instead of re-deriving it from the local's source spelling.
            let place = bir::Place::from_local(local);
            let (fact, last_use) = self.ownership_fact_for_place(&place, &self.locals[local.index()].ty.clone());
            let (operands, binding) = match self.lower_planned_args(&planned, slots.len(), scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };
            let callee = bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
                operand: bir::PlaceOperand { place, fact, last_use },
                binding,
            }));
            let ty = self.resolve_ty(span);
            return self.push_call_temp(callee, fixed_elements(operands), ty, scope, hir_span_value, false, out);
        }

        // A name that resolves to a nominal type but has no recorded field binding is a construction the checker
        // declined to bind (a duplicate or unknown field). Refusing it as a call to an unknown function would name
        // the wrong construct entirely.
        if self.type_info.declarations.class_layouts.contains_key(&name)
            || self.type_info.declarations.model_field_visibilities.contains_key(&name)
        {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        }

        let declaration = match self.declared_slots_for_direct_call(&name, span) {
            Ok(declaration) => declaration,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };
        let (operands, binding) =
            match self.bind_declared_args(&format!("function `{name}`"), declaration.slots, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };

        let ty = self.resolve_ty(span);
        self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                name,
                direct_call_id: declaration.direct_call_id,
                builtin: declaration.builtin,
                type_args: resolved_type_args,
                binding,
            })),
            operands,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Return the typechecker's callable type for a closure or local partial value that Body IR constructs itself.
    ///
    /// Local partials use the typechecker's canonical full signature with overrideable preset-default slots, so the
    /// binding, its [`bir::Rvalue::Closure`], and a later [`Self::lower_call`] share one arity/default contract.
    fn callable_value_ty(&self, expr: &ast::Spanned<ast::Expr>) -> Option<IncanType> {
        match &expr.node {
            ast::Expr::Closure(_, _) | ast::Expr::Partial(_) => Some(self.resolve_ty(expr.span)),
            _ => None,
        }
    }

    /// Lower a method call `recv.name(args)` to a [`bir::Callee::Method`] call, with the receiver prepended to
    /// `args[0]` as a [`bir::OwnershipFact::Borrow`] operand (see the inline comment on the receiver-borrow decision
    /// below).
    ///
    /// Argument binding goes through the same [`plan_declared_args`] planner every other call shape uses, against
    /// the typechecker's own rest-aware call-site signature for this span -- which already has the receiver's
    /// generic arguments substituted, so a generic method's slots are concrete here. The receiver is deliberately
    /// outside the recorded binding: its slots index the method's declared parameters, so a consumer reads
    /// `args[0]` as the receiver and `args[1..]` as the bound arguments. A method call whose signature the
    /// typechecker did not record still lowers positional arguments faithfully and refuses only the spellings it
    /// cannot bind — a named spelling with no spread beside it — matching [`Self::lower_call`]'s treatment of an
    /// unresolved direct callee. Spread arguments lower here too, after the receiver.
    #[allow(clippy::too_many_arguments)]
    fn lower_method_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(_) => {
                return self.unsupported_operand(
                    "method call with unresolved explicit type arguments".to_string(),
                    scope,
                    hir_span_value,
                    out,
                );
            }
        };

        let declared: Option<Vec<DeclaredSlot>> = self
            .type_info
            .call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());

        // The receiver is read before the arguments, matching source evaluation order: `recv.m(f())` observes the
        // receiver place first. Method receivers are treated as borrowed rather than moved/cloned, mirroring how the
        // existing Rust-emission backend's ownership planner treats most method receivers
        // (`src/backend/ir/ownership.rs`) -- see this module's rustdoc for the full precedent discussion.
        let receiver_operand = if let ast::Expr::Field(base, member) = &recv.node
            && self.local_value_enum_variant_target(base, member).is_some()
        {
            self.lower_expr_to_operand(recv, scope, out)
        } else {
            let recv_place = self.lower_expr_to_place(recv, scope, out);
            bir::Operand::place(recv_place, bir::OwnershipFact::Borrow, false)
        };

        let (mut arg_operands, binding) =
            match self.bind_declared_args(&format!("method `{name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };

        // The receiver is `args[0]` and is never spliced, so it is always a single-value element.
        let mut call_args = Vec::with_capacity(arg_operands.len() + 1);
        call_args.push(bir::ArgumentElement::One(receiver_operand));
        call_args.append(&mut arg_operands);
        let ty = self.resolve_ty(span);
        self.push_call_temp(
            bir::Callee::Method(bir::MethodTarget {
                name: name.to_string(),
                type_args: resolved_type_args,
                binding,
            }),
            call_args,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Lower a list literal, including spread entries, into a [`bir::AggregateKind::List`] aggregate.
    ///
    /// Elements are lowered in written source order, so a spread source's evaluation is interleaved with the fixed
    /// elements around it exactly as written. A spread contributes one [`bir::ArgumentElement::Spread`] whose
    /// length is a runtime fact; surrounding fixed elements keep their positions relative to it.
    fn lower_list_literal(
        &mut self,
        entries: &[ast::ListEntry],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let mut elements = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                ast::ListEntry::Element(item) => {
                    elements.push(bir::ArgumentElement::One(self.lower_expr_to_operand(item, scope, out)));
                }
                ast::ListEntry::Spread(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Sequence, scope, out));
                }
            }
        }
        let ty = self.resolve_ty(span);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.push_assign_temp(
            bir::Rvalue::Aggregate(bir::AggregateKind::List, elements),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Lower one spread source into a [`bir::ArgumentElement::Spread`].
    ///
    /// The source is read through the ordinary ownership path, so a spliced source carries the same
    /// [`bir::OwnershipFact`]/last-use discipline as any other read. That fact is recorded on the spread itself
    /// rather than inferred from the surrounding aggregate or call, because a spliced source is consumed
    /// differently from a single element: its contents are distributed into the surrounding list.
    fn lower_spread_element(
        &mut self,
        source: &ast::Spanned<ast::Expr>,
        kind: bir::SpreadKind,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::ArgumentElement {
        let operand = self.lower_expr_to_operand(source, scope, out);
        bir::ArgumentElement::Spread(bir::SpreadElement { source: operand, kind })
    }

    /// Lower a tuple or set literal to a [`bir::Rvalue::Aggregate`], recording an
    /// [`AbiV0RuntimeRequirement::Allocator`] requirement for lists and sets specifically (list/set construction
    /// always allocates; tuples do not).
    fn lower_aggregate(
        &mut self,
        kind: bir::AggregateKind,
        items: &[ast::Spanned<ast::Expr>],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let operands: Vec<bir::Operand> = items
            .iter()
            .map(|item| self.lower_expr_to_operand(item, scope, out))
            .collect();
        let ty = self.resolve_ty(span);
        if matches!(kind, bir::AggregateKind::List | bir::AggregateKind::Set) {
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        }
        self.push_assign_temp(
            bir::Rvalue::Aggregate(kind, fixed_elements(operands)),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Lower a dict literal `{k: v, ...}` to a [`bir::Rvalue::Dict`], one entry per source entry, in written order.
    ///
    /// Keys and values are lowered in written order, key before value, because both are arbitrary expressions
    /// whose evaluation order is source-observable. A `**source` spread contributes one
    /// [`bir::DictEntry::Spread`] in written position; entries take effect in order and a later entry overwrites an
    /// earlier one with the same key, which is what makes `{**base, "x": 1}` well defined.
    fn lower_dict(
        &mut self,
        entries: &[ast::DictEntry],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let mut lowered = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                ast::DictEntry::Pair(key, value) => {
                    let key_operand = self.lower_expr_to_operand(key, scope, out);
                    let value_operand = self.lower_expr_to_operand(value, scope, out);
                    lowered.push(bir::DictEntry::Pair(key_operand, value_operand));
                }
                ast::DictEntry::Spread(source) => {
                    // Reuse the shared spread lowering so the two construction sites cannot drift.
                    let bir::ArgumentElement::Spread(spread) =
                        self.lower_spread_element(source, bir::SpreadKind::Mapping, scope, out)
                    else {
                        return self.unsupported_operand("dict spread entry".to_string(), scope, hir_span_value, out);
                    };
                    lowered.push(bir::DictEntry::Spread(spread));
                }
            }
        }
        let ty = self.resolve_ty(span);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.push_assign_temp(bir::Rvalue::Dict(lowered), ty, scope, hir_span_value, out)
    }

    /// Lower an f-string `f"...{expr}...{expr!r}..."` to a [`bir::Rvalue::Format`]. Literal text chunks are
    /// carried through verbatim; each embedded expression is lowered through the same
    /// [`Self::lower_expr_to_operand`] path as any other read, so ownership facts and last-use tracking apply to
    /// f-string interpolations exactly like any other expression use. Mirrors the existing Rust-emission backend's
    /// dedicated `Format` node (`src/backend/ir/lower/expr/mod.rs`) rather than desugaring into a helper call --
    /// see [`bir::Rvalue::Format`]'s own docs for why this needed its own `Rvalue` shape.
    ///
    /// Building the formatted string always allocates and always needs the `fstring` runtime helper
    /// (`incan_stdlib::strings::fstring`, the function the existing Rust-emission backend's `Format` node itself
    /// compiles down to -- see `src/backend/ir/emit/expressions/format.rs`), so both requirements are recorded
    /// unconditionally here, the same way [`Self::lower_binary_from_operands`] records requirements for its own
    /// compiler-owned string helpers.
    fn lower_fstring(
        &mut self,
        parts: &[ast::FStringPart],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ir_parts: Vec<bir::FormatPart> = parts
            .iter()
            .map(|part| match part {
                ast::FStringPart::Literal(s) => bir::FormatPart::Literal(s.clone()),
                ast::FStringPart::Expr { expr, format } => {
                    let operand = self.lower_expr_to_operand(expr, scope, out);
                    let style = match format {
                        ast::FStringFormat::Display => bir::FormatStyle::Display,
                        ast::FStringFormat::Debug => bir::FormatStyle::Debug,
                    };
                    bir::FormatPart::Expr { operand, style }
                }
            })
            .collect();
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper("fstring".to_string()));
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        let ty = self.resolve_ty(span);
        self.push_assign_temp(bir::Rvalue::Format(ir_parts), ty, scope, hir_span_value, out)
    }

    /// Lower `base[start:end:step]` (each component independently optional) into a value read through a
    /// [`bir::PlaceElem::Slice`] projection, mirroring how `Expr::Index` builds an `[index]`-projected place read
    /// in [`Self::lower_expr_to_operand`] (including that same arm's index-before-base evaluation order, extended
    /// here to start-then-end-then-step-then-base).
    fn lower_slice(
        &mut self,
        base: &ast::Spanned<ast::Expr>,
        slice: &ast::SliceExpr,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let start = slice
            .start
            .as_ref()
            .map(|e| Box::new(self.lower_expr_to_operand(e, scope, out)));
        let end = slice
            .end
            .as_ref()
            .map(|e| Box::new(self.lower_expr_to_operand(e, scope, out)));
        let step = slice
            .step
            .as_ref()
            .map(|e| Box::new(self.lower_expr_to_operand(e, scope, out)));
        let mut place = self.lower_expr_to_place(base, scope, out);
        place.projection.push(bir::PlaceElem::Slice { start, end, step });
        let ty = self.resolve_ty(span);
        let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
        bir::Operand::place(place, fact, last_use)
    }

    /// Lower `expr?` (`ast::Expr::Try`) into a single [`bir::StatementKind::TryPropagate`] primitive rather than
    /// decomposing it into explicit `is_err`/`unwrap`-shaped calls -- see that variant's own docs for the full
    /// rationale (it mirrors the same #653-criterion-3 compiler-owned-primitive treatment as
    /// [`bir::Callee::Helper`], standing in for what the existing Rust-emission backend defers entirely to Rust's
    /// native `?` operator).
    fn lower_try(
        &mut self,
        inner: &ast::Spanned<ast::Expr>,
        outer_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(outer_span);
        let operand_result_type = self.resolve_ty(inner.span);
        let error_routing = match (
            result_error_type(&operand_result_type),
            result_error_type(&self.owner_return_type),
        ) {
            (Some(source_error_type), Some(destination_error_type)) if source_error_type == destination_error_type => {
                bir::TryErrorRouting::SameType {
                    error_type: source_error_type.clone(),
                }
            }
            (Some(source_error_type), Some(destination_error_type)) => bir::TryErrorRouting::ConversionRequired {
                source_error_type: source_error_type.clone(),
                destination_error_type: destination_error_type.clone(),
            },
            _ => bir::TryErrorRouting::Unresolved,
        };
        let operand = self.lower_expr_to_operand(inner, scope, out);
        let ty = self.resolve_ty(outer_span);
        let destination = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::TryPropagate {
                destination: bir::Place::from_local(destination),
                operand,
                error_routing,
            },
            span: hir_span_value,
        });
        self.temp_operand(destination, &ty)
    }

    /// Lower an `ast::Expr::Constructor` node by delegating to [`Self::lower_nominal_construction`].
    ///
    /// No stage of the current pipeline produces this AST variant: `P(x=1, y=2)` parses as an
    /// `ast::Expr::Call` whose callee is a bare identifier, and `lower_call` recognises the construction from the
    /// typechecker's recorded field binding. The arm is kept because the variant is still part of the AST contract,
    /// and it delegates rather than duplicating the lowering so a future producer cannot reach a second, divergent
    /// construction path.
    fn lower_constructor(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        self.lower_nominal_construction(name, args, span, scope, out)
    }

    // ---- Async surface (#1164) ----

    /// Lower an `ast::Expr::Surface` node, accepting only the async pair this issue owns.
    ///
    /// Dispatch is on the surface **key**, not the payload shape. `SurfaceExprPayload::PrefixUnary` is generic over
    /// any prefix soft keyword and `await` merely happens to be the only one registered today, so matching the
    /// payload alone would silently accept a future prefix keyword as an await. The typechecker
    /// (`check_expr/mod.rs`) and the existing Rust-emission backend (`backend/ir/lower/expr/mod.rs`) both dispatch
    /// on the key/payload pair for exactly this reason.
    ///
    /// Every other payload -- the scoped-DSL surface nodes -- keeps its existing named refusal. Those reach this
    /// module only when a caller skips the desugar pass the legacy pipeline runs first, and they belong to the Body
    /// IR input-contract issue, not to this one.
    fn lower_surface_expr(
        &mut self,
        surface: &ast::SurfaceExpr,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        match (&surface.key, &surface.payload) {
            (SurfaceFeatureKey::SoftKeyword(KeywordId::Await), ast::SurfaceExprPayload::PrefixUnary(awaited)) => {
                self.lower_await(awaited, span, scope, out)
            }
            (
                SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key,
                    descriptor_key,
                },
                ast::SurfaceExprPayload::RaceFor(race),
            ) if dependency_key == "std.async" && descriptor_key == "race_for" => {
                self.lower_race_for(race, span, scope, out)
            }
            (_, payload) => self.unsupported_operand(surface_expr_label(payload), scope, hir_span(span), out),
        }
    }

    /// Lower `await expr` into a [`bir::StatementKind::Await`] suspension point.
    ///
    /// The awaited operand is read through the ordinary ownership path, so the suspension carries the same
    /// [`bir::OwnershipFact`]/last-use discipline as any other read. The resumed value lands in a fresh temporary,
    /// which is what makes the suspension's destination explicit rather than implied by the surrounding statement.
    ///
    /// Records [`AbiV0RuntimeRequirement::AsyncRuntime`] on the enclosing body so a consumer reads the requirement
    /// off the body it applies to instead of re-deriving it from the program's imports and declaration modifiers.
    fn lower_await(
        &mut self,
        awaited: &ast::Spanned<ast::Expr>,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let operand = self.lower_expr_to_operand(awaited, scope, out);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::AsyncRuntime);
        let ty = self.resolve_ty(span);
        let destination = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Await {
                destination: Some(bir::Place::from_local(destination)),
                awaited: operand,
            },
            span: hir_span_value,
        });
        self.temp_operand(destination, &ty)
    }

    /// Lower `race for value:` into a [`bir::StatementKind::Race`].
    ///
    /// Each arm's awaitable is lowered into the enclosing block *before* any arm body, which is what makes "every
    /// awaitable is evaluated before selection" observable in the statement sequence rather than a claim in prose.
    /// Each arm then gets its own scope and its own binding local: the source spells one shared name, but arms
    /// re-scope it and can resolve it to different types, so one local per arm is the faithful shape.
    ///
    /// An arm body containing an unsupported construct keeps its own `Unsupported` node *inside* the represented
    /// race rather than collapsing the whole expression, so a consumer loses only the construct it cannot handle.
    fn lower_race_for(
        &mut self,
        race: &ast::RaceForExpr,
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);

        // Selection observes every awaitable, so all of them are evaluated first, in source order, into the
        // enclosing block. Only the winning arm's body runs, so arm bodies are lowered into their own blocks below.
        let mut awaitables = Vec::with_capacity(race.arms.len());
        for arm in &race.arms {
            awaitables.push(self.lower_expr_to_operand(&arm.awaitable, scope, out));
        }
        self.record_runtime_requirement(AbiV0RuntimeRequirement::AsyncRuntime);

        let mut arms = Vec::with_capacity(race.arms.len());
        for (arm, awaitable) in race.arms.iter().zip(awaitables) {
            let arm_scope = self.new_scope(Some(scope), hir_span_value);
            // The arm binds the *awaited output* type, which only the typechecker computes: `Awaitable[T]` binds
            // `T`, `JoinHandle[T]` binds `Result[T, TaskJoinError]`. The awaitable's own type would be wrong.
            let binding_ty = self
                .type_info
                .race_arm_binding_type(arm.awaitable.span)
                .map(semantic_type_from_resolved)
                .unwrap_or(IncanType::Unknown);
            // Snapshot the whole binding environment before the arm, not just the shared race binding. A block arm
            // lowers ordinary statements, and every `x = ...` in it declares a local that
            // `declare_new_local_with_reads` installs into `self.bindings`. Restoring only `race.binding`
            // would leave those arm-locals visible to later arms and to code after the race, so a trailing
            // read of a name an arm happened to shadow would silently resolve to the arm's local.
            // `insert_scope_drops` handles the *drop* obligation; it does not touch name resolution, which
            // is what this restores.
            let enclosing_bindings = self.bindings.clone();
            let reads = match &arm.body {
                ast::RaceForBody::Expr(expr) => count_reads_in_expr(&race.binding, &expr.node),
                ast::RaceForBody::Block(stmts) => count_reads_in_stmts(&race.binding, stmts),
            };
            let binding =
                self.declare_new_local_with_reads(race.binding.clone(), binding_ty, arm_scope, hir_span_value, reads);

            let mut arm_stmts = Vec::new();
            let result = match &arm.body {
                ast::RaceForBody::Expr(expr) => self.lower_expr_to_operand(expr, arm_scope, &mut arm_stmts),
                ast::RaceForBody::Block(stmts) => {
                    self.lower_race_arm_block(stmts, arm.awaitable.span, arm_scope, &mut arm_stmts)
                }
            };
            self.insert_scope_drops(&mut arm_stmts, arm_scope);

            // Every name an arm bound -- its winner binding and any local its block body declared -- is scoped to
            // that arm, exactly like a closure body's. Code after the race, and each later arm, must keep resolving
            // every name to whatever it meant outside.
            self.bindings = enclosing_bindings;

            arms.push(bir::RaceArm {
                awaitable,
                binding,
                body: bir::Block {
                    scope: arm_scope,
                    stmts: arm_stmts,
                },
                result,
            });
        }

        let ty = self.resolve_ty(span);
        let destination = self.new_temp(ty.clone(), scope, hir_span_value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Race {
                destination: Some(bir::Place::from_local(destination)),
                arms,
            },
            span: hir_span_value,
        });
        self.temp_operand(destination, &ty)
    }

    /// Lower a race arm's block body, whose value is its trailing expression statement.
    ///
    /// That trailing-expression convention is the source contract the typechecker already applies, so lowering
    /// matches `check_race_arm_block_body` exactly, including its two non-expression cases: an empty block and a
    /// block whose last statement is not an expression both produce `Unit`, the same type the checker assigns them.
    /// Refusing either would make a program the source language accepts unrepresentable, and the established
    /// precedent for a valueless block arm is [`Self::lower_match`]'s own block body.
    fn lower_race_arm_block(
        &mut self,
        stmts: &[ast::Spanned<ast::Statement>],
        arm_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let Some((last, leading)) = stmts.split_last() else {
            return bir::Operand::Constant(bir::Constant::Unit);
        };
        for (index, stmt) in leading.iter().enumerate() {
            self.lower_stmt_into(stmt, &stmts[index + 1..], scope, out);
        }
        match &last.node {
            ast::Statement::Expr(expr) => self.lower_expr_to_operand(expr, scope, out),
            _ => {
                let _ = arm_span;
                self.lower_stmt_into(last, &[], scope, out);
                bir::Operand::Constant(bir::Constant::Unit)
            }
        }
    }

    // ---- Closures and partial callables (#1101 bucket B4) ----

    /// Lower a closure literal `(params) => expr` into a [`bir::Rvalue::Closure`].
    ///
    /// Body IR must represent captures explicitly rather than deferring to a consuming backend's own closure syntax
    /// to auto-capture (see this module's docs and #1101's B4 pre-intake), so this: (1) statically determines every
    /// free variable the closure body reads via [`free_vars_in_closure_body`]; (2) reads each one exactly once, at
    /// this closure-creation site, through the same [`Self::ownership_fact_for_place`] path any other read in this
    /// body uses, recording the result as this closure's `captured_operands`; (3) declares a fresh
    /// [`bir::LocalOrigin::Captured`] local per capture plus one [`bir::LocalOrigin::Parameter`] local per declared
    /// parameter, shadowing (and restoring afterward) any outer binding of the same name, so the closure body's own
    /// reads resolve to its own bound copy rather than silently reading through to the enclosing scope; then (4)
    /// lowers the body expression under those bindings. The restore step is what makes this different from every
    /// other nested block this file lowers -- ordinary nested blocks (`if`/`loop` bodies) let a shadowing binding
    /// leak forward in `self.bindings` with no restore, which is harmless for straight-line control flow but would
    /// be wrong here: code lexically after the closure literal must keep resolving the shadowed name to the
    /// *enclosing* variable, not to the closure's own captured copy.
    fn lower_closure(
        &mut self,
        params: &[ast::Spanned<ast::Param>],
        body_expr: &ast::Spanned<ast::Expr>,
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(expr_span);
        let closure_scope = self.new_scope(Some(scope), hir_span_value);

        // ---- Capture every free variable exactly once, at this closure-creation site ----
        let free_names = free_vars_in_closure_body(params, body_expr);
        let mut captured_operands = Vec::with_capacity(free_names.len());
        let mut capture_locals = Vec::with_capacity(free_names.len());
        let mut saved_bindings: Vec<(String, Option<bir::LocalId>)> = Vec::new();
        for name in &free_names {
            // A free name lowering cannot resolve to a tracked outer local (e.g. a module-level `const`) is not
            // captured -- the closure body's own `Self::local_for_name` lookup synthesizes an `External` reference
            // for it exactly like anywhere else, since there is nothing meaningful to read-and-rebind.
            let Some(&outer_local) = self.bindings.get(name) else {
                continue;
            };
            let outer_ty = self.locals[outer_local.index()].ty.clone();
            let outer_place = bir::Place::from_local(outer_local);
            let (fact, last_use) = self.ownership_fact_for_place(&outer_place, &outer_ty);
            captured_operands.push(bir::Operand::place(outer_place, fact, last_use));

            let total_reads = count_reads_in_expr(name, &body_expr.node);
            let capture_local =
                self.declare_new_local_with_reads(name.clone(), outer_ty, closure_scope, hir_span_value, total_reads);
            self.locals[capture_local.index()].origin = bir::LocalOrigin::Captured;
            capture_locals.push(capture_local);
            saved_bindings.push((name.clone(), Some(outer_local)));
        }

        // ---- Bind the closure's own parameters, shadowing any outer binding of the same name ----
        let param_types = self.closure_param_types(params, expr_span);
        let mut closure_param_locals = Vec::with_capacity(params.len());
        for (param, ty) in params.iter().zip(param_types) {
            let previous = self.bindings.get(&param.node.name).copied();
            let total_reads = count_reads_in_expr(&param.node.name, &body_expr.node);
            let local = self.declare_new_local_with_reads(
                param.node.name.clone(),
                ty.clone(),
                closure_scope,
                hir_span(param.span),
                total_reads,
            );
            self.locals[local.index()].origin = bir::LocalOrigin::Parameter;
            closure_param_locals.push(local);
            saved_bindings.push((param.node.name.clone(), previous));
        }

        let mut closure_params = Vec::with_capacity(params.len());
        for (param, local) in params.iter().zip(closure_param_locals) {
            let ty = self.locals[local.index()].ty.clone();
            closure_params.push(bir::CallableParam {
                local,
                name: param.node.name.clone(),
                ty,
                span: hir_span(param.span),
                default: self.lower_callable_default(param.node.default.as_ref(), closure_scope),
            });
        }

        // ---- Lower the body under the closure's own bindings, then restore the enclosing scope's ----
        let mut body_stmts = Vec::new();
        let result = self.lower_expr_to_operand(body_expr, closure_scope, &mut body_stmts);
        for (name, previous) in saved_bindings {
            match previous {
                Some(local) => {
                    self.bindings.insert(name, local);
                }
                None => {
                    self.bindings.remove(&name);
                }
            }
        }

        let closure_body = bir::ClosureBody {
            capture_locals,
            stmts: body_stmts,
            result,
        };
        let ty = self.resolve_ty(expr_span);
        self.push_assign_temp(
            bir::Rvalue::Closure {
                params: closure_params,
                captured_operands,
                body: Box::new(closure_body),
            },
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Resolve each of a closure literal's parameter types from the typechecker's resolved callable type at the
    /// closure's own span, falling back to [`IncanType::Unknown`] per parameter when unavailable or of mismatched
    /// length. Mirrors the existing Rust-emission backend's own `recorded_param_types` fallback
    /// (`src/backend/ir/lower/expr/mod.rs`), minus that backend's additional Rust-display-exact override, which is
    /// meaningful only for concrete Rust closure syntax, not this target-agnostic model.
    fn closure_param_types(&self, params: &[ast::Spanned<ast::Param>], expr_span: ast::Span) -> Vec<IncanType> {
        let resolved = self.type_info.expr_type(expr_span).and_then(|ty| match ty {
            ResolvedType::Function(callable_params, _) => Some(
                callable_params
                    .iter()
                    .map(|p| semantic_type_from_resolved(&p.ty))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        });
        match resolved {
            Some(types) if types.len() == params.len() => types,
            _ => vec![IncanType::Unknown; params.len()],
        }
    }

    /// Lower a partial callable preset expression (`partial Target(name=value, ...)`) into the same
    /// [`bir::Rvalue::Closure`] shape a closure literal produces, mirroring how the existing Rust-emission backend
    /// already desugars a partial application into a synthesized closure that forwards the still-missing arguments
    /// into a call (`src/backend/ir/lower/expr/mod.rs`'s `ast::Expr::Partial` arm) -- see #1101's B4 pre-intake.
    /// Partial construction currently supports only a bare top-level function-name `target` whose full parameter list
    /// the typechecker resolved. General Body IR calls still distinguish named functions from local callable values
    /// and record local supplied-parameter slots (see [`Self::lower_call`]). A method-shaped partial target from
    /// `partial recv.method(...)`, explicit type arguments, or a target with an unnamed parameter lowers to an
    /// explicit unsupported placeholder instead.
    ///
    /// Preset values (`partial.args`) are lowered once each, at the partial-creation site -- exactly like an
    /// ordinary call argument, not deduplicated per free-variable name the way [`Self::lower_closure`]'s captures
    /// are -- and folded into the synthesized closure's own `captured_operands`. Every declared target parameter
    /// remains a closure parameter in declaration order. A preset parameter records
    /// [`bir::CallableParamDefault::PartialPreset`], while an unpresetted target default retains its distinct
    /// source-default contract: a deferred [`bir::CallableParamDefault::Source`] computation only when it has
    /// usable type facts, otherwise an original-span refusal. Positional local calls skip only preset parameters;
    /// [`Self::lower_call`] records the supplied declaration slots rather than pretending the complete callable
    /// surface is a residual function type.
    ///
    /// `Expr::Partial` uses this same full callable surface through `local_partial_params`; module-level partial
    /// declarations intentionally keep their existing full-signature-plus-preset-metadata projection for backend
    /// and export consumers. A compound-assignment-style mutation of a captured preset from inside a nested closure
    /// is out of scope here in the same way [`Self::lower_closure`]'s own docs note for ordinary closures.
    fn lower_partial(
        &mut self,
        partial: &ast::PartialExpr,
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(expr_span);
        let ast::Expr::Ident(target_name) = &partial.target.node else {
            return self.unsupported_operand(
                "partial callable with a non-function-name target".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        if !partial.type_args.is_empty() {
            return self.unsupported_operand(
                "partial callable with explicit type arguments".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        let Some(binding) = self.type_info.declarations.function_bindings.get(target_name).cloned() else {
            return self.unsupported_operand(
                "partial callable target with no resolvable top-level function signature".to_string(),
                scope,
                hir_span_value,
                out,
            );
        };
        if binding
            .params
            .iter()
            .any(|param| param.name.is_none() || param.kind != ast::ParamKind::Normal)
        {
            return self.unsupported_operand(
                "partial callable target with an unnamed or rest parameter".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }
        let target_name = target_name.clone();
        let direct_call_id = self
            .local_function_declarations
            .get(&target_name)
            .and_then(|candidates| match candidates.as_slice() {
                [target_span] => Some(CompilerNodeId::declaration_span(
                    self.module_identity,
                    target_span.start,
                    target_span.end,
                )),
                _ => None,
            });
        let target_default_sources = self.function_default_sources.get(&target_name).cloned();
        let closure_scope = self.new_scope(Some(scope), hir_span_value);

        // ---- Lower each preset value once, at the partial-creation site, as a captured operand ----
        let mut captured_operands = Vec::with_capacity(partial.args.len());
        let mut capture_locals = Vec::with_capacity(partial.args.len());
        let mut preset_lookup: HashMap<String, bir::LocalId> = HashMap::with_capacity(partial.args.len());
        let mut saved_bindings = Vec::with_capacity(binding.params.len() + partial.args.len());
        for arg in &partial.args {
            let value_ty = self.resolve_ty(arg.value.span);
            let operand = self.lower_expr_to_operand(&arg.value, scope, out);
            captured_operands.push(operand);
            let capture_name = format!("__partial_preset_{}", arg.name);
            let previous = self.bindings.get(&capture_name).copied();
            let capture_local =
                self.declare_new_local_with_reads(capture_name.clone(), value_ty, closure_scope, hir_span_value, 1);
            self.locals[capture_local.index()].origin = bir::LocalOrigin::Captured;
            capture_locals.push(capture_local);
            preset_lookup.insert(arg.name.clone(), capture_local);
            saved_bindings.push((capture_name, previous));
        }

        // ---- Every target parameter stays on the closure surface; presets become overrideable defaults ----
        let mut closure_params = Vec::new();
        let mut call_arg_locals = Vec::with_capacity(binding.params.len());
        for (index, param) in binding.params.iter().enumerate() {
            let Some(param_name) = &param.name else {
                return self.unsupported_operand(
                    "partial callable target with an unnamed parameter".to_string(),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let ty = semantic_type_from_resolved(&param.ty);
            let previous = self.bindings.get(param_name).copied();
            let local =
                self.declare_new_local_with_reads(param_name.clone(), ty.clone(), closure_scope, hir_span_value, 1);
            self.locals[local.index()].origin = bir::LocalOrigin::Parameter;
            let source_param = target_default_sources.as_ref().and_then(|params| params.get(index));
            let default = match preset_lookup.get(param_name).copied() {
                Some(capture) => bir::CallableParamDefault::PartialPreset { capture },
                None => match source_param {
                    Some(source_param) => self.lower_callable_default(source_param.default.as_ref(), closure_scope),
                    None if param.has_default => bir::CallableParamDefault::Unsupported {
                        span: hir_span_value,
                        description: format!(
                            "partial target {target_name} declares a default Body IR could not source"
                        ),
                    },
                    None => bir::CallableParamDefault::Required,
                },
            };
            closure_params.push(bir::CallableParam {
                local,
                name: param_name.clone(),
                ty,
                span: source_param.map_or(hir_span_value, |param| hir_span(param.param_span)),
                default,
            });
            call_arg_locals.push(local);
            saved_bindings.push((param_name.clone(), previous));
        }

        // ---- Synthesize the forwarding call as the closure's single-statement body ----
        let mut body_stmts = Vec::new();
        let call_args: Vec<bir::Operand> = call_arg_locals
            .iter()
            .zip(&binding.params)
            .map(|(&local, param)| {
                let ty = semantic_type_from_resolved(&param.ty);
                let place = bir::Place::from_local(local);
                let (fact, last_use) = self.ownership_fact_for_place(&place, &ty);
                bir::Operand::place(place, fact, last_use)
            })
            .collect();
        let ret_ty = semantic_type_from_resolved(&binding.return_type);
        // The synthesized forwarding call supplies every declared parameter of the target, in declaration order:
        // preset slots are filled from the captured locals and residual slots from the closure's own parameters.
        let forwarding_binding = bir::ArgumentBinding::resolved_positional(call_args.len());
        let result = self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                name: target_name,
                direct_call_id,
                builtin: None,
                type_args: Vec::new(),
                binding: forwarding_binding,
            })),
            fixed_elements(call_args),
            ret_ty,
            closure_scope,
            hir_span_value,
            false,
            &mut body_stmts,
        );

        let closure_body = bir::ClosureBody {
            capture_locals,
            stmts: body_stmts,
            result,
        };

        // ---- The synthesized closure's bindings are lexically private to it, not new outer bindings ----
        for (name, previous) in saved_bindings.into_iter().rev() {
            match previous {
                Some(local) => {
                    self.bindings.insert(name, local);
                }
                None => {
                    self.bindings.remove(&name);
                }
            }
        }

        let ty = self.resolve_ty(expr_span);
        self.push_assign_temp(
            bir::Rvalue::Closure {
                params: closure_params,
                captured_operands,
                body: Box::new(closure_body),
            },
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Lower a `match` expression (`ast::Expr::Match`) into a single [`bir::Rvalue::Match`], mirroring the existing
    /// Rust-emission backend's own `IrExprKind::Match { scrutinee, arms }` node -- see [`bir::Rvalue::Match`]'s docs
    /// for why matching stays one structured node rather than being decomposed into a chain of `If` statements, and
    /// [`bir::Pattern`]'s docs for the closed pattern vocabulary this mirrors and its two deliberate v0 gaps (no
    /// union-type pattern narrowing, no RFC 021 field-alias resolution).
    ///
    /// Bails the whole expression to an explicit unsupported placeholder *before* lowering the scrutinee when any
    /// arm's pattern contains a byte-string literal (the one pattern shape [`bir::Constant`] cannot represent --
    /// see [`match_pattern_is_supported`]), mirroring [`Self::lower_binary`]'s "check before partially lowering"
    /// precedent so an unrepresentable pattern never produces a partially-lowered `Rvalue::Match`.
    fn lower_match(
        &mut self,
        subject: &ast::Spanned<ast::Expr>,
        arms: &[ast::Spanned<ast::MatchArm>],
        expr_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(expr_span);
        if arms
            .iter()
            .any(|arm| !match_pattern_is_supported(&arm.node.pattern.node))
        {
            return self.unsupported_operand(
                "match arm with a byte-string literal pattern".to_string(),
                scope,
                hir_span_value,
                out,
            );
        }

        let scrutinee_ty = self.resolve_ty(subject.span);
        let scrutinee_place = self.lower_expr_to_place(subject, scope, out);
        // Always read as `Borrow` -- see `bir::Rvalue::Match::scrutinee`'s own docs for why the overall scrutinee
        // read must not risk an unconditional move while individual pattern bindings below compute their own,
        // more precise facts against projected places rooted at this same scrutinee.
        let scrutinee_operand = bir::Operand::place(scrutinee_place.clone(), bir::OwnershipFact::Borrow, false);

        let mut lowered_arms = Vec::with_capacity(arms.len());
        for arm in arms {
            let arm_span = hir_span(arm.span);
            let arm_scope = self.new_scope(Some(scope), arm_span);

            // ---- Lower the pattern, declaring one fresh arm-scoped local per distinct bound name ----
            let mut seen: HashMap<String, bir::LocalId> = HashMap::new();
            let mut saved_bindings: Vec<(String, Option<bir::LocalId>)> = Vec::new();
            let pattern = self.lower_match_pattern(
                &arm.node.pattern,
                &scrutinee_ty,
                &scrutinee_place,
                arm_scope,
                &arm.node,
                &mut seen,
                &mut saved_bindings,
            );

            // ---- Guard and body see this arm's own pattern bindings, shadowing any outer binding of the same name
            // ----
            let mut guard_stmts = Vec::new();
            let guard = arm
                .node
                .guard
                .as_ref()
                .map(|g| self.lower_expr_to_operand(g, arm_scope, &mut guard_stmts));

            let (body_stmts, result) = match &arm.node.body {
                ast::MatchBody::Expr(e) => {
                    let mut stmts = Vec::new();
                    let result = self.lower_expr_to_operand(e, arm_scope, &mut stmts);
                    (stmts, result)
                }
                ast::MatchBody::Block(block_stmts) => {
                    let mut stmts = Vec::new();
                    self.lower_block_into(block_stmts, arm_scope, &mut stmts);
                    self.insert_scope_drops(&mut stmts, arm_scope);
                    (stmts, bir::Operand::Constant(bir::Constant::Unit))
                }
            };

            // ---- Restore the enclosing scope's bindings before moving on to the next (mutually exclusive) arm ----
            for (name, previous) in saved_bindings {
                match previous {
                    Some(local) => {
                        self.bindings.insert(name, local);
                    }
                    None => {
                        self.bindings.remove(&name);
                    }
                }
            }

            lowered_arms.push(bir::MatchArm {
                pattern,
                guard_stmts,
                guard,
                body_stmts,
                result,
            });
        }

        let ty = self.resolve_ty(expr_span);
        self.push_assign_temp(
            bir::Rvalue::Match {
                scrutinee: scrutinee_operand,
                arms: lowered_arms,
            },
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Recursively lower one source `ast::Pattern` node into a [`bir::Pattern`], declaring a fresh arm-scoped local
    /// the first time a bound name is encountered and reusing it for any later `Or`-alternative occurrence of the
    /// same name (`seen`) -- Incan's typechecker (RFC 071) requires every alternative of an `A(x) | B(x)` pattern to
    /// bind an identical name/type set, so Rust's own single shared binding slot per name is the correct target
    /// shape, not one local per occurrence. `saved_bindings` accumulates `(name, previous_local)` pairs so
    /// [`Self::lower_match`] can restore `self.bindings` to the enclosing scope once this arm's guard/body have
    /// both been lowered, the same save/restore shape [`Self::lower_closure`] already uses around its own
    /// params/captures.
    ///
    /// `place` is the (possibly already-projected) scrutinee place this pattern node corresponds to; each
    /// recursive call into a `Tuple`/`Struct`/`Enum` sub-pattern extends it with one more
    /// [`bir::PlaceElem::Field`] projection -- named for a struct field, or the zero-based positional index as a
    /// string for a tuple/enum-variant positional field, mirroring [`Self::lower_tuple_unpack`]'s own tuple-element
    /// projection convention (`.0`/`.1` Rust tuple-field-access spelling) rather than inventing a second one.
    ///
    /// `expected_ty` is the best available type for this pattern node: propagated through [`Self::lower_match`]'s
    /// own `Self::resolve_ty` call on the scrutinee for the root pattern, and through
    /// [`tuple_element_types`] for `Tuple` sub-patterns (both already-established sources elsewhere in this file);
    /// a `Struct`/`Enum` constructor pattern's own fields fall back to [`IncanType::Unknown`] per field, since
    /// resolving a model/class/enum-variant's real field types would mean rebuilding the existing Rust-emission
    /// backend's own field-type-projection machinery (`constructor_field_types_for_pattern` in
    /// `src/backend/ir/lower/expr/patterns.rs`), which this bucket deliberately does not mirror -- see
    /// [`bir::Pattern`]'s own docs.
    #[allow(clippy::too_many_arguments)]
    fn lower_match_pattern(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        expected_ty: &IncanType,
        place: &bir::Place,
        arm_scope: bir::ScopeId,
        arm: &ast::MatchArm,
        seen: &mut HashMap<String, bir::LocalId>,
        saved_bindings: &mut Vec<(String, Option<bir::LocalId>)>,
    ) -> bir::Pattern {
        let span = hir_span(pattern.span);
        match &pattern.node {
            ast::Pattern::Wildcard => bir::Pattern::Wildcard,
            ast::Pattern::Binding(name) => {
                let local = match seen.get(name) {
                    Some(&local) => local,
                    None => {
                        let total_reads = count_reads_in_match_arm(name, arm);
                        let previous = self.bindings.get(name).copied();
                        let local = self.declare_new_local_with_reads(
                            name.clone(),
                            expected_ty.clone(),
                            arm_scope,
                            span,
                            total_reads,
                        );
                        seen.insert(name.clone(), local);
                        saved_bindings.push((name.clone(), previous));
                        local
                    }
                };
                let (fact, last_use) = self.ownership_fact_for_place(place, expected_ty);
                bir::Pattern::Var(bir::PatternBinding { local, fact, last_use })
            }
            // `match_pattern_is_supported` has already ruled out the one shape `lower_literal` cannot represent
            // (a byte-string literal) for every arm in this match before `Self::lower_match` calls this method at
            // all, so the `None` case here is unreachable in practice; `Constant::Unit` is a harmless, structurally
            // valid fallback rather than a panic if that invariant is ever violated.
            ast::Pattern::Literal(lit) => bir::Pattern::Literal(lower_literal(lit).unwrap_or(bir::Constant::Unit)),
            ast::Pattern::Tuple(items) => {
                let element_types = tuple_element_types(expected_ty, items.len());
                let fields = items
                    .iter()
                    .zip(element_types.iter())
                    .enumerate()
                    .map(|(index, (item, element_ty))| {
                        let mut field_place = place.clone();
                        field_place.projection.push(bir::PlaceElem::Field(index.to_string()));
                        self.lower_match_pattern(item, element_ty, &field_place, arm_scope, arm, seen, saved_bindings)
                    })
                    .collect();
                bir::Pattern::Tuple(fields)
            }
            ast::Pattern::Constructor(name, args) => {
                // Preserve exact source-local pattern targets instead of asking the executor to recover a
                // declaration from the printed constructor spelling. The direct profile accepts only canonical
                // named fields of a plain model; every other structurally lowered constructor remains the
                // name-only fallback below and is visibly refused by replacement execution.
                if let Some(declaration) = self.local_nominal_declarations.get(name)
                    && matches!(expected_ty, IncanType::Named(type_name) if type_name == name)
                    && args.iter().all(|arg| matches!(arg, ast::PatternArg::Named(_, _)))
                {
                    let fields = args
                        .iter()
                        .filter_map(|arg| match arg {
                            ast::PatternArg::Named(field, pat) => {
                                let mut field_place = place.clone();
                                field_place.projection.push(bir::PlaceElem::Field(field.clone()));
                                Some((
                                    field.clone(),
                                    self.lower_match_pattern(
                                        pat,
                                        &IncanType::Unknown,
                                        &field_place,
                                        arm_scope,
                                        arm,
                                        seen,
                                        saved_bindings,
                                    ),
                                ))
                            }
                            ast::PatternArg::Positional(_) => None,
                        })
                        .collect();
                    return bir::Pattern::Nominal {
                        target: bir::NominalPatternTarget {
                            direct_declaration_id: declaration.direct_declaration_id.clone(),
                            name: declaration.name.clone(),
                        },
                        fields,
                    };
                }

                if let Some((enum_name, variant_name)) = name.rsplit_once("::").or_else(|| name.rsplit_once('.'))
                    && args.is_empty()
                    && matches!(expected_ty, IncanType::Named(type_name) if type_name == enum_name)
                    && let Some(declaration) = self.local_fieldless_enum_declarations.get(enum_name)
                    && let Some(variant) = declaration.variants.iter().find(|variant| variant.name == variant_name)
                {
                    return bir::Pattern::FieldlessEnumVariant(bir::FieldlessEnumVariantTarget {
                        enum_declaration_id: declaration.direct_declaration_id.clone(),
                        variant_declaration_id: variant.direct_declaration_id.clone(),
                        enum_name: declaration.name.clone(),
                        variant_name: variant.name.clone(),
                    });
                }

                if let Some(variant) = result_variant_kind(name)
                    && let Some((ok_type, error_type)) = result_type_parts(expected_ty)
                    && args.len() == 1
                    && let [ast::PatternArg::Positional(payload)] = args.as_slice()
                {
                    let payload_type = match variant {
                        bir::ResultVariantKind::Ok => ok_type,
                        bir::ResultVariantKind::Err => error_type,
                    };
                    let lowered_payload =
                        self.lower_match_pattern(payload, payload_type, place, arm_scope, arm, seen, saved_bindings);
                    return bir::Pattern::Result {
                        variant,
                        fields: vec![lowered_payload],
                    };
                }

                // Mirrors the existing Rust-emission backend's own `lower_pattern` (non-union-aware) mapping
                // exactly: a mix of named and positional arguments (unusual, likely non-representative source)
                // still lowers every sub-pattern's own bindings for side effects, but only the named fields survive
                // into the constructed `Pattern` once `has_named` is known.
                let mut named_fields = Vec::new();
                let mut positional_fields = Vec::new();
                let mut has_named = false;
                let mut positional_index = 0usize;
                for arg in args {
                    match arg {
                        ast::PatternArg::Named(field, pat) => {
                            has_named = true;
                            let mut field_place = place.clone();
                            field_place.projection.push(bir::PlaceElem::Field(field.clone()));
                            let lowered = self.lower_match_pattern(
                                pat,
                                &IncanType::Unknown,
                                &field_place,
                                arm_scope,
                                arm,
                                seen,
                                saved_bindings,
                            );
                            named_fields.push((field.clone(), lowered));
                        }
                        ast::PatternArg::Positional(pat) => {
                            let mut field_place = place.clone();
                            field_place
                                .projection
                                .push(bir::PlaceElem::Field(positional_index.to_string()));
                            positional_index += 1;
                            let lowered = self.lower_match_pattern(
                                pat,
                                &IncanType::Unknown,
                                &field_place,
                                arm_scope,
                                arm,
                                seen,
                                saved_bindings,
                            );
                            positional_fields.push(lowered);
                        }
                    }
                }
                if has_named {
                    bir::Pattern::Struct {
                        name: name.clone(),
                        fields: named_fields,
                    }
                } else {
                    bir::Pattern::Enum {
                        name: String::new(),
                        variant: name.clone(),
                        fields: positional_fields,
                    }
                }
            }
            ast::Pattern::Group(inner) => {
                self.lower_match_pattern(inner, expected_ty, place, arm_scope, arm, seen, saved_bindings)
            }
            ast::Pattern::Or(items) => {
                let alternatives = items
                    .iter()
                    .map(|item| {
                        self.lower_match_pattern(item, expected_ty, place, arm_scope, arm, seen, saved_bindings)
                    })
                    .collect();
                bir::Pattern::Or(alternatives)
            }
        }
    }
}

/// Return the first explicitly unsupported default statement, preserving the source span a direct consumer must
/// show when it refuses an omitted argument.
///
/// [`BodyBuilder::unsupported_operand`] records every unsupported expression as a
/// [`bir::StatementKind::Unsupported`] statement. Defaults can also nest executable statement sequences inside
/// control-flow, race arms, closures, generators, and match arms, so the scan walks each such sequence before the
/// deferred computation becomes callable metadata.
fn first_unsupported_default_statement(stmts: &[bir::Statement]) -> Option<(HirSourceSpan, String)> {
    stmts.iter().find_map(first_unsupported_default_statement_inner)
}

/// Inspect one statement and each rvalue shape that owns a nested executable statement sequence.
fn first_unsupported_default_statement_inner(statement: &bir::Statement) -> Option<(HirSourceSpan, String)> {
    match &statement.kind {
        bir::StatementKind::Unsupported { description } => Some((statement.span, description.clone())),
        bir::StatementKind::Assign { rvalue, .. } => first_unsupported_default_rvalue(rvalue),
        bir::StatementKind::If {
            then_block, else_block, ..
        } => first_unsupported_default_statement(&then_block.stmts).or_else(|| {
            else_block
                .as_ref()
                .and_then(|block| first_unsupported_default_statement(&block.stmts))
        }),
        bir::StatementKind::Loop { body } => first_unsupported_default_statement(&body.stmts),
        bir::StatementKind::Race { arms, .. } => arms
            .iter()
            .find_map(|arm| first_unsupported_default_statement(&arm.body.stmts)),
        _ => None,
    }
}

/// Inspect an rvalue's deferred executable parts without treating its explicit operands as source syntax to rebuild.
fn first_unsupported_default_rvalue(rvalue: &bir::Rvalue) -> Option<(HirSourceSpan, String)> {
    match rvalue {
        bir::Rvalue::Closure { body, .. } => first_unsupported_default_statement(&body.stmts),
        bir::Rvalue::Generator { body, .. } => first_unsupported_default_statement(&body.stmts),
        bir::Rvalue::Match { arms, .. } => arms.iter().find_map(|arm| {
            first_unsupported_default_statement(&arm.guard_stmts)
                .or_else(|| first_unsupported_default_statement(&arm.body_stmts))
        }),
        _ => None,
    }
}

// ============================================================================
// Comprehension desugaring helpers
// ============================================================================

/// The innermost action a list/dict-comprehension clause chain performs once every clause accepts one binding
/// combination -- what [`BodyBuilder::lower_comprehension_terminal`] lowers. It distinguishes a list's
/// single-element push from a dict's key/value insert while sharing the same clause-chain desugar.
enum ComprehensionTerminal<'a> {
    /// Push `element`'s value into the list at `list_local`.
    ListPush {
        list_local: bir::LocalId,
        element: &'a ast::Spanned<ast::Expr>,
    },
    /// Insert `key`/`value` into the dict at `dict_local`.
    DictInsert {
        dict_local: bir::LocalId,
        key: &'a ast::Spanned<ast::Expr>,
        value: &'a ast::Spanned<ast::Expr>,
    },
    /// Suspend the surrounding generator body with `element` for one accepted binding combination.
    GeneratorYield { element: &'a ast::Spanned<ast::Expr> },
}

impl ComprehensionTerminal<'_> {
    /// Count `name` occurrences in this terminal's own expression(s), for seeding a comprehension `for`-clause
    /// binding's last-use countdown (see [`BodyBuilder::declare_new_local_with_reads`]'s doc for why comprehension
    /// bindings cannot reuse the statement-suffix-based [`count_reads_in_stmts`]).
    fn count_reads(&self, name: &str) -> usize {
        match self {
            Self::ListPush { element, .. } => count_reads_in_expr(name, &element.node),
            Self::DictInsert { key, value, .. } => {
                count_reads_in_expr(name, &key.node) + count_reads_in_expr(name, &value.node)
            }
            Self::GeneratorYield { element } => count_reads_in_expr(name, &element.node),
        }
    }
}

/// Build the single mirrored `(pattern, iter, filter)` clause list a list/dict comprehension carries, as an owned
/// `Vec<ast::ComprehensionClause>` so [`BodyBuilder::lower_comprehension_clauses`] can share its
/// `&[ast::ComprehensionClause]`-based recursion with generator expressions' real multi-clause `generator.clauses`
/// without a second clause-walking implementation. See [`BodyBuilder::lower_list_comp`]'s docs for why only this
/// single mirrored clause is used, not the comprehension's own (unread-elsewhere) `clauses` field.
fn single_comprehension_clauses(
    pattern: &ast::Spanned<ast::Pattern>,
    iter: &ast::Spanned<ast::Expr>,
    filter: Option<&ast::Spanned<ast::Expr>>,
) -> Vec<ast::ComprehensionClause> {
    let mut clauses = vec![ast::ComprehensionClause::For {
        pattern: pattern.clone(),
        iter: iter.clone(),
    }];
    if let Some(filter) = filter {
        clauses.push(ast::ComprehensionClause::If(filter.clone()));
    }
    clauses
}

/// Count `name` occurrences across a tail of comprehension/generator clauses, for seeding a `for`-clause binding's
/// last-use countdown alongside [`ComprehensionTerminal::count_reads`] (see
/// [`BodyBuilder::lower_comprehension_clauses`]).
fn count_reads_in_comprehension_clauses(name: &str, clauses: &[ast::ComprehensionClause]) -> usize {
    clauses
        .iter()
        .map(|clause| match clause {
            ast::ComprehensionClause::For { iter, .. } => count_reads_in_expr(name, &iter.node),
            ast::ComprehensionClause::If(cond) => count_reads_in_expr(name, &cond.node),
        })
        .sum()
}

// ============================================================================
// Free helper functions
// ============================================================================

/// One resolved direct-call declaration narrowed to the executor-relevant facts.
///
/// `direct_call_id` is present only for a declaration physically represented by this module. Keeping the target
/// separate from its parameter slots prevents a future consumer from treating a successfully planned argument list
/// as proof that an imported callable is executable here.
struct DirectCallDeclaration {
    slots: Option<Vec<DeclaredSlot>>,
    direct_call_id: Option<CompilerNodeId>,
    builtin: Option<bir::NamedCallableBuiltin>,
}

/// One declared callable parameter or nominal field, reduced to the facts call-site binding actually needs.
///
/// Direct functions, methods, local callables, and nominal constructors each carry their declared surface in a
/// different type (`IncanCallableParam`, `symbols::CallableParam`, a field layout). Binding them through one planner
/// is what keeps #1158's "one mechanism" contract honest, so each caller narrows its own declaration surface to this
/// shape first rather than getting its own copy of the binding rules.
struct DeclaredSlot {
    /// Declared name, when the slot can be supplied by name. Positional-only slots carry `None`.
    name: Option<String>,
    /// Whether omitting this slot is legal because the declaration supplies a default.
    has_default: bool,
    /// Whether this slot holds a partial's construction-time preset, which positional binding skips.
    is_partial_preset: bool,
    /// Whether this slot is a `*args`/`**kwargs` rest parameter, which this planner refuses.
    is_rest: bool,
}

impl DeclaredSlot {
    /// Narrow a semantic callable parameter (a local callable value's signature) to its binding-relevant facts.
    fn from_semantic_param(param: &IncanCallableParam) -> Self {
        Self {
            name: param.name.clone(),
            has_default: param.has_default,
            is_partial_preset: param.is_partial_preset,
            is_rest: param.kind != IncanCallableParamKind::Normal,
        }
    }

    /// Narrow a typechecker-resolved source callable parameter to its binding-relevant facts.
    fn from_checked_param(param: &CallableParam) -> Self {
        Self {
            name: param.name.clone(),
            has_default: param.has_default,
            is_partial_preset: param.is_partial_preset,
            is_rest: param.kind != ast::ParamKind::Normal,
        }
    }
}

/// Expand a statically shaped spread argument into the ordinary arguments it stands for.
///
/// The typechecker proves a spread's shape when its operand is written as a literal whose arity is visible before
/// lowering -- `f(*(1, 2))`, `f(**{"a": 1})` -- and records the result as a
/// [`FixedUnpackPlan`](crate::frontend::typechecker::FixedUnpackPlan). Those calls have a perfectly ordinary fixed
/// arity, so they bind through the same declaration-slot planner as any other call rather than being pushed onto
/// the runtime-arity path; a `*(1, 2)` against `def add(a, b)` really is `add(1, 2)`.
///
/// Returns `None` when the spread has no proven shape, which is the ordinary case (`f(*xs)` for a list variable):
/// its arity is a runtime fact and it belongs on the unresolved-arity path. Also returns `None` when a plan exists
/// but the operand is not a destructurable literal -- the plan is recorded for tuple-*typed* operands too, and
/// those have no written elements to expand.
///
/// Parentheses are transparent here exactly as they are for the typechecker's own shape check, so the two stages
/// agree on which spellings count as shaped.
fn expand_shaped_spread(type_info: &TypeCheckInfo, arg: &ast::CallArg) -> Option<Vec<ast::CallArg>> {
    /// Look through any number of parenthesis layers to the expression they wrap.
    ///
    /// The typechecker's own shape check treats parentheses as transparent, so this must too, or the two stages
    /// would disagree about which spellings count as statically shaped.
    fn unparenthesized(expr: &ast::Spanned<ast::Expr>) -> &ast::Spanned<ast::Expr> {
        match &expr.node {
            ast::Expr::Paren(inner) => unparenthesized(inner),
            _ => expr,
        }
    }

    match arg {
        ast::CallArg::PositionalUnpack(source) => {
            if !matches!(
                type_info.fixed_unpack_plan(source.span),
                Some(FixedUnpackPlan::Positional(_))
            ) {
                return None;
            }
            match &unparenthesized(source).node {
                ast::Expr::Tuple(items) => Some(items.iter().cloned().map(ast::CallArg::Positional).collect()),
                ast::Expr::List(entries) => entries
                    .iter()
                    .map(|entry| match entry {
                        ast::ListEntry::Element(value) => Some(ast::CallArg::Positional(value.clone())),
                        ast::ListEntry::Spread(_) => None,
                    })
                    .collect(),
                _ => None,
            }
        }
        ast::CallArg::KeywordUnpack(source) => {
            if !matches!(
                type_info.fixed_unpack_plan(source.span),
                Some(FixedUnpackPlan::Keyword(_))
            ) {
                return None;
            }
            let ast::Expr::Dict(entries) = &unparenthesized(source).node else {
                return None;
            };
            entries
                .iter()
                .map(|entry| match entry {
                    ast::DictEntry::Pair(key, value) => match &unparenthesized(key).node {
                        ast::Expr::Literal(ast::Literal::String(name)) => {
                            Some(ast::CallArg::Named(name.clone(), value.clone()))
                        }
                        _ => None,
                    },
                    ast::DictEntry::Spread(_) => None,
                })
                .collect()
        }
        ast::CallArg::Positional(_) | ast::CallArg::Named(_, _) => None,
    }
}

/// Plan a call's supplied arguments into declaration slots before lowering any expression.
///
/// This validates the whole call before any *argument* ownership read is emitted, then leaves the returned
/// expressions in source evaluation order. A method call is the one exception on the callee side: its receiver is
/// read first, because source evaluation observes the receiver before the arguments, so a refusal here can follow a
/// receiver read that the refused call never consumes. The caller can therefore lower values left-to-right while the
/// final argument vector follows declaration order. Preset-default slots are intentionally omitted from positional
/// binding and may be skipped in the vector because the call's [`bir::ArgumentBinding`] records each supplied operand's
/// declaration slot; an omitted ordinary default is recorded the same way, as a defaulted slot.
///
/// `callee` is the caller's own description of the target (`function \`add\``, `local callable \`g\``,
/// `method \`add\``), so a refusal names the specific spelling that failed rather than a generic label.
fn plan_declared_args<'a>(
    callee: &str,
    params: &[DeclaredSlot],
    args: &'a [ast::CallArg],
) -> Result<Vec<(usize, &'a ast::Spanned<ast::Expr>)>, String> {
    if params.iter().any(|param| param.is_rest) {
        return Err(format!("{callee} has a rest parameter"));
    }
    let positional_slots: Vec<usize> = params
        .iter()
        .enumerate()
        .filter_map(|(index, param)| (!param.is_partial_preset).then_some(index))
        .collect();
    let mut slots: Vec<Option<&ast::Spanned<ast::Expr>>> = vec![None; params.len()];
    let mut positional_index = 0usize;
    let mut planned = Vec::with_capacity(args.len());
    for arg in args {
        let (index, expr) = match arg {
            ast::CallArg::Positional(expr) => {
                if positional_index >= positional_slots.len() {
                    return Err(format!(
                        "{callee} expects at most {} positional arguments, got {}",
                        positional_slots.len(),
                        args.len()
                    ));
                }
                let index = positional_slots[positional_index];
                positional_index += 1;
                (index, expr)
            }
            ast::CallArg::Named(arg_name, expr) => {
                let Some(index) = params
                    .iter()
                    .position(|param| param.name.as_deref() == Some(arg_name.as_str()))
                else {
                    return Err(format!("{callee} has no parameter `{arg_name}`"));
                };
                (index, expr)
            }
            ast::CallArg::PositionalUnpack(_) => {
                return Err(format!("{callee} called with a positional argument spread"));
            }
            ast::CallArg::KeywordUnpack(_) => {
                return Err(format!("{callee} called with a keyword argument spread"));
            }
        };
        if slots[index].is_some() {
            let parameter = params[index].name.as_deref().unwrap_or("<unnamed>");
            return Err(format!("{callee} receives `{parameter}` more than once"));
        }
        slots[index] = Some(expr);
        planned.push((index, expr));
    }

    let required = params.iter().filter(|param| !param.has_default).count();
    if let Some((_index, parameter)) = params
        .iter()
        .enumerate()
        .find(|(index, parameter)| slots[*index].is_none() && !parameter.has_default)
    {
        return Err(format!(
            "{callee} expects at least {required} required arguments, got {}; missing required parameter `{}`",
            args.len(),
            parameter.name.as_deref().unwrap_or("<unnamed>")
        ));
    }
    // An omitted interior default needs no refusal any more. #1124 had to reject one because a flat operand vector
    // could not say which slot a later operand filled; `bir::ArgumentBinding` now records exactly that, so a sparse
    // call is representable rather than ambiguous.
    Ok(planned)
}

/// Wrap fixed operands as single-value element list entries.
///
/// Used by every lowering path that produces a known number of values -- the overwhelming majority. Only a source
/// spread produces a [`bir::ArgumentElement::Spread`], so this keeps those call sites reading as they did before
/// element lists became variable-arity.
fn fixed_elements(operands: Vec<bir::Operand>) -> Vec<bir::ArgumentElement> {
    operands.into_iter().map(bir::ArgumentElement::One).collect()
}

/// Whether a type is string-like enough to route binary operators through the compiler-owned string helpers
/// (mirrors `is_string_like_type` in `src/backend/ir/conversions.rs`, restated here so Body IR does not depend on
/// that Rust-emission-specific module — see this file's module docs).
fn is_string_like(ty: &IncanType) -> bool {
    matches!(
        ty,
        IncanType::Primitive(IncanPrimitiveType::Str | IncanPrimitiveType::FrozenStr)
    )
}

/// Map a string-typed binary operator to its compiler-owned helper operation, or `None` for operators that have no
/// string-specific helper (arithmetic-only operators never reach here because `lower_binary` only checks this for
/// string-like operand types).
fn string_helper_for_binop(op: ast::BinaryOp) -> Option<bir::HelperOp> {
    match op {
        ast::BinaryOp::Add => Some(bir::HelperOp::StrConcat),
        ast::BinaryOp::Eq => Some(bir::HelperOp::StrEq),
        ast::BinaryOp::NotEq => Some(bir::HelperOp::StrNe),
        ast::BinaryOp::Lt => Some(bir::HelperOp::StrLt),
        ast::BinaryOp::LtEq => Some(bir::HelperOp::StrLe),
        ast::BinaryOp::Gt => Some(bir::HelperOp::StrGt),
        ast::BinaryOp::GtEq => Some(bir::HelperOp::StrGe),
        _ => None,
    }
}

/// Map a surface binary operator to Body IR's canonical arithmetic/comparison/boolean operator set, or `None` for
/// operators v0 does not model.
///
/// The unmapped set is `Pow`, `MatMul`, both pipes, the bitwise `BitAnd`/`BitOr`/`BitXor`, the `Shl`/`Shr` shifts,
/// `In`/`NotIn`, and `Is`/`IsNot`. Membership is the notable one: `parity-987-0003` records string `in` as a
/// `Preserved` behavior, so refusing it here is a tracked #1101 gap rather than a settled boundary. Adding any of
/// these needs a matching [`bir::BinOp`] variant, or a compiler-owned [`bir::HelperOp`] where the operation is a
/// runtime call rather than a primitive -- the same split [`string_helper_for_binop`] already makes.
fn lower_binary_op(op: ast::BinaryOp) -> Option<bir::BinOp> {
    match op {
        ast::BinaryOp::Add => Some(bir::BinOp::Add),
        ast::BinaryOp::Sub => Some(bir::BinOp::Sub),
        ast::BinaryOp::Mul => Some(bir::BinOp::Mul),
        ast::BinaryOp::Div => Some(bir::BinOp::Div),
        ast::BinaryOp::FloorDiv => Some(bir::BinOp::FloorDiv),
        ast::BinaryOp::Mod => Some(bir::BinOp::Mod),
        ast::BinaryOp::Eq => Some(bir::BinOp::Eq),
        ast::BinaryOp::NotEq => Some(bir::BinOp::Ne),
        ast::BinaryOp::Lt => Some(bir::BinOp::Lt),
        ast::BinaryOp::LtEq => Some(bir::BinOp::Le),
        ast::BinaryOp::Gt => Some(bir::BinOp::Gt),
        ast::BinaryOp::GtEq => Some(bir::BinOp::Ge),
        ast::BinaryOp::And => Some(bir::BinOp::And),
        ast::BinaryOp::Or => Some(bir::BinOp::Or),
        _ => None,
    }
}

/// Map a surface unary operator to Body IR's unary operator set. Exhaustive: all three surface unary operators have
/// a direct Body IR equivalent.
const fn lower_unary_op(op: ast::UnaryOp) -> bir::UnOp {
    match op {
        ast::UnaryOp::Neg => bir::UnOp::Neg,
        ast::UnaryOp::Not => bir::UnOp::Not,
        ast::UnaryOp::Invert => bir::UnOp::Invert,
    }
}

/// Lower a literal to a Body IR constant, or `None` for literal kinds v0 does not model distinctly (`bytes`).
fn lower_literal(lit: &ast::Literal) -> Option<bir::Constant> {
    match lit {
        ast::Literal::Int(int_lit) => Some(bir::Constant::Int(int_lit.value)),
        ast::Literal::Float(float_lit) => Some(bir::Constant::Float(float_lit.repr.clone())),
        ast::Literal::Decimal(decimal_lit) => Some(bir::Constant::Float(decimal_lit.repr.clone())),
        ast::Literal::String(s) => Some(bir::Constant::Str(s.clone())),
        ast::Literal::Bool(b) => Some(bir::Constant::Bool(*b)),
        ast::Literal::None => Some(bir::Constant::None),
        ast::Literal::Bytes(_) => None,
    }
}

/// Short diagnostic label for a statement kind v0 does not lower.
///
/// Statement-position `loop:` is named explicitly because it is the one entry here whose Body IR vocabulary
/// already exists: [`BodyBuilder::lower_loop_expr`] emits [`bir::StatementKind::Loop`] for the expression
/// spelling, and only [`BodyBuilder::lower_stmt_into`]'s dispatch is missing (#1101). Leaving it under the
/// generic "statement" label made a five-line dispatch gap read like an unmodeled construct.
fn unsupported_stmt_label(stmt: &ast::Statement) -> String {
    match stmt {
        ast::Statement::Loop(_) => "statement-position `loop:`".to_string(),
        ast::Statement::Unsafe(_) => "unsafe block".to_string(),
        ast::Statement::VocabExpressionItem(_) => "vocab expression item".to_string(),
        ast::Statement::Surface(_) => "surface statement".to_string(),
        ast::Statement::VocabBlock(_) => "vocab block".to_string(),
        _ => "statement".to_string(),
    }
}

/// Short diagnostic label for an expression kind v0 does not lower.
///
/// Only reached from [`BodyBuilder::lower_expr_to_operand`]'s fallback arm, so every expression kind that arm
/// dispatches by name -- closures and partial callables included, since #1124 gave both a real lowering -- is
/// deliberately absent here. Async surface (`await`, `race for`) and vocab/scoped-DSL surface are named rather
/// than left to the generic label, because both are tracked remaining work under #1101 (#1164 and #1166
/// respectively) and a diagnostic reading only "expression" hides which one a program actually hit.
fn unsupported_expr_label(expr: &ast::Expr) -> String {
    match expr {
        ast::Expr::Yield(_) => "yield expression".to_string(),
        ast::Expr::Range { .. } => "range expression outside a for-loop".to_string(),
        ast::Expr::Surface(surface) => surface_expr_label(&surface.payload),
        ast::Expr::VocabBlock(_) => "vocab block expression".to_string(),
        _ => "expression".to_string(),
    }
}

/// Name the specific surface-expression payload behind an [`ast::Expr::Surface`] refusal.
///
/// The payloads split into two very different buckets: `await`/`race for` are the async surface #1164 represents,
/// which #1155 needs before it can execute task state, while the remaining payloads are vocab/DSL nodes that the
/// legacy pipeline desugars away before lowering and that only reach here when a caller skips that pass (#1166).
fn surface_expr_label(payload: &ast::SurfaceExprPayload) -> String {
    match payload {
        ast::SurfaceExprPayload::PrefixUnary(_) => {
            "prefix-keyword surface expression (for example `await`)".to_string()
        }
        ast::SurfaceExprPayload::RaceFor(_) => "`race for` expression".to_string(),
        ast::SurfaceExprPayload::LeadingDotPath { .. } => "scoped DSL leading-dot path".to_string(),
        ast::SurfaceExprPayload::ScopedGlyph { .. } => "scoped DSL glyph operator".to_string(),
        ast::SurfaceExprPayload::ScopedSymbolCall { .. } => "scoped DSL symbol call".to_string(),
    }
}

/// Resolve the per-element types for a tuple-typed value being destructured into `count` targets, falling back to
/// [`IncanType::Unknown`] per element when the resolved type is not (or not yet) known to be a tuple of the right
/// arity -- mirrors how the existing Rust-emission backend falls back to `IrType::Unknown` per slot in the same
/// situation (`src/backend/ir/lower/stmt.rs`'s `TupleUnpack` lowering). Used by
/// [`BodyBuilder::lower_tuple_unpack`], [`BodyBuilder::lower_tuple_assign`], and
/// [`BodyBuilder::bind_for_pattern_fields`].
///
/// A tuple type reaches lowering in two spellings and both must be understood here. A tuple *literal* resolves to
/// [`IncanType::Tuple`], while a written `tuple[A, B]` *annotation* resolves through the collection-type registry
/// and therefore arrives as an [`IncanType::Generic`] whose base is that registry's canonical name. Matching only
/// the first spelling silently degraded every element of an annotated tuple to `Unknown`, which in turn made each
/// element read `Borrow` rather than its real Copy/non-Copy fact. The generic base is classified through
/// [`collections::from_str`] rather than compared against a literal name, so the registry stays the single source
/// of truth for that vocabulary.
/// Why a statement-level destructure of `value_ty` into `arity` names cannot be lowered, or `None` when it can.
///
/// The statement sibling of [`unsupported_for_pattern`], and it exempts the same two types for the same reason:
/// `Unknown` and `Never` mean the typechecker either already reported a failure or is looking at unreachable code,
/// so lowering has nothing to refuse. Everything else — including Rust interop, which is checked against the same
/// [`rust_tuple_arity`] rule the typechecker uses rather than waved through — must be a tuple of exactly matching
/// arity before lowering may emit a `.0`/`.1` field projection. Without this, a non-tuple value produced
/// `__incan_tuple_unpack_*.0` against a fieldless value and surfaced as a raw `rustc` E0610 (#1132).
fn unsupported_tuple_destructure(value_ty: &IncanType, arity: usize) -> Option<String> {
    if matches!(value_ty, IncanType::Unknown | IncanType::Never) {
        return None;
    }
    // Interop values go through the same accepted-shape rule the typechecker uses, not an exemption: a readable
    // tuple spelling lowers, and anything opaque refuses. Waving every `RustInteropPath` through would have let a
    // genuine non-tuple Rust value reach a `.0`/`.1` projection, which is the leakage #1132 closes.
    if let IncanType::RustInteropPath(path) = value_ty {
        return match rust_tuple_arity(path) {
            Some(rust_arity) if rust_arity == arity => None,
            Some(rust_arity) => Some(format!(
                "tuple destructure binds {arity} names but Rust value type `{path}` has {rust_arity} elements"
            )),
            None => Some(format!(
                "tuple destructure of Rust value type `{path}` whose tuple shape cannot be verified"
            )),
        };
    }
    let Some(element_types) = tuple_type_elements(value_ty) else {
        return Some(format!("tuple destructure of non-tuple value type `{value_ty}`"));
    };
    if element_types.len() != arity {
        return Some(format!(
            "tuple destructure binds {arity} names but value type `{value_ty}` has {} elements",
            element_types.len()
        ));
    }
    None
}

fn tuple_element_types(ty: &IncanType, count: usize) -> Vec<IncanType> {
    match tuple_type_elements(ty) {
        Some(items) if items.len() == count => items.to_vec(),
        _ => vec![IncanType::Unknown; count],
    }
}

/// The element types of a tuple-shaped [`IncanType`], in either spelling, or `None` when `ty` is not a tuple at
/// all. Backs both [`tuple_element_types`] and [`unsupported_for_pattern`], so the "is this a tuple, and of what
/// arity" question is answered in exactly one place rather than once per caller.
fn tuple_type_elements(ty: &IncanType) -> Option<&[IncanType]> {
    match ty {
        IncanType::Tuple(items) => Some(items),
        IncanType::Generic { base, args } if collections::from_str(base) == Some(CollectionTypeId::Tuple) => Some(args),
        _ => None,
    }
}

/// Count textual `Ident(name)` occurrences reachable from `stmts`, restricted to the same statement/expression
/// subset [`BodyBuilder`] actually lowers. This seeds a local's last-use countdown (see
/// [`BodyBuilder::declare_new_local`]).
///
/// This is a **textual, source-order over-approximation**, not dynamic dataflow: it does not special-case shadowing
/// (a later redeclaration of the same name still contributes to this count) and it counts occurrences across all
/// branches of a conditional rather than only the branch that will execute. Both simplifications only ever make the
/// count too high, which biases the resulting ownership fact toward `Clone`/`Borrow` instead of `Move` — never the
/// reverse — so it cannot produce an unsound move.
fn count_reads_in_stmts(name: &str, stmts: &[ast::Spanned<ast::Statement>]) -> usize {
    stmts.iter().map(|stmt| count_reads_in_stmt(name, &stmt.node)).sum()
}

/// Count `name` occurrences reachable from one statement, recursing into every branch of a conditional/loop rather
/// than only the branch that will execute — part of [`count_reads_in_stmts`]'s documented over-approximation.
/// Statement kinds outside v0's lowered subset are not walked and contribute zero (they cannot themselves bind or
/// read `name` in a way v0's lowering will ever observe).
fn count_reads_in_stmt(name: &str, stmt: &ast::Statement) -> usize {
    match stmt {
        ast::Statement::Assignment(a) => count_reads_in_expr(name, &a.value.node),
        ast::Statement::FieldAssignment(fa) => {
            count_reads_in_expr(name, &fa.object.node) + count_reads_in_expr(name, &fa.value.node)
        }
        ast::Statement::IndexAssignment(ia) => {
            count_reads_in_expr(name, &ia.object.node)
                + count_reads_in_expr(name, &ia.index.node)
                + count_reads_in_expr(name, &ia.value.node)
        }
        ast::Statement::CompoundAssignment(ca) => {
            usize::from(ca.name == name) + count_reads_in_expr(name, &ca.value.node)
        }
        ast::Statement::TupleUnpack(tu) => count_reads_in_expr(name, &tu.value.node),
        ast::Statement::TupleAssign(ta) => {
            ta.targets
                .iter()
                .map(|t| count_reads_in_expr(name, &t.node))
                .sum::<usize>()
                + count_reads_in_expr(name, &ta.value.node)
        }
        ast::Statement::ChainedAssignment(ca) => count_reads_in_expr(name, &ca.value.node),
        ast::Statement::Return(Some(e)) => count_reads_in_expr(name, &e.node),
        ast::Statement::Return(None) => 0,
        ast::Statement::If(if_stmt) => {
            let mut total = count_reads_in_condition(name, &if_stmt.condition);
            total += count_reads_in_stmts(name, &if_stmt.then_body);
            for (cond, body) in &if_stmt.elif_branches {
                total += count_reads_in_expr(name, &cond.node);
                total += count_reads_in_stmts(name, body);
            }
            if let Some(else_body) = &if_stmt.else_body {
                total += count_reads_in_stmts(name, else_body);
            }
            total
        }
        ast::Statement::While(w) => count_reads_in_condition(name, &w.condition) + count_reads_in_stmts(name, &w.body),
        ast::Statement::For(f) => count_reads_in_expr(name, &f.iter.node) + count_reads_in_stmts(name, &f.body),
        ast::Statement::Expr(e) => count_reads_in_expr(name, &e.node),
        ast::Statement::Assert(a) => {
            let mut total = match &a.kind {
                ast::AssertKind::Condition(e) => count_reads_in_expr(name, &e.node),
                _ => 0,
            };
            total += a
                .message
                .as_ref()
                .map(|m| count_reads_in_expr(name, &m.node))
                .unwrap_or(0);
            total
        }
        ast::Statement::Break(Some(e)) => count_reads_in_expr(name, &e.node),
        _ => 0,
    }
}

/// Count `name` occurrences in an `if`/`while` condition, including the value expression of a `Condition::Let`
/// pattern condition (even though v0 lowering does not model `if let`/`while let` themselves — see
/// [`BodyBuilder::lower_if`]/[`BodyBuilder::lower_while`] — so the read-count approximation stays an
/// over-approximation rather than silently under-counting).
fn count_reads_in_condition(name: &str, cond: &ast::Condition) -> usize {
    match cond {
        ast::Condition::Expr(e) => count_reads_in_expr(name, &e.node),
        ast::Condition::Let { value, .. } => count_reads_in_expr(name, &value.node),
    }
}

/// Count `name` occurrences reachable from one expression, recursing into every expression kind v0's lowering
/// itself walks (see this module's module-level docs for the covered subset). Expression kinds outside that subset
/// contribute zero, consistent with [`count_reads_in_stmts`]'s "restricted to the same subset `BodyBuilder` actually
/// lowers" scope.
fn count_reads_in_expr(name: &str, expr: &ast::Expr) -> usize {
    match expr {
        ast::Expr::Ident(id) => usize::from(id == name),
        ast::Expr::Binary(l, _, r) => count_reads_in_expr(name, &l.node) + count_reads_in_expr(name, &r.node),
        ast::Expr::Unary(_, e) => count_reads_in_expr(name, &e.node),
        ast::Expr::Call(callee, _, args) => {
            count_reads_in_expr(name, &callee.node)
                + args.iter().map(|a| count_reads_in_call_arg(name, a)).sum::<usize>()
        }
        ast::Expr::MethodCall(recv, _, _, args) => {
            count_reads_in_expr(name, &recv.node) + args.iter().map(|a| count_reads_in_call_arg(name, a)).sum::<usize>()
        }
        ast::Expr::Field(e, _) => count_reads_in_expr(name, &e.node),
        ast::Expr::Index(e, idx) => count_reads_in_expr(name, &e.node) + count_reads_in_expr(name, &idx.node),
        ast::Expr::Slice(base, slice) => {
            count_reads_in_expr(name, &base.node)
                + slice
                    .start
                    .as_ref()
                    .map(|e| count_reads_in_expr(name, &e.node))
                    .unwrap_or(0)
                + slice
                    .end
                    .as_ref()
                    .map(|e| count_reads_in_expr(name, &e.node))
                    .unwrap_or(0)
                + slice
                    .step
                    .as_ref()
                    .map(|e| count_reads_in_expr(name, &e.node))
                    .unwrap_or(0)
        }
        ast::Expr::Paren(e) | ast::Expr::Try(e) => count_reads_in_expr(name, &e.node),
        ast::Expr::Tuple(items) | ast::Expr::Set(items) => {
            items.iter().map(|i| count_reads_in_expr(name, &i.node)).sum()
        }
        ast::Expr::List(entries) => entries
            .iter()
            .map(|entry| match entry {
                ast::ListEntry::Element(e) | ast::ListEntry::Spread(e) => count_reads_in_expr(name, &e.node),
            })
            .sum(),
        ast::Expr::Dict(entries) => entries
            .iter()
            .map(|entry| match entry {
                ast::DictEntry::Pair(k, v) => count_reads_in_expr(name, &k.node) + count_reads_in_expr(name, &v.node),
                ast::DictEntry::Spread(e) => count_reads_in_expr(name, &e.node),
            })
            .sum(),
        ast::Expr::Constructor(_, args) => args.iter().map(|a| count_reads_in_call_arg(name, a)).sum(),
        ast::Expr::Range { start, end, .. } => {
            count_reads_in_expr(name, &start.node) + count_reads_in_expr(name, &end.node)
        }
        ast::Expr::If(if_expr) => {
            count_reads_in_expr(name, &if_expr.condition.node)
                + count_reads_in_stmts(name, &if_expr.then_body)
                + if_expr
                    .else_body
                    .as_ref()
                    .map(|body| count_reads_in_stmts(name, body))
                    .unwrap_or(0)
        }
        ast::Expr::Loop(loop_expr) => count_reads_in_stmts(name, &loop_expr.body),
        ast::Expr::FString(parts) => parts
            .iter()
            .map(|part| match part {
                ast::FStringPart::Literal(_) => 0,
                ast::FStringPart::Expr { expr, .. } => count_reads_in_expr(name, &expr.node),
            })
            .sum(),
        ast::Expr::ListComp(comp) => {
            count_reads_in_expr(name, &comp.iter.node)
                + comp
                    .filter
                    .as_ref()
                    .map(|f| count_reads_in_expr(name, &f.node))
                    .unwrap_or(0)
                + count_reads_in_expr(name, &comp.expr.node)
        }
        ast::Expr::DictComp(comp) => {
            count_reads_in_expr(name, &comp.iter.node)
                + comp
                    .filter
                    .as_ref()
                    .map(|f| count_reads_in_expr(name, &f.node))
                    .unwrap_or(0)
                + count_reads_in_expr(name, &comp.key.node)
                + count_reads_in_expr(name, &comp.value.node)
        }
        ast::Expr::Generator(generator) => {
            count_reads_in_comprehension_clauses(name, &generator.clauses)
                + count_reads_in_expr(name, &generator.expr.node)
        }
        ast::Expr::Closure(params, body) => {
            // `BodyBuilder::lower_closure` reads a captured free variable exactly once at the closure-creation
            // site, however many times the closure body itself uses it afterward (subsequent uses read the
            // closure's own captured-binding local, not the outer one this count seeds) -- so this contributes at
            // most 1, not the raw in-body occurrence count. A name shadowed by the closure's own parameter is never
            // captured at all and so contributes 0, regardless of how many times the body uses its own parameter.
            if params.iter().any(|p| p.node.name == name) {
                0
            } else {
                usize::from(count_reads_in_expr(name, &body.node) > 0)
            }
        }
        ast::Expr::Partial(partial) => {
            // Unlike a closure's captures, a partial callable's preset values are lowered as ordinary sub-expression
            // reads (see `BodyBuilder::lower_partial`), not deduplicated per free-variable name, so this counts them
            // plainly like any other nested expression.
            count_reads_in_expr(name, &partial.target.node)
                + partial
                    .args
                    .iter()
                    .map(|a| count_reads_in_expr(name, &a.value.node))
                    .sum::<usize>()
        }
        // `BodyBuilder::lower_yield` lowers a yielded value through the same `lower_expr_to_operand` path as any
        // other statement's operand, so a name read inside `yield value` must be counted here too -- otherwise it
        // would be undercounted for last-use purposes, the same soundness gap #1101's f-string bucket found and
        // fixed for `count_reads_in_expr`'s `FString` arm.
        ast::Expr::Yield(value) => value.as_ref().map_or(0, |v| count_reads_in_expr(name, &v.node)),
        // Same soundness class as the `Yield`/`FString` arms above: a `match` scrutinee, guard, or arm body is
        // lowered through the ordinary expression/statement paths (`BodyBuilder::lower_match`), so a read of `name`
        // reachable inside any of them must be counted here too. Unlike `collect_free_vars_in_expr`'s `Match` arm,
        // this does not need to exclude an arm's own pattern-bound names from the count: this function is a coarse,
        // source-order over-approximation by design (see its own docs), and over-counting only ever biases the
        // resulting ownership fact toward `Clone`/`Borrow` rather than `Move` -- never unsound.
        ast::Expr::Match(subject, arms) => {
            count_reads_in_expr(name, &subject.node)
                + arms
                    .iter()
                    .map(|arm| count_reads_in_match_arm(name, &arm.node))
                    .sum::<usize>()
        }
        _ => 0,
    }
}

/// Count `name` occurrences reachable from one `match` arm's guard and body, for seeding a pattern-bound local's
/// last-use countdown the same way [`count_reads_in_stmts`] seeds an ordinary binding's -- see
/// [`BodyBuilder::lower_match_pattern`]. Also reused by [`count_reads_in_expr`]'s own `Match` arm so both counting
/// paths agree on what "a read inside this arm" means.
fn count_reads_in_match_arm(name: &str, arm: &ast::MatchArm) -> usize {
    let guard_reads = arm.guard.as_ref().map_or(0, |g| count_reads_in_expr(name, &g.node));
    let body_reads = match &arm.body {
        ast::MatchBody::Expr(e) => count_reads_in_expr(name, &e.node),
        ast::MatchBody::Block(stmts) => count_reads_in_stmts(name, stmts),
    };
    guard_reads + body_reads
}

/// Whether `pattern` is representable by [`bir::Pattern`]'s closed vocabulary. The only unrepresentable shape is a
/// byte-string literal pattern ([`bir::Constant`] has no byte-string variant -- see [`lower_literal`]'s own `None`
/// case for the identical gap in plain literal *expressions*); every other pattern shape lowers structurally, with
/// [`IncanType::Unknown`] field-type fallbacks where needed rather than an outright failure (see
/// [`BodyBuilder::lower_match_pattern`]'s own docs). Checked for every arm before [`BodyBuilder::lower_match`]
/// lowers any of them, mirroring [`BodyBuilder::binary_op_is_supported`]'s "check before partially lowering"
/// precedent.
fn match_pattern_is_supported(pattern: &ast::Pattern) -> bool {
    match pattern {
        ast::Pattern::Literal(ast::Literal::Bytes(_)) => false,
        ast::Pattern::Literal(_) | ast::Pattern::Wildcard | ast::Pattern::Binding(_) => true,
        ast::Pattern::Tuple(items) => items.iter().all(|item| match_pattern_is_supported(&item.node)),
        ast::Pattern::Constructor(_, args) => args.iter().all(|arg| match arg {
            ast::PatternArg::Positional(pat) | ast::PatternArg::Named(_, pat) => match_pattern_is_supported(&pat.node),
        }),
        ast::Pattern::Group(inner) => match_pattern_is_supported(&inner.node),
        ast::Pattern::Or(items) => items.iter().all(|item| match_pattern_is_supported(&item.node)),
    }
}

/// Name the reason Body IR cannot bind `pattern` against a produced item of type `item_ty`, or `None` when it
/// can. Consulted once, up front, so a refusal never leaves half-emitted bindings behind -- the same precedent as
/// [`match_pattern_is_supported`].
///
/// Two independent things can make a loop pattern unbindable, and both are checked here.
///
/// **Shape.** The accepted subset is deliberately the same one `TypeChecker::define_for_pattern_bindings`
/// (`src/frontend/typechecker/check_stmt.rs`) accepts -- a plain binding, `_`, and recursively a tuple of those
/// (#1125). Naming the offending shape keeps a hand-built AST that bypassed the typechecker diagnosable.
///
/// **Type agreement.** A tuple pattern can only take elements from a tuple. Without this check, `for a, b in
/// items` over a `list[int]` would lower `.0`/`.1` projections out of an `int` -- structurally valid Body IR
/// describing something that does not exist. The typechecker rejects that program first, so this is defence in
/// depth for hand-built ASTs and for lowering that runs despite type errors, not the primary diagnostic.
///
/// Two item types are exempt from the tuple requirement, mirroring `TypeChecker::define_for_pattern_bindings`
/// exactly so the two stages cannot disagree about which programs are bindable.
/// [`IncanType::Unknown`] is recovery-only: it means the type is unresolved, not proven non-tuple, so each element
/// binds as `Unknown` just as [`tuple_element_types`] already falls back to. [`IncanType::Never`] is the bottom
/// type, which the typechecker's own `types_compatible` treats as compatible with every type including a tuple.
///
/// A bare [`IncanType::TypeVar`] is deliberately **not** exempt. An unconstrained `T` is known to be
/// underdetermined rather than merely unknown, and can be instantiated as `int`; Incan has no tuple-shaped bound
/// that could promise otherwise. This does not affect the common `list[Tuple[K, V]]` shape, whose item type is a
/// tuple whose *elements* are type variables.
fn unsupported_for_pattern(pattern: &ast::Pattern, item_ty: &IncanType) -> Option<String> {
    match pattern {
        ast::Pattern::Binding(_) | ast::Pattern::Wildcard => None,
        ast::Pattern::Tuple(items) => {
            if matches!(item_ty, IncanType::Unknown | IncanType::Never) {
                return items
                    .iter()
                    .find_map(|item| unsupported_for_pattern(&item.node, &IncanType::Unknown));
            }
            let Some(element_types) = tuple_type_elements(item_ty) else {
                return Some(format!("for-loop tuple pattern over non-tuple item type `{item_ty}`"));
            };
            if element_types.len() != items.len() {
                return Some(format!(
                    "for-loop tuple pattern binds {} names but item type `{item_ty}` has {} elements",
                    items.len(),
                    element_types.len()
                ));
            }
            items
                .iter()
                .zip(element_types)
                .find_map(|(item, element_ty)| unsupported_for_pattern(&item.node, element_ty))
        }
        ast::Pattern::Literal(_) => Some("for-loop pattern shape: literal".to_string()),
        ast::Pattern::Constructor(..) => Some("for-loop pattern shape: constructor".to_string()),
        ast::Pattern::Group(_) => Some("for-loop pattern shape: parenthesized group".to_string()),
        ast::Pattern::Or(_) => Some("for-loop pattern shape: alternation".to_string()),
    }
}

/// Count `name` occurrences in one call argument's expression, regardless of whether the argument is positional,
/// named, or an unpack — the read-count approximation counts the expression either way even though
/// [`BodyBuilder::lower_positional_args`] itself rejects named/unpack arguments during real lowering.
fn count_reads_in_call_arg(name: &str, arg: &ast::CallArg) -> usize {
    match arg {
        ast::CallArg::Positional(e)
        | ast::CallArg::Named(_, e)
        | ast::CallArg::PositionalUnpack(e)
        | ast::CallArg::KeywordUnpack(e) => count_reads_in_expr(name, &e.node),
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
                1152,
                "Body IR carries the source contract; direct local callable targets remain visibly refused until the callable runtime slice executes them.",
                "src/frontend/typechecker/check_expr/calls.rs",
                "fn check_call",
                "fn lower_call",
                "fn execute_call",
            ),
            planned_feature_at_boundary(
                "call.stored-callables",
                "Stored closures and partials retain lexical capture timing, ownership, and isolated local call frames.",
                1152,
                "Direct execution deliberately refuses local callable targets; this is the coherent callable-frame profile.",
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

mod free_vars;

use free_vars::{
    count_reads_in_generator_deferred_body, free_vars_in_closure_body, free_vars_in_generator_deferred_body,
};

#[cfg(test)]
mod tests;
