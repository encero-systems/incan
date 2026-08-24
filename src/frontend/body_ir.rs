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
//! field access, indexing, slicing, parenthesization, tuples, list/dict/set literals (no spreads), `model`/`class`
//! construction (named-only at the source level, bound to declared field order -- see
//! [`BodyBuilder::lower_nominal_construction`]), expression-position `if`/`loop`, `try` (`?`), f-strings,
//! list/dict comprehensions, lazy generator expressions, closure literals, partial callables (see
//! [`BodyBuilder::lower_closure`]/[`BodyBuilder::lower_partial`] for how captures
//! are computed and represented explicitly rather than left implicit), and `match` (see [`BodyBuilder::lower_match`]
//! for how patterns are lowered and their bindings scoped).
//!
//! Everything else lowers to an explicit `Statement::Unsupported` / `Operand::Unknown` node rather than panicking,
//! so the model stays total over real programs. That residue is not a short tail, and #1101 tracks it as named
//! remaining work rather than as an implied "almost everything" claim: spread call arguments (`f(*xs)`, `f(**kw)`,
//! `Ctor(**kw)`) and spread entries in list/dict literals; the `**`, bitwise, shift, `in`/`not
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
use crate::frontend::typechecker::{IdentKind, TypeCheckInfo, semantic_type_from_resolved};

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
    let bodies = program
        .declarations
        .iter()
        .flat_map(|decl| -> Vec<bir::Body> {
            match &decl.node {
                ast::Declaration::Function(function) => {
                    vec![lower_function_body(
                        function,
                        decl.span,
                        &module_identity,
                        type_info,
                        &function_default_sources,
                        &local_function_declarations,
                        &local_nominal_declarations,
                        &local_fieldless_enum_declarations,
                        &local_value_enum_declarations,
                    )]
                }
                ast::Declaration::Model(model) => lower_owner_method_bodies(
                    &model.methods,
                    &model.name,
                    owner_self_type(&model.name, &model.type_params),
                    &module_identity,
                    type_info,
                    &function_default_sources,
                    &local_function_declarations,
                    &local_nominal_declarations,
                    &local_fieldless_enum_declarations,
                    &local_value_enum_declarations,
                ),
                ast::Declaration::Class(class) => lower_owner_method_bodies(
                    &class.methods,
                    &class.name,
                    owner_self_type(&class.name, &class.type_params),
                    &module_identity,
                    type_info,
                    &function_default_sources,
                    &local_function_declarations,
                    &local_nominal_declarations,
                    &local_fieldless_enum_declarations,
                    &local_value_enum_declarations,
                ),
                ast::Declaration::Trait(trait_decl) => lower_owner_method_bodies(
                    &trait_decl.methods,
                    &trait_decl.name,
                    IncanType::SelfType,
                    &module_identity,
                    type_info,
                    &function_default_sources,
                    &local_function_declarations,
                    &local_nominal_declarations,
                    &local_fieldless_enum_declarations,
                    &local_value_enum_declarations,
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
    module_identity: &str,
    type_info: &TypeCheckInfo,
    function_default_sources: &FunctionDefaultSources,
    local_function_declarations: &LocalFunctionDeclarations,
    local_nominal_declarations: &LocalNominalDeclarations,
    local_fieldless_enum_declarations: &LocalFieldlessEnumDeclarations,
    local_value_enum_declarations: &LocalValueEnumDeclarations,
) -> Vec<bir::Body> {
    methods
        .iter()
        .filter_map(|method| {
            lower_method_body(
                &method.node,
                method.span,
                owner_name,
                &receiver_ty,
                module_identity,
                type_info,
                function_default_sources,
                local_function_declarations,
                local_nominal_declarations,
                local_fieldless_enum_declarations,
                local_value_enum_declarations,
            )
        })
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
    module_identity: &str,
    type_info: &TypeCheckInfo,
    function_default_sources: &FunctionDefaultSources,
    local_function_declarations: &LocalFunctionDeclarations,
    local_nominal_declarations: &LocalNominalDeclarations,
    local_fieldless_enum_declarations: &LocalFieldlessEnumDeclarations,
    local_value_enum_declarations: &LocalValueEnumDeclarations,
) -> bir::Body {
    let decl_id = CompilerNodeId::declaration(module_identity, &function.name);
    let direct_call_id = CompilerNodeId::declaration_span(module_identity, decl_span.start, decl_span.end);
    // The bare-name map is a compatibility projection and collapses top-level overloads. A body is one physical
    // declaration, so its parameter types must come from the same span-keyed fact the direct-call identity uses.
    let binding = type_info
        .declarations
        .function_bindings_by_span
        .get(&(decl_span.start, decl_span.end));
    let owner_return_type = binding
        .map(|binding| semantic_type_from_resolved(&binding.return_type))
        .unwrap_or(IncanType::Unknown);

    let mut builder = BodyBuilder::new(
        type_info,
        function_default_sources,
        local_function_declarations,
        local_nominal_declarations,
        local_fieldless_enum_declarations,
        local_value_enum_declarations,
        module_identity,
        owner_return_type,
    );
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
    module_identity: &str,
    type_info: &TypeCheckInfo,
    function_default_sources: &FunctionDefaultSources,
    local_function_declarations: &LocalFunctionDeclarations,
    local_nominal_declarations: &LocalNominalDeclarations,
    local_fieldless_enum_declarations: &LocalFieldlessEnumDeclarations,
    local_value_enum_declarations: &LocalValueEnumDeclarations,
) -> Option<bir::Body> {
    let body_stmts = method.body.as_ref()?;

    // Method names are not unique across a module the way top-level function names are (two classes can each
    // declare a method named `new`), so the method's CompilerNodeId is scoped under its owning declaration's name
    // rather than reusing `CompilerNodeId::declaration(module_identity, &method.name)` directly.
    let decl_id = CompilerNodeId::declaration(module_identity, &format!("{owner_name}::{}", method.name));
    let direct_call_id = CompilerNodeId::declaration_span(module_identity, decl_span.start, decl_span.end);
    let binding = type_info
        .declarations
        .method_bindings_by_span
        .get(&(decl_span.start, decl_span.end));
    let owner_return_type = binding
        .map(|binding| semantic_type_from_resolved(&binding.return_type))
        .unwrap_or(IncanType::Unknown);

    let mut builder = BodyBuilder::new(
        type_info,
        function_default_sources,
        local_function_declarations,
        local_nominal_declarations,
        local_fieldless_enum_declarations,
        local_value_enum_declarations,
        module_identity,
        owner_return_type,
    );
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
    fn new(
        type_info: &'type_info TypeCheckInfo,
        function_default_sources: &'source FunctionDefaultSources,
        local_function_declarations: &'source LocalFunctionDeclarations,
        local_nominal_declarations: &'source LocalNominalDeclarations,
        local_fieldless_enum_declarations: &'source LocalFieldlessEnumDeclarations,
        local_value_enum_declarations: &'source LocalValueEnumDeclarations,
        module_identity: &'source str,
        owner_return_type: IncanType,
    ) -> Self {
        Self {
            type_info,
            function_default_sources,
            local_function_declarations,
            local_nominal_declarations,
            local_fieldless_enum_declarations,
            local_value_enum_declarations,
            module_identity,
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
        args: Vec<bir::Operand>,
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
                    args: vec![bir::Operand::place(iterable_place, bir::OwnershipFact::Borrow, false)],
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
                rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Dict, Vec::new()),
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
                    args: vec![bir::Operand::place(
                        bir::Place::from_local(source_local),
                        bir::OwnershipFact::Borrow,
                        false,
                    )],
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
                        args: vec![
                            bir::Operand::place(
                                bir::Place::from_local(*list_local),
                                bir::OwnershipFact::MutBorrow,
                                false,
                            ),
                            element_operand,
                        ],
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
                        args: vec![
                            bir::Operand::place(
                                bir::Place::from_local(*dict_local),
                                bir::OwnershipFact::MutBorrow,
                                false,
                            ),
                            key_operand,
                            value_operand,
                        ],
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
            ast::Expr::List(entries) => {
                if entries.iter().any(|entry| matches!(entry, ast::ListEntry::Spread(_))) {
                    return self.unsupported_operand("list spread entries".to_string(), scope, span, out);
                }
                let items: Vec<ast::Spanned<ast::Expr>> = entries
                    .iter()
                    .map(|entry| match entry {
                        ast::ListEntry::Element(e) => e.clone(),
                        ast::ListEntry::Spread(_) => unreachable!("spread entries filtered out above"),
                    })
                    .collect();
                self.lower_aggregate(bir::AggregateKind::List, &items, expr.span, scope, out)
            }
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
                vec![lhs_operand, rhs_operand],
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

    /// Lower every argument in `args` to an operand, or return `None` if any argument is named or an unpack
    /// (`*args`/`**kwargs`) — v0's [`bir::Callee`] call shape only models positional arguments, so callers use the
    /// `None` case to fall back to an explicit unsupported placeholder instead of dropping the extra arguments.
    fn lower_positional_args(
        &mut self,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<Vec<bir::Operand>> {
        let mut operands = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) => operands.push(self.lower_expr_to_operand(expr, scope, out)),
                ast::CallArg::Named(_, _) | ast::CallArg::PositionalUnpack(_) | ast::CallArg::KeywordUnpack(_) => {
                    return None;
                }
            }
        }
        Some(operands)
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
    /// declared slot, so those calls keep lowering their positional arguments — refusing them would drop a delivered
    /// language capability — but record [`bir::ArgumentBinding::UnresolvedPositional`] rather than a slot map this
    /// stage did not compute. A named or spread argument against such a signature is still refused: binding a name
    /// into a rest parameter is variadic-binding work this issue does not own.
    fn bind_declared_args(
        &mut self,
        callee: &str,
        declared: Option<Vec<DeclaredSlot>>,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::Operand>, bir::ArgumentBinding), String> {
        let has_rest = declared
            .as_ref()
            .is_some_and(|slots| slots.iter().any(|slot| slot.is_rest));
        let fixed_slots = declared.filter(|_| !has_rest);
        let Some(slots) = fixed_slots else {
            let reason = if has_rest {
                "a rest parameter"
            } else {
                "no resolved signature"
            };
            let Some(operands) = self.lower_positional_args(args, scope, out) else {
                return Err(format!("{callee} called with named or spread arguments and {reason}"));
            };
            return Ok((operands, bir::ArgumentBinding::UnresolvedPositional));
        };
        let planned = plan_declared_args(callee, &slots, args)?;
        self.lower_planned_args(&planned, slots.len(), scope, out)
            .map_err(|description| format!("{callee}: {description}"))
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
                operands,
            ),
            ty,
            scope,
            hir_span_value,
            out,
        )
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
    /// (notably a builtin) still lowers its positional arguments faithfully, and refuses only the named or spread
    /// spellings it cannot bind without one. Non-identifier callees and argument spreads remain explicit unsupported
    /// forms; v0 has no dynamic-call-target or variable-arity argument node for them yet (#1159 owns spreads).
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
        // distinction explicitly: a same-spelled local function or imported target must remain on the normal call
        // path and refuse unless its own direct callable facts are available. The direct runtime never resolves a
        // constructor name dynamically.
        if !self.local_function_declarations.contains_key(&name)
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
            return self.push_call_temp(callee, operands, ty, scope, hir_span_value, false, out);
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
    /// cannot bind, matching [`Self::lower_call`]'s treatment of an unresolved direct callee.
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

        let mut call_args = Vec::with_capacity(arg_operands.len() + 1);
        call_args.push(receiver_operand);
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

    /// Lower a tuple, (non-spread) list literal, or set literal to a [`bir::Rvalue::Aggregate`], recording an
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
        self.push_assign_temp(bir::Rvalue::Aggregate(kind, operands), ty, scope, hir_span_value, out)
    }

    /// Lower a dict literal `{k: v, ...}` to a [`bir::Rvalue::Aggregate`] with [`bir::AggregateKind::Dict`], whose
    /// operand vector alternates key, value, key, value, ... in source order (see that variant's own docs for the
    /// invariant). A spread entry (`{**other}`) -- not yet modeled by v0, matching how a list literal's spread
    /// entries are also unsupported in [`Self::lower_expr_to_operand`] -- lowers to an explicit unsupported
    /// placeholder instead.
    fn lower_dict(
        &mut self,
        entries: &[ast::DictEntry],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        if entries.iter().any(|entry| matches!(entry, ast::DictEntry::Spread(_))) {
            return self.unsupported_operand("dict spread entries".to_string(), scope, hir_span_value, out);
        }
        let mut operands = Vec::with_capacity(entries.len() * 2);
        for entry in entries {
            let ast::DictEntry::Pair(key, value) = entry else {
                return self.unsupported_operand("dict spread entries".to_string(), scope, hir_span_value, out);
            };
            operands.push(self.lower_expr_to_operand(key, scope, out));
            operands.push(self.lower_expr_to_operand(value, scope, out));
        }
        let ty = self.resolve_ty(span);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.push_assign_temp(
            bir::Rvalue::Aggregate(bir::AggregateKind::Dict, operands),
            ty,
            scope,
            hir_span_value,
            out,
        )
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
            call_args,
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

/// Determine every lexical free variable used after a generator expression's first source, in first-occurrence
/// order. The initial source is evaluated before constructing [`bir::Rvalue::Generator`] and therefore is not a
/// deferred capture. Each `for` pattern becomes bound only after its own source expression has been visited, so a
/// later source/filter/element sees every preceding clause binding but not a name it introduces itself.
fn free_vars_in_generator_deferred_body(generator: &ast::GeneratorExpr) -> Vec<String> {
    let Some((first, remaining)) = generator.clauses.split_first() else {
        return Vec::new();
    };
    let mut bound = HashSet::new();
    if let ast::ComprehensionClause::For { pattern, .. } = first {
        bind_pattern_names(&pattern.node, &mut bound);
    }
    let mut free = Vec::new();
    for clause in remaining {
        match clause {
            ast::ComprehensionClause::For { pattern, iter } => {
                collect_free_vars_in_expr(&iter.node, &mut bound, &mut free);
                bind_pattern_names(&pattern.node, &mut bound);
            }
            ast::ComprehensionClause::If(condition) => {
                collect_free_vars_in_expr(&condition.node, &mut bound, &mut free);
            }
        }
    }
    collect_free_vars_in_expr(&generator.expr.node, &mut bound, &mut free);
    free
}

/// Count a generator capture's deferred reads after the first source. This intentionally remains a conservative
/// source-order over-approximation, like [`count_reads_in_expr`]: a later pattern can shadow the same spelling and
/// leave this count high, which selects a clone rather than an unsound move in the generator-local body.
fn count_reads_in_generator_deferred_body(name: &str, generator: &ast::GeneratorExpr) -> usize {
    let Some((_, remaining)) = generator.clauses.split_first() else {
        return 0;
    };
    count_reads_in_comprehension_clauses(name, remaining) + count_reads_in_expr(name, &generator.expr.node)
}

/// Determine every free variable a closure literal's body reads from its enclosing scope, in first-occurrence
/// source order, given the closure's own declared parameters as the initial bound set. A "free variable" is any
/// `Ident` read the closure body itself does not bind -- exactly the set [`BodyBuilder::lower_closure`] must
/// capture before lowering the body, so each one gets its own explicit Duckborrower read at the point the closure
/// is constructed (see this module's docs on why Body IR cannot rely on a target backend's own closure syntax to
/// auto-capture the way the existing Rust-emission backend does).
fn free_vars_in_closure_body(params: &[ast::Spanned<ast::Param>], body: &ast::Spanned<ast::Expr>) -> Vec<String> {
    let mut bound: HashSet<String> = params.iter().map(|p| p.node.name.clone()).collect();
    let mut free = Vec::new();
    collect_free_vars_in_expr(&body.node, &mut bound, &mut free);
    free
}

/// Record `name` in `free` (in first-occurrence order, deduplicated) unless it is already in `bound`.
fn push_free(name: &str, bound: &HashSet<String>, free: &mut Vec<String>) {
    if !bound.contains(name) && !free.iter().any(|existing| existing == name) {
        free.push(name.to_string());
    }
}

/// Collect every name `pattern` binds into `bound`, recursing into every sub-pattern shape.
///
/// Used by [`collect_free_vars_in_expr`] to exclude a pattern's own bound names from the free variables an
/// enclosing closure must capture, for every construct that binds through a pattern: `match` arms, `for` loops,
/// comprehension/generator `for` clauses, and `if let`/`while let` conditions. A single recursive walk serves all
/// of them because a `for` pattern can now bind more than one name too (#1125) -- a flat "only a plain
/// [`ast::Pattern::Binding`] binds here" walk would leave a destructured loop binding looking free, and an
/// enclosing closure would wrongly capture it.
///
/// This mirrors [`BodyBuilder::lower_match_pattern`]'s and [`BodyBuilder::bind_for_pattern_fields`]' binding walks
/// in spirit, though it only needs the names, not the locals/ownership facts those walks build.
fn bind_pattern_names(pattern: &ast::Pattern, bound: &mut HashSet<String>) {
    match pattern {
        ast::Pattern::Wildcard | ast::Pattern::Literal(_) => {}
        ast::Pattern::Binding(name) => {
            bound.insert(name.clone());
        }
        ast::Pattern::Tuple(items) => {
            for item in items {
                bind_pattern_names(&item.node, bound);
            }
        }
        ast::Pattern::Constructor(_, args) => {
            for arg in args {
                match arg {
                    ast::PatternArg::Positional(pat) | ast::PatternArg::Named(_, pat) => {
                        bind_pattern_names(&pat.node, bound);
                    }
                }
            }
        }
        ast::Pattern::Group(inner) => bind_pattern_names(&inner.node, bound),
        ast::Pattern::Or(items) => {
            for item in items {
                bind_pattern_names(&item.node, bound);
            }
        }
    }
}

/// Recursively collect free variables from an expression, given the names already bound at this point in `bound`.
/// Constructs that introduce their own bindings for a sub-expression (comprehension/`for`-clause patterns, nested
/// closures' own parameters, or a nested expression-position `if`/`loop`'s own statement-block bindings) extend a
/// *cloned* copy of `bound` before recursing into that sub-expression, so a binding introduced in one branch never
/// leaks into a sibling branch or back out to the caller -- unlike [`BodyBuilder`]'s own flat `self.bindings` map,
/// which this analysis runs entirely independently of (see [`free_vars_in_closure_body`]'s docs).
fn collect_free_vars_in_expr(expr: &ast::Expr, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    match expr {
        ast::Expr::Ident(name) => push_free(name, bound, free),
        ast::Expr::Binary(l, _, r) => {
            collect_free_vars_in_expr(&l.node, bound, free);
            collect_free_vars_in_expr(&r.node, bound, free);
        }
        ast::Expr::Unary(_, e) | ast::Expr::Paren(e) | ast::Expr::Try(e) => {
            collect_free_vars_in_expr(&e.node, bound, free)
        }
        ast::Expr::Call(callee, _, args) => {
            collect_free_vars_in_expr(&callee.node, bound, free);
            for arg in args {
                collect_free_vars_in_call_arg(arg, bound, free);
            }
        }
        ast::Expr::MethodCall(recv, _, _, args) => {
            collect_free_vars_in_expr(&recv.node, bound, free);
            for arg in args {
                collect_free_vars_in_call_arg(arg, bound, free);
            }
        }
        ast::Expr::Field(e, _) => collect_free_vars_in_expr(&e.node, bound, free),
        ast::Expr::Index(e, idx) => {
            collect_free_vars_in_expr(&e.node, bound, free);
            collect_free_vars_in_expr(&idx.node, bound, free);
        }
        ast::Expr::Slice(base, slice) => {
            collect_free_vars_in_expr(&base.node, bound, free);
            for component in [&slice.start, &slice.end, &slice.step].into_iter().flatten() {
                collect_free_vars_in_expr(&component.node, bound, free);
            }
        }
        ast::Expr::Tuple(items) | ast::Expr::Set(items) => {
            for item in items {
                collect_free_vars_in_expr(&item.node, bound, free);
            }
        }
        ast::Expr::List(entries) => {
            for entry in entries {
                match entry {
                    ast::ListEntry::Element(e) | ast::ListEntry::Spread(e) => {
                        collect_free_vars_in_expr(&e.node, bound, free)
                    }
                }
            }
        }
        ast::Expr::Dict(entries) => {
            for entry in entries {
                match entry {
                    ast::DictEntry::Pair(k, v) => {
                        collect_free_vars_in_expr(&k.node, bound, free);
                        collect_free_vars_in_expr(&v.node, bound, free);
                    }
                    ast::DictEntry::Spread(e) => collect_free_vars_in_expr(&e.node, bound, free),
                }
            }
        }
        ast::Expr::Constructor(_, args) => {
            for arg in args {
                collect_free_vars_in_call_arg(arg, bound, free);
            }
        }
        ast::Expr::Range { start, end, .. } => {
            collect_free_vars_in_expr(&start.node, bound, free);
            collect_free_vars_in_expr(&end.node, bound, free);
        }
        ast::Expr::FString(parts) => {
            for part in parts {
                if let ast::FStringPart::Expr { expr, .. } = part {
                    collect_free_vars_in_expr(&expr.node, bound, free);
                }
            }
        }
        ast::Expr::If(if_expr) => {
            collect_free_vars_in_expr(&if_expr.condition.node, bound, free);
            let mut then_bound = bound.clone();
            collect_free_vars_in_stmts(&if_expr.then_body, &mut then_bound, free);
            if let Some(else_body) = &if_expr.else_body {
                let mut else_bound = bound.clone();
                collect_free_vars_in_stmts(else_body, &mut else_bound, free);
            }
        }
        ast::Expr::Loop(loop_expr) => {
            let mut loop_bound = bound.clone();
            collect_free_vars_in_stmts(&loop_expr.body, &mut loop_bound, free);
        }
        ast::Expr::ListComp(comp) => {
            collect_free_vars_in_expr(&comp.iter.node, bound, free);
            let mut inner_bound = bound.clone();
            bind_pattern_names(&comp.pattern.node, &mut inner_bound);
            if let Some(filter) = &comp.filter {
                collect_free_vars_in_expr(&filter.node, &mut inner_bound, free);
            }
            collect_free_vars_in_expr(&comp.expr.node, &mut inner_bound, free);
        }
        ast::Expr::DictComp(comp) => {
            collect_free_vars_in_expr(&comp.iter.node, bound, free);
            let mut inner_bound = bound.clone();
            bind_pattern_names(&comp.pattern.node, &mut inner_bound);
            if let Some(filter) = &comp.filter {
                collect_free_vars_in_expr(&filter.node, &mut inner_bound, free);
            }
            collect_free_vars_in_expr(&comp.key.node, &mut inner_bound, free);
            collect_free_vars_in_expr(&comp.value.node, &mut inner_bound, free);
        }
        ast::Expr::Generator(generator) => {
            let mut inner_bound = bound.clone();
            for clause in &generator.clauses {
                match clause {
                    ast::ComprehensionClause::For { pattern, iter } => {
                        collect_free_vars_in_expr(&iter.node, &mut inner_bound, free);
                        bind_pattern_names(&pattern.node, &mut inner_bound);
                    }
                    ast::ComprehensionClause::If(cond) => collect_free_vars_in_expr(&cond.node, &mut inner_bound, free),
                }
            }
            collect_free_vars_in_expr(&generator.expr.node, &mut inner_bound, free);
        }
        ast::Expr::Closure(params, body) => {
            let mut inner_bound = bound.clone();
            for param in params {
                inner_bound.insert(param.node.name.clone());
            }
            collect_free_vars_in_expr(&body.node, &mut inner_bound, free);
        }
        ast::Expr::Partial(partial) => {
            collect_free_vars_in_expr(&partial.target.node, bound, free);
            for arg in &partial.args {
                collect_free_vars_in_expr(&arg.value.node, bound, free);
            }
        }
        // Mirrors `count_reads_in_expr`'s `Yield` arm: a yielded value is an ordinary nested expression for
        // free-variable purposes, so a name it reads from an enclosing closure scope must still be captured.
        ast::Expr::Yield(Some(value)) => collect_free_vars_in_expr(&value.node, bound, free),
        // The scrutinee is read in the enclosing scope like any other sub-expression. Each arm gets its own
        // *cloned* `bound` set (matching the `If`/`Loop` arms above) extended with that arm's own pattern-bound
        // names before walking its guard and body, so one arm's bindings never leak into a sibling arm or shadow an
        // outer free variable of the same name.
        ast::Expr::Match(subject, arms) => {
            collect_free_vars_in_expr(&subject.node, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                bind_pattern_names(&arm.node.pattern.node, &mut arm_bound);
                if let Some(guard) = &arm.node.guard {
                    collect_free_vars_in_expr(&guard.node, &mut arm_bound, free);
                }
                match &arm.node.body {
                    ast::MatchBody::Expr(e) => collect_free_vars_in_expr(&e.node, &mut arm_bound, free),
                    ast::MatchBody::Block(stmts) => collect_free_vars_in_stmts(stmts, &mut arm_bound, free),
                }
            }
        }
        _ => {}
    }
}

/// Collect free variables from one call argument's expression, regardless of whether it is positional, named, or an
/// unpack -- matching [`count_reads_in_call_arg`]'s own "count the expression either way" stance, even though
/// [`BodyBuilder::lower_positional_args`] itself rejects named/unpack arguments during real lowering.
fn collect_free_vars_in_call_arg(arg: &ast::CallArg, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    match arg {
        ast::CallArg::Positional(e)
        | ast::CallArg::Named(_, e)
        | ast::CallArg::PositionalUnpack(e)
        | ast::CallArg::KeywordUnpack(e) => collect_free_vars_in_expr(&e.node, bound, free),
    }
}

/// Collect free variables from an `if`/`while` condition, including the value expression of a `Condition::Let`
/// pattern condition (even though v0 lowering does not model `if let`/`while let` themselves -- see
/// [`BodyBuilder::lower_if`]/[`BodyBuilder::lower_while`]) -- a pattern-bound name still shadows an outer name of
/// the same spelling for anything nested inside the branch this condition gates, so it is bound defensively here
/// even though the branch itself lowers to `Unsupported`.
fn collect_free_vars_in_condition(cond: &ast::Condition, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    match cond {
        ast::Condition::Expr(e) => collect_free_vars_in_expr(&e.node, bound, free),
        ast::Condition::Let { pattern, value } => {
            collect_free_vars_in_expr(&value.node, bound, free);
            bind_pattern_names(&pattern.node, bound);
        }
    }
}

/// Collect free variables from a statement block in source order, threading a progressively-extended `bound` set
/// through each statement so a binding one statement introduces (`let`, `for`, tuple unpack, ...) is visible to
/// every later statement in the *same* block, matching ordinary lexical scoping -- and, symmetrically, does not
/// leak into a sibling block (an `if`'s `else` body, for instance), since callers always pass a freshly cloned
/// `bound` per block (see [`collect_free_vars_in_expr`]'s `If`/`Loop` arms).
fn collect_free_vars_in_stmts(
    stmts: &[ast::Spanned<ast::Statement>],
    bound: &mut HashSet<String>,
    free: &mut Vec<String>,
) {
    for stmt in stmts {
        collect_free_vars_in_stmt(&stmt.node, bound, free);
    }
}

/// Collect free variables from one statement, recursing into every statement kind [`BodyBuilder`]'s own lowering
/// walks (see this module's module-level docs for the covered subset) and extending `bound` wherever that statement
/// introduces a new binding for the remainder of its enclosing block. Statement kinds outside v0's lowered subset
/// are not walked and neither read nor bind anything this analysis needs to know about.
fn collect_free_vars_in_stmt(stmt: &ast::Statement, bound: &mut HashSet<String>, free: &mut Vec<String>) {
    match stmt {
        ast::Statement::Assignment(a) => {
            collect_free_vars_in_expr(&a.value.node, bound, free);
            bound.insert(a.name.clone());
        }
        ast::Statement::FieldAssignment(fa) => {
            collect_free_vars_in_expr(&fa.object.node, bound, free);
            collect_free_vars_in_expr(&fa.value.node, bound, free);
        }
        ast::Statement::IndexAssignment(ia) => {
            collect_free_vars_in_expr(&ia.object.node, bound, free);
            collect_free_vars_in_expr(&ia.index.node, bound, free);
            collect_free_vars_in_expr(&ia.value.node, bound, free);
        }
        ast::Statement::CompoundAssignment(ca) => {
            // A compound assignment target must already exist, so it is a read of whatever bound it (an outer
            // capture, if this statement lives inside a closure body and `ca.name` was never rebound locally), not
            // a fresh binding -- see `Self::lower_partial`'s docs for the known limitation this implies for
            // mutating a captured variable from inside a closure.
            push_free(&ca.name, bound, free);
            collect_free_vars_in_expr(&ca.value.node, bound, free);
        }
        ast::Statement::TupleUnpack(tu) => {
            collect_free_vars_in_expr(&tu.value.node, bound, free);
            for name in &tu.names {
                bound.insert(name.clone());
            }
        }
        ast::Statement::TupleAssign(ta) => {
            for target in &ta.targets {
                collect_free_vars_in_expr(&target.node, bound, free);
            }
            collect_free_vars_in_expr(&ta.value.node, bound, free);
        }
        ast::Statement::ChainedAssignment(ca) => {
            collect_free_vars_in_expr(&ca.value.node, bound, free);
            for name in &ca.targets {
                bound.insert(name.clone());
            }
        }
        ast::Statement::Return(Some(e)) => collect_free_vars_in_expr(&e.node, bound, free),
        ast::Statement::Return(None) => {}
        ast::Statement::If(if_stmt) => {
            collect_free_vars_in_condition(&if_stmt.condition, bound, free);
            let mut then_bound = bound.clone();
            collect_free_vars_in_stmts(&if_stmt.then_body, &mut then_bound, free);
            for (cond, body) in &if_stmt.elif_branches {
                collect_free_vars_in_expr(&cond.node, bound, free);
                let mut elif_bound = bound.clone();
                collect_free_vars_in_stmts(body, &mut elif_bound, free);
            }
            if let Some(else_body) = &if_stmt.else_body {
                let mut else_bound = bound.clone();
                collect_free_vars_in_stmts(else_body, &mut else_bound, free);
            }
        }
        ast::Statement::While(w) => {
            collect_free_vars_in_condition(&w.condition, bound, free);
            let mut loop_bound = bound.clone();
            collect_free_vars_in_stmts(&w.body, &mut loop_bound, free);
        }
        ast::Statement::For(f) => {
            collect_free_vars_in_expr(&f.iter.node, bound, free);
            let mut loop_bound = bound.clone();
            bind_pattern_names(&f.pattern.node, &mut loop_bound);
            collect_free_vars_in_stmts(&f.body, &mut loop_bound, free);
        }
        ast::Statement::Expr(e) => collect_free_vars_in_expr(&e.node, bound, free),
        ast::Statement::Assert(a) => {
            if let ast::AssertKind::Condition(e) = &a.kind {
                collect_free_vars_in_expr(&e.node, bound, free);
            }
            if let Some(message) = &a.message {
                collect_free_vars_in_expr(&message.node, bound, free);
            }
        }
        ast::Statement::Break(Some(e)) => collect_free_vars_in_expr(&e.node, bound, free),
        _ => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::typechecker::TypeChecker;
    use crate::frontend::{lexer, parser};

    fn build(source: &str, module_path: &[&str]) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
    }

    /// Lower an intentionally-invalid source program after recording its typecheck diagnostics.
    ///
    /// Positive coverage must go through [`build`], which requires ordinary typechecking. This helper is only for
    /// Body IR's fail-closed assertions: after the source checker correctly rejects a program, lowering must still
    /// make its unsupported representation explicit rather than approximating it.
    fn build_after_expected_typecheck_errors(
        source: &str,
        module_path: &[&str],
    ) -> Result<(bir::BodyIrModule, Vec<String>), Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        let diagnostics = checker
            .check_program(&program)
            .err()
            .ok_or("expected the intentionally invalid source program to produce a diagnostic")?
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        Ok((
            build_body_ir_module_v0(&program, &module_path, checker.type_info()),
            diagnostics,
        ))
    }

    /// Build a Body IR module from `source` after rewriting its first `for a, b in ...:` header into the nested
    /// `for a, (b, c) in ...:` shape the parser has no spelling for (see
    /// `nested_tuple_for_patterns_have_no_source_spelling_yet`). The rewrite happens *before* typechecking, so the
    /// nested pattern flows through `TypeChecker::define_for_pattern_bindings`' own recursion and reaches lowering
    /// with real resolved element types, exactly as a future parser-supported nesting would.
    fn build_with_nested_for_pattern(
        source: &str,
        module_path: &[&str],
    ) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let mut program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

        let for_stmt = program
            .declarations
            .iter_mut()
            .find_map(|decl| match &mut decl.node {
                ast::Declaration::Function(function) => {
                    function.body.iter_mut().find_map(|stmt| match &mut stmt.node {
                        ast::Statement::For(for_stmt) => Some(for_stmt),
                        _ => None,
                    })
                }
                _ => None,
            })
            .ok_or("expected a top-level function containing a `for` statement")?;
        let ast::Pattern::Tuple(items) = &mut for_stmt.pattern.node else {
            return Err("expected a flat tuple loop pattern to nest".into());
        };
        let second = items.pop().ok_or("expected a two-item tuple loop pattern")?;
        let span = second.span;
        let third = ast::Spanned::new(ast::Pattern::Binding("c".to_string()), span);
        items.push(ast::Spanned::new(ast::Pattern::Tuple(vec![second, third]), span));

        let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
    }

    /// Build a Body IR module from `source` after rewriting its first `for x in ...:` header into a two-name tuple
    /// pattern **after** typechecking, leaving the recorded item type as the original non-tuple element type.
    ///
    /// This reaches lowering's defence-in-depth path directly: the typechecker rejects such a program
    /// (`for_pattern_expects_tuple_item`), so no ordinary `build` could ever produce this state, yet lowering must
    /// still refuse rather than project `.0`/`.1` out of a value with no such fields.
    fn build_with_for_pattern_widened_after_typecheck(
        source: &str,
        module_path: &[&str],
    ) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let mut program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

        let for_stmt = program
            .declarations
            .iter_mut()
            .find_map(|decl| match &mut decl.node {
                ast::Declaration::Function(function) => {
                    function.body.iter_mut().find_map(|stmt| match &mut stmt.node {
                        ast::Statement::For(for_stmt) => Some(for_stmt),
                        _ => None,
                    })
                }
                _ => None,
            })
            .ok_or("expected a top-level function containing a `for` statement")?;
        let span = for_stmt.pattern.span;
        let first = std::mem::replace(&mut for_stmt.pattern.node, ast::Pattern::Wildcard);
        for_stmt.pattern.node = ast::Pattern::Tuple(vec![
            ast::Spanned::new(first, span),
            ast::Spanned::new(ast::Pattern::Binding("second".to_string()), span),
        ]);

        Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
    }

    #[test]
    fn lowers_arithmetic_with_a_copy_last_use_and_a_move_return() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def add(x: int, y: int) -> int:\n  return x + y\n";
        let module = build(source, &["m", "arith"])?;
        let snapshot_first = module.render_snapshot();
        let snapshot_second = build(source, &["m", "arith"])?.render_snapshot();
        assert_eq!(snapshot_first, snapshot_second, "lowering must be deterministic");

        assert!(snapshot_first.contains("body add decl:m::arith::add"));
        assert!(snapshot_first.contains("local 0 x : int [param]"));
        assert!(snapshot_first.contains("local 1 y : int [param]"));
        // x is not the last read (y is), so x is Copy either way (int is a Copy type); both reads should be `copy`.
        assert!(snapshot_first.contains("copy(_0)"));
        assert!(snapshot_first.contains("copy(_1"));
        // `int` is a Copy-shaped type, so even a freshly created temporary reads as `copy`, not `move`.
        assert!(snapshot_first.contains("return copy(_2, last_use)"));
        Ok(())
    }

    #[test]
    fn lowers_string_concat_as_an_explicit_helper_call_with_runtime_requirements()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "def greet(name: str) -> str:\n  return \"hi \" + name\n";
        let module = build(source, &["m", "strs"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("call helper:str_concat"));
        assert!(snapshot.contains("runtime_requirements:"));
        assert!(snapshot.contains("runtime_helper(str_concat)"));
        assert!(snapshot.contains("allocator"));
        Ok(())
    }

    #[test]
    fn lowers_a_non_copy_binding_and_drops_it_when_never_moved() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def make() -> None:\n  s = \"hello\"\n  return\n";
        let module = build(source, &["m", "drop"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("local 0 s : str [binding]"));
        assert!(snapshot.contains("drop _0"));
        Ok(())
    }

    #[test]
    fn lowers_a_non_copy_binding_and_skips_the_drop_when_moved_via_return() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def make() -> str:\n  s = \"hello\"\n  return s\n";
        let module = build(source, &["m", "moved"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("return move(_0, last_use)"));
        assert!(
            !snapshot.contains("drop _0"),
            "a moved-out local must not also be dropped: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_clone_when_a_non_copy_binding_is_read_more_than_once() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def dup(s: str) -> str:\n  first = s\n  return s\n";
        let module = build(source, &["m", "clone"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("clone(_0)"),
            "the first, non-last read of `s` should clone: {snapshot}"
        );
        assert!(snapshot.contains("return move(_0, last_use)"));
        Ok(())
    }

    #[test]
    fn lowers_if_while_and_for_into_normalized_control_flow() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def run(n: int) -> int:\n  total = 0\n  for i in 0..n:\n    if i > 2:\n      total = total + i\n  while total > 100:\n    total = total - 1\n  return total\n";
        let module = build(source, &["m", "control"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("loop:"),
            "for/while should desugar to a normalized loop: {snapshot}"
        );
        assert!(snapshot.contains("if "));
        assert!(snapshot.contains("break"));
        Ok(())
    }

    #[test]
    fn lowers_division_and_assert_as_explicit_panic_facts() -> Result<(), Box<dyn std::error::Error>> {
        // Floor division keeps an `int` result (true division promotes to `float`), so this stays a same-type return.
        let source = "def div(a: int, b: int) -> int:\n  assert b != 0\n  return a // b\n";
        let module = build(source, &["m", "panics"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("panic_facts:"));
        assert!(snapshot.contains("assert_failure"));
        assert!(snapshot.contains("division_or_modulo"));
        assert!(snapshot.contains("panic_strategy"));
        Ok(())
    }

    #[test]
    fn unsupported_constructs_lower_to_an_explicit_placeholder_instead_of_panicking()
    -> Result<(), Box<dyn std::error::Error>> {
        // #1123 supports lazy generator expressions with simple binding clauses. A destructuring clause still needs
        // a generator-specific binding/poll representation, so it must refuse the complete expression rather than
        // partly lowering it as an eager list or silently dropping the pattern.
        let source = "def pick(x: int) -> int:\n  gen = (left + right for left, right in [(1, 2)])\n  return x\n";
        let module = build(source, &["m", "unsupported"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("unsupported(generator for-clause pattern is not a simple binding)"),
            "should record an explicit placeholder rather than panicking: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_an_immutable_receiver_read_through_a_field_projection() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model Counter:\n  value: int\n\n  def get(self) -> int:\n    return self.value\n";
        let module = build(source, &["m", "receiver_read"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("body get decl:m::receiver_read::Counter::get"));
        assert!(snapshot.contains("local 0 self : Counter [receiver]"));
        // `self.value` is a projected read of an `int` (Copy) field, so it reads `copy`, never `move` or `clone`.
        assert!(snapshot.contains("return copy(_0.value)"));

        Ok(())
    }

    #[test]
    fn lowers_for_over_a_builtin_list_using_the_builtin_iter_protocol() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "def total(items: list[int]) -> int:\n  mut acc = 0\n  for x in items:\n    acc = acc + x\n  return acc\n";
        let module = build(source, &["m", "builtin_for"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("iter_next(mut_borrow("),
            "builtin for should poll via IterNext: {snapshot}"
        );
        assert!(
            snapshot.contains(", builtin)"),
            "builtin collection iteration should use IterProtocol::Builtin: {snapshot}"
        );
        assert!(
            !snapshot.contains("unsupported("),
            "should not fall back to Unsupported: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn mut_self_receiver_origin_is_mutable_and_field_mutation_lowers() -> Result<(), Box<dyn std::error::Error>> {
        // `mut self` must remain a mutable receiver when its field assignment is lowered.
        let source = "model Counter:\n  value: int\n\n  def bump(mut self) -> None:\n    self.value = self.value + 1\n";
        let module = build(source, &["m", "receiver_mut"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("body bump decl:m::receiver_mut::Counter::bump"));
        assert!(snapshot.contains("local 0 self : Counter [receiver_mut]"));
        assert!(
            !snapshot.contains("unsupported("),
            "mutable receiver field assignment should lower without a placeholder: {snapshot}"
        );

        Ok(())
    }

    #[test]
    fn for_pattern_bindings_do_not_escape_the_loop_scope() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def keep_outer(x: int, items: list[int]) -> int:\n  for x in items:\n    pass\n  return x\n";
        let module = build(source, &["m", "for_scope"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("return copy(_0)"),
            "the trailing read must resolve the enclosing parameter, not the for-pattern local: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_for_over_a_user_defined_iteration_protocol() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model CounterIter:\n  value: int\n  limit: int\n\n  def __next__(self) -> Option[int]:\n    if self.value < self.limit:\n      return Some(self.value)\n    return None\n\nmodel Counter:\n  limit: int\n\n  def __iter__(self) -> CounterIter:\n    return CounterIter(value=0, limit=self.limit)\n\ndef total() -> int:\n  mut acc = 0\n  for item in Counter(limit=3):\n    acc = acc + item\n  return acc\n";
        let module = build(source, &["m", "protocol_for"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("call method:__iter__"),
            "should call the resolved __iter__ method to obtain an iterator: {snapshot}"
        );
        assert!(
            snapshot.contains("user_defined(__next__)"),
            "should poll via the resolved __next__ method, non-fallible: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_fallible_for_iteration_with_an_implicit_try_propagate_semantic() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = "model ChunkStream:\n  def __iter__(self) -> ChunkStream:\n    return self\n\n  def __next__(self) -> Result[Option[int], str]:\n    return Ok(None)\n\ndef total() -> Result[int, str]:\n  mut acc = 0\n  for chunk in ChunkStream()?:\n    acc = acc + chunk\n  return Ok(acc)\n";
        let module = build(source, &["m", "fallible_for"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("user_defined(__next__, fallible)"),
            "fallible protocol iteration should mark IterNext as fallible: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_list_comprehension_into_a_push_loop() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def doubled(items: list[int]) -> list[int]:\n  return [x * 2 for x in items]\n";
        let module = build(source, &["m", "list_comp"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("list[]"),
            "should start from an empty list aggregate: {snapshot}"
        );
        assert!(
            snapshot.contains("call method:push unbound(mut_borrow("),
            "should grow the list via a synthesized push call: {snapshot}"
        );
        assert!(
            snapshot.contains("iter_next("),
            "should desugar into the shared iteration primitive: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_filtered_list_comprehension_with_a_guarding_if() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def evens(items: list[int]) -> list[int]:\n  return [x for x in items if x % 2 == 0]\n";
        let module = build(source, &["m", "list_comp_filter"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("call method:push unbound("),
            "filtered comprehension should still push accepted elements: {snapshot}"
        );
        assert!(
            snapshot.contains("if "),
            "the filter clause should lower to a guarding If: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn comprehension_bindings_do_not_escape_the_expression_scope() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "def keep_outer(x: int, items: list[int]) -> int:\n  doubled = [x * 2 for x in items]\n  return x\n";
        let module = build(source, &["m", "comprehension_scope"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("return copy(_0)"),
            "the trailing read must resolve the enclosing parameter, not the comprehension binding: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_dict_comprehension_into_an_insert_loop() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def doubled(items: list[int]) -> dict[int, int]:\n  return {x: x * 2 for x in items}\n";
        let module = build(source, &["m", "dict_comp"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("dict[]"),
            "should start from an empty dict aggregate: {snapshot}"
        );
        assert!(
            snapshot.contains("call method:insert unbound(mut_borrow("),
            "should grow the dict via a synthesized insert call: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn generator_expression_keeps_its_multi_clause_body_lazy_and_captures_its_environment()
    -> Result<(), Box<dyn std::error::Error>> {
        // Mirrors the multi-clause fixture from `test_rfc006_generator_expression_infers_element_type` in
        // `src/frontend/typechecker/tests.rs`, but also reads `offset` from both the filter and element. The Body IR
        // value must capture that enclosing local once at construction; it must not materialize the chain or run
        // either filter/element in the enclosing body.
        let source = "def positives(offset: int, xs: list[int], ys: list[int]) -> Generator[int]:\n  return (x * offset for x in xs if x > offset for y in ys if y > x)\n";
        let module = build(source, &["m", "generator_expr"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("generator(source="),
            "generator construction must be represented as a distinct lazy rvalue: {snapshot}"
        );
        assert!(
            snapshot.contains("captures=["),
            "the deferred body must receive explicit construction-time captures: {snapshot}"
        );
        assert!(
            !snapshot.contains("list[]"),
            "a generator expression must not materialize an eager list while claiming Generator[T]: {snapshot}"
        );
        assert!(
            snapshot.contains("yield "),
            "the element must be suspended in the generator body: {snapshot}"
        );
        assert!(
            snapshot.contains("iter_next("),
            "for clauses must remain deferred iteration operations: {snapshot}"
        );
        assert!(
            snapshot.contains("if "),
            "filters must remain deferred guard operations: {snapshot}"
        );
        assert!(
            !snapshot.contains("unsupported("),
            "a valid generator expression must not leave an unsupported placeholder: {snapshot}"
        );
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "positives")
            .ok_or("generator fixture must lower its function body")?;
        assert!(
            body.block.stmts.iter().all(|statement| !matches!(
                statement.kind,
                bir::StatementKind::IterNext { .. } | bir::StatementKind::Yield { .. }
            )),
            "polling and yield must stay inside the generator rvalue, not the enclosing body: {snapshot}"
        );
        let (source, captured_operands, generator_body) = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Assign {
                    rvalue:
                        bir::Rvalue::Generator {
                            source,
                            captured_operands,
                            body,
                        },
                    ..
                } => Some((source, captured_operands, body)),
                _ => None,
            })
            .ok_or("generator fixture must assign a Generator rvalue")?;
        assert!(
            matches!(source, bir::Operand::Place(_)),
            "the first for source must be captured as a construction-time operand: {source:?}"
        );
        assert_eq!(
            captured_operands.len(),
            2,
            "offset and ys are the deferred free captures"
        );
        let capture_names: Vec<_> = generator_body
            .capture_locals
            .iter()
            .map(|local| body.locals[local.index()].name.as_deref())
            .collect();
        assert_eq!(capture_names, vec![Some("offset"), Some("ys")]);
        assert!(
            matches!(
                body.locals[generator_body.source_local.index()].origin,
                bir::LocalOrigin::Captured
            ),
            "the construction-time source needs a generator-owned local"
        );
        assert!(
            generator_body
                .capture_locals
                .iter()
                .all(|local| matches!(body.locals[local.index()].origin, bir::LocalOrigin::Captured)),
            "each deferred free value must bind through an explicit captured local"
        );
        Ok(())
    }

    #[test]
    fn generator_expression_evaluates_only_its_outer_source_before_construction()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = concat!(
            "def source() -> list[int]:\n",
            "  return [1, 2]\n\n",
            "def lazy() -> Generator[int]:\n",
            "  return (item for item in source())\n"
        );
        let module = build(source, &["m", "generator_source_timing"])?;
        let snapshot = module.render_snapshot();
        let source_call = snapshot
            .find("call fn:source()")
            .ok_or("outer generator source call must lower at construction")?;
        let generator = snapshot
            .find("generator(source=")
            .ok_or("generator construction must have a distinct rvalue")?;
        assert!(
            source_call < generator,
            "the first for source must be evaluated before generator construction: {snapshot}"
        );
        assert!(
            !snapshot.contains("unsupported("),
            "a supported outer source must not leave an unsupported marker: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn generator_expression_captures_an_outer_value_without_leaking_its_clause_binding()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = concat!(
            "def preserve(prefix: str, values: list[str]) -> str:\n",
            "  generated = (prefix + value for value in values)\n",
            "  return prefix\n"
        );
        let module = build(source, &["m", "generator_capture_scope"])?;
        let snapshot = module.render_snapshot();
        assert!(
            snapshot.contains("captures=[clone(_0)"),
            "the generator must own a construction-time clone while the enclosing binding remains live: {snapshot}"
        );
        assert!(
            snapshot.contains("return move(_0, last_use)"),
            "the trailing source read must resolve the outer prefix, not a generator-local capture: {snapshot}"
        );
        assert!(
            !snapshot.contains("unsupported("),
            "captured generator values must lower without an unsupported placeholder: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_dict_literal_as_a_dict_aggregate_with_paired_operands() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def make() -> dict[str, int]:\n  return {\"a\": 1, \"b\": 2}\n";
        let module = build(source, &["m", "dict_lit"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("dict[const(\"a\"): const(1), const(\"b\"): const(2)]"),
            "dict aggregate should render key/value pairs: {snapshot}"
        );
        assert!(snapshot.contains("allocator"));
        Ok(())
    }

    #[test]
    fn lowers_a_set_literal_as_a_set_aggregate() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def make() -> set[str]:\n  return {\"a\", \"b\"}\n";
        let module = build(source, &["m", "set_lit"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("set[const(\"a\"), const(\"b\")]"),
            "set aggregate should render as a flat element list: {snapshot}"
        );
        assert!(snapshot.contains("allocator"));
        Ok(())
    }

    #[test]
    fn lowers_a_slice_expression_as_a_slice_projected_place_read() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def middle(s: str) -> str:\n  return s[1:3]\n";
        let module = build(source, &["m", "slice"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("[const(1):const(3)]"),
            "slice projection should render start/end operands: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_tuple_unpack_into_field_projected_reads_off_a_materialized_tuple()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "def sum_pair() -> int:\n  pair = (1, 2)\n  a, b = pair\n  return a + b\n";
        let module = build(source, &["m", "tuple_unpack"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains(".0") && snapshot.contains(".1"),
            "tuple unpack should project each element by index: {snapshot}"
        );
        assert!(
            !snapshot.contains("unsupported("),
            "tuple unpack should not fall back: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_method_call_on_self_with_a_borrowed_receiver_argument() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model Counter:\n  value: int\n\n  def get(self) -> int:\n    return self.value\n\n  def get_twice(self) -> int:\n    return self.get() + self.get()\n";
        let module = build(source, &["m", "method_call"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("body get_twice decl:m::method_call::Counter::get_twice"));
        // Method-call receivers borrow, mirroring how any other method call's receiver already lowers.
        assert!(snapshot.contains("call method:get(borrow(_0))"));
        Ok(())
    }

    #[test]
    fn abstract_trait_method_produces_no_body() -> Result<(), Box<dyn std::error::Error>> {
        let source = "trait Greeter:\n  def greet(self) -> str: ...\n";
        let module = build(source, &["m", "abstract_method"])?;

        assert!(
            module.bodies.is_empty(),
            "an abstract method has no body to lower, and must not produce an Unsupported placeholder body either: {:?}",
            module.bodies
        );

        Ok(())
    }

    #[test]
    fn lowers_tuple_assign_swap_with_correct_evaluation_order() -> Result<(), Box<dyn std::error::Error>> {
        // `arr[i], arr[j] = (arr[j], arr[i])` must read both original values before writing either target, or the
        // swap would clobber `arr[i]` before `arr[j]`'s read observes it. A leading plain-identifier target (`a, b
        // = ...`) always parses as `TupleUnpackStmt` instead (new bindings, possibly shadowing) -- lvalue index/
        // field targets are what actually reaches `TupleAssignStmt`, matching the parser's own routing
        // (`crates/incan_syntax/src/parser/stmts.rs`'s `assignment_or_expr_stmt`).
        let source = "def swap(mut arr: list[int], i: int, j: int) -> int:\n  arr[i], arr[j] = (arr[j], arr[i])\n  return arr[i]\n";
        let module = build(source, &["m", "tuple_assign"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "tuple assign should not fall back: {snapshot}"
        );
        // Both targets should end up written via a plain `Assign` into an `[index]`-projected place, not
        // `Unsupported`.
        assert!(
            snapshot.matches("] = ").count() >= 2,
            "both index-projected targets should be assigned: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_default_trait_method_with_a_self_typed_receiver() -> Result<(), Box<dyn std::error::Error>> {
        let source = "trait Identity:\n  def identity(self) -> Self:\n    return self\n";
        let module = build(source, &["m", "trait_default"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("body identity decl:m::trait_default::Identity::identity"));
        assert!(snapshot.contains("local 0 self : Self [receiver]"));
        assert!(snapshot.contains("return clone(_0)"));

        Ok(())
    }

    #[test]
    fn lowers_chained_assignment_right_to_left() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def chain() -> int:\n  x = y = z = 5\n  return x + y + z\n";
        let module = build(source, &["m", "chained"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "chained assignment should not fall back: {snapshot}"
        );
        assert!(
            snapshot.contains("const(5)"),
            "the rightmost target reads the literal value: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn static_method_lowers_like_a_free_function_with_no_receiver_local() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model Counter:\n  value: int\n\n  def zero() -> Counter:\n    return Counter(value=0)\n";
        let module = build(source, &["m", "static_method"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("body zero decl:m::static_method::Counter::zero"));
        assert!(
            !snapshot.contains("[receiver"),
            "a static/associated method (receiver: None) must not declare a receiver local: {snapshot}"
        );

        Ok(())
    }

    #[test]
    fn method_parameter_type_is_recorded_from_the_checked_callable_signature() -> Result<(), Box<dyn std::error::Error>>
    {
        let source =
            "model Counter:\n  value: int\n\n  def add(self, amount: int) -> int:\n    return self.value + amount\n";
        let module = build(source, &["m", "method_param"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("body add decl:m::method_param::Counter::add"));
        assert!(
            snapshot.contains("local 1 amount : int [param]"),
            "an ordinary method parameter must declare with its checked resolved type, not Unknown: {snapshot}"
        );

        Ok(())
    }

    #[test]
    fn top_level_defaults_lower_to_deferred_source_computations() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def fallback() -> int:\n  return 2\n\ndef choose(limit: u8 = 7, value: int = fallback()) -> int:\n  return value\n";
        let module = build(source, &["m", "top_level_default"])?;
        let choose = module
            .bodies
            .iter()
            .find(|body| body.name == "choose")
            .ok_or("expected the choose Body IR")?;
        let limit = choose.params.first().ok_or("expected choose's limit parameter")?;
        let value = choose.params.get(1).ok_or("expected choose's value parameter")?;

        assert_eq!(limit.local, bir::LocalId(0));
        assert_eq!(limit.name, "limit");
        assert_eq!(
            limit.ty,
            IncanType::Primitive(IncanPrimitiveType::Numeric("u8".to_string()))
        );
        let bir::CallableParamDefault::Source(limit_default) = &limit.default else {
            return Err("a checked literal default must become a deferred Body-IR computation".into());
        };
        let limit_start = source.find("7,").ok_or("missing literal default source spelling")?;
        assert_eq!(limit_default.span, HirSourceSpan::new(limit_start, limit_start + 1));
        assert!(limit_default.stmts.is_empty());
        assert_eq!(limit_default.result, bir::Operand::Constant(bir::Constant::Int(7)));

        assert_eq!(value.local, bir::LocalId(1));
        assert_eq!(value.name, "value");
        let bir::CallableParamDefault::Source(value_default) = &value.default else {
            return Err("a checked function default call must become a deferred Body-IR computation".into());
        };
        let call_start = source.rfind("fallback()").ok_or("missing default source spelling")?;
        assert_eq!(
            value_default.span,
            HirSourceSpan::new(call_start, call_start + "fallback()".len()),
            "the direct consumer must receive the default expression's exact source span"
        );
        let [call] = value_default.stmts.as_slice() else {
            return Err("the deferred function default should contain one call statement".into());
        };
        let bir::StatementKind::Call {
            destination: Some(destination),
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            args,
            may_panic,
        } = &call.kind
        else {
            return Err("the deferred default must retain a direct named call".into());
        };
        assert_eq!(target.name, "fallback");
        assert!(target.type_args.is_empty());
        assert!(args.is_empty());
        assert!(!may_panic);
        let bir::Operand::Place(result) = &value_default.result else {
            return Err("the deferred default call must return its computed temporary".into());
        };
        assert_eq!(&result.place, destination);
        assert!(
            !choose.block.stmts.iter().any(|statement| matches!(
                &statement.kind,
                bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                    ..
                } if target.name == "fallback"
            )),
            "the default call must not be appended to the ordinary function body: {choose:?}"
        );
        assert!(
            !choose
                .locals
                .iter()
                .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
            "a refused source default must not retain an implicit frontend lookup: {choose:?}"
        );

        Ok(())
    }

    #[test]
    fn generic_method_defaults_use_the_shared_parameter_contract_after_self() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = "def fallback() -> str:\n  return \"label\"\n\nmodel Shelf[T]:\n  def label[U](self, owner_items: list[T] = [], method_items: list[U] = [], suffix: str = \"\", fallback_label: str = fallback()) -> str:\n    return suffix\n";
        let module = build(source, &["m", "method_default"])?;
        let label = module
            .bodies
            .iter()
            .find(|body| body.name == "label")
            .ok_or("expected the label method Body IR")?;
        let self_param = label.params.first().ok_or("expected the self parameter")?;
        let owner_items = label.params.get(1).ok_or("expected the owner-generic parameter")?;
        let method_items = label.params.get(2).ok_or("expected the method-generic parameter")?;
        let suffix = label.params.get(3).ok_or("expected the literal default parameter")?;
        let fallback_label = label.params.get(4).ok_or("expected the call default parameter")?;

        assert_eq!(self_param.local, bir::LocalId(0));
        assert!(
            self_param.span.start < self_param.span.end,
            "the synthetic receiver must carry its documented declaration-span fallback"
        );
        assert!(matches!(&self_param.default, bir::CallableParamDefault::Required));
        assert!(matches!(&owner_items.default, bir::CallableParamDefault::Source(_)));
        assert!(matches!(&method_items.default, bir::CallableParamDefault::Source(_)));
        let bir::CallableParamDefault::Source(suffix_default) = &suffix.default else {
            return Err("a checked method literal default must become a deferred computation".into());
        };
        let literal_start = source.find("\"\"").ok_or("missing method literal default spelling")?;
        assert_eq!(
            suffix_default.span,
            HirSourceSpan::new(literal_start, literal_start + "\"\"".len())
        );
        assert!(suffix_default.stmts.is_empty());
        assert_eq!(
            suffix_default.result,
            bir::Operand::Constant(bir::Constant::Str(String::new()))
        );
        let bir::CallableParamDefault::Source(fallback_default) = &fallback_label.default else {
            return Err("a checked method call default must become a deferred computation".into());
        };
        let call_start = source
            .rfind("fallback()")
            .ok_or("missing method call default spelling")?;
        assert_eq!(
            fallback_default.span,
            HirSourceSpan::new(call_start, call_start + "fallback()".len())
        );
        assert_eq!(fallback_default.stmts.len(), 1);
        assert!(
            !label.block.stmts.iter().any(|statement| matches!(
                &statement.kind,
                bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                    ..
                } if target.name == "fallback"
            )),
            "the method default call must not be appended to the ordinary method body: {label:?}"
        );
        assert_eq!(label.param_locals.len(), 5);

        Ok(())
    }

    #[test]
    fn trait_method_default_uses_a_deferred_source_computation() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def fallback() -> str:\n  return \"hello\"\n\ntrait Greeter:\n  def greet(self, greeting: str = fallback()) -> str:\n    return greeting\n";
        let module = build(source, &["m", "trait_method_default"])?;
        let greet = module
            .bodies
            .iter()
            .find(|body| body.name == "greet")
            .ok_or("expected the greet trait-method Body IR")?;
        let self_param = greet.params.first().ok_or("expected the self parameter")?;
        let greeting = greet.params.get(1).ok_or("expected the greeting parameter")?;

        assert!(matches!(&self_param.default, bir::CallableParamDefault::Required));
        let bir::CallableParamDefault::Source(default) = &greeting.default else {
            return Err("a checked trait-method default must become a deferred computation".into());
        };
        let default_start = source
            .rfind("fallback()")
            .ok_or("missing trait-method default source spelling")?;
        assert_eq!(
            default.span,
            HirSourceSpan::new(default_start, default_start + "fallback()".len())
        );
        let [call] = default.stmts.as_slice() else {
            return Err("the deferred trait-method default should contain one call statement".into());
        };
        assert!(matches!(
            &call.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        ));
        assert!(
            !greet.block.stmts.iter().any(|statement| matches!(
                &statement.kind,
                bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                    ..
                } if target.name == "fallback"
            )),
            "the trait-method default call must not be appended to the ordinary method body: {greet:?}"
        );

        Ok(())
    }

    #[test]
    fn unrepresentable_default_is_a_parameter_refusal_at_its_own_span() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def keep(payload: bytes = b\"x\") -> bytes:\n  return payload\n";
        let module = build(source, &["m", "unsupported_default"])?;
        let keep = module.bodies.first().ok_or("expected the keep function Body IR")?;
        let payload = keep.params.first().ok_or("expected the payload parameter")?;

        let bir::CallableParamDefault::Unsupported { span, description } = &payload.default else {
            return Err("bytes defaults must refuse instead of pretending to be executable".into());
        };
        assert_eq!(description, "bytes literal");
        let default_start = source.find("b\"x\"").ok_or("missing bytes default spelling")?;
        assert_eq!(
            *span,
            HirSourceSpan::new(default_start, default_start + "b\"x\"".len()),
            "the refusal must retain the unsupported default expression's exact source span"
        );
        assert_eq!(
            keep.locals.len(),
            1,
            "refused speculative lowering must not leak a default temporary or external local: {keep:?}"
        );
        assert!(
            !keep
                .block
                .stmts
                .iter()
                .any(|statement| matches!(&statement.kind, bir::StatementKind::Unsupported { .. })),
            "the refusal belongs to the parameter contract, not the normal function body: {keep:?}"
        );

        Ok(())
    }

    #[test]
    fn unsupported_race_arm_in_a_default_is_found_at_its_nested_source_span() {
        // A race remains a structured Body-IR node even when one arm has an unsupported construct. Callable
        // defaults have the stricter contract: the whole deferred computation must be executable, so the default
        // boundary must find that nested refusal and retain the nested construct's span for direct consumers.
        let unsupported_span = HirSourceSpan::new(24, 34);
        let statements = vec![bir::Statement {
            kind: bir::StatementKind::Race {
                destination: None,
                arms: vec![bir::RaceArm {
                    awaitable: bir::Operand::Constant(bir::Constant::Int(1)),
                    binding: bir::LocalId(0),
                    body: bir::Block {
                        scope: bir::ScopeId(1),
                        stmts: vec![bir::Statement {
                            kind: bir::StatementKind::Unsupported {
                                description: "power operator".to_string(),
                            },
                            span: unsupported_span,
                        }],
                    },
                    result: bir::Operand::Constant(bir::Constant::Int(0)),
                }],
            },
            span: HirSourceSpan::new(10, 40),
        }];

        assert_eq!(
            first_unsupported_default_statement(&statements),
            Some((unsupported_span, "power operator".to_string())),
            "a direct consumer must refuse the nested construct rather than accept a partially unsupported default"
        );
    }

    #[test]
    fn unsupported_rvalue_bodies_in_a_default_are_found_at_their_nested_source_spans() {
        // A source default can construct a closure or generator, or evaluate a match, whose structured Body IR owns
        // more statements than the outer assignment exposes. Those statement sequences are still part of the direct
        // default contract: a consumer must receive their original refusal span instead of a misleading `Source`.
        let unsupported = |span: HirSourceSpan, description: &str| bir::Statement {
            kind: bir::StatementKind::Unsupported {
                description: description.to_string(),
            },
            span,
        };
        let assignment = |rvalue| bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(bir::LocalId(0)),
                rvalue,
            },
            span: HirSourceSpan::new(0, 80),
        };
        let result = bir::Operand::Constant(bir::Constant::Int(0));
        let closure_span = HirSourceSpan::new(10, 20);
        let generator_span = HirSourceSpan::new(21, 31);
        let guard_span = HirSourceSpan::new(32, 42);
        let body_span = HirSourceSpan::new(43, 53);
        let cases = vec![
            (
                vec![assignment(bir::Rvalue::Closure {
                    params: Vec::new(),
                    captured_operands: Vec::new(),
                    body: Box::new(bir::ClosureBody {
                        capture_locals: Vec::new(),
                        stmts: vec![unsupported(closure_span, "closure body")],
                        result: result.clone(),
                    }),
                })],
                closure_span,
                "closure body",
            ),
            (
                vec![assignment(bir::Rvalue::Generator {
                    source: bir::Operand::Constant(bir::Constant::Int(1)),
                    captured_operands: Vec::new(),
                    body: Box::new(bir::GeneratorBody {
                        source_local: bir::LocalId(1),
                        capture_locals: Vec::new(),
                        stmts: vec![unsupported(generator_span, "generator body")],
                    }),
                })],
                generator_span,
                "generator body",
            ),
            (
                vec![assignment(bir::Rvalue::Match {
                    scrutinee: bir::Operand::Constant(bir::Constant::Int(1)),
                    arms: vec![bir::MatchArm {
                        pattern: bir::Pattern::Wildcard,
                        guard_stmts: vec![unsupported(guard_span, "match guard")],
                        guard: Some(bir::Operand::Constant(bir::Constant::Bool(true))),
                        body_stmts: Vec::new(),
                        result: result.clone(),
                    }],
                })],
                guard_span,
                "match guard",
            ),
            (
                vec![assignment(bir::Rvalue::Match {
                    scrutinee: bir::Operand::Constant(bir::Constant::Int(1)),
                    arms: vec![bir::MatchArm {
                        pattern: bir::Pattern::Wildcard,
                        guard_stmts: Vec::new(),
                        guard: None,
                        body_stmts: vec![unsupported(body_span, "match body")],
                        result,
                    }],
                })],
                body_span,
                "match body",
            ),
        ];

        for (statements, span, description) in cases {
            assert_eq!(
                first_unsupported_default_statement(&statements),
                Some((span, description.to_string())),
                "a nested {description} refusal must prevent an incomplete default computation from becoming Source"
            );
        }
    }

    #[test]
    fn invalid_default_is_rejected_before_body_ir_is_built() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def choose(value: int = \"wrong\") -> int:\n  return value\n";
        let error = build(source, &["m", "invalid_default"])
            .err()
            .ok_or("a mismatched callable default must be rejected before Body IR construction")?;
        assert!(
            error.to_string().contains("Type mismatch: expected 'int', found 'str'"),
            "the source typechecker must reject the mismatched default before a Body-IR consumer sees it: {error}"
        );

        Ok(())
    }

    #[test]
    fn refused_default_restores_ownership_state_before_local_ids_are_reused() -> Result<(), Box<dyn std::error::Error>>
    {
        // Lowering the partial moves one of its synthesized forwarding locals before the bytes literal refuses.
        // The transaction must discard that move before `second` reuses the local id in the normal body, or the
        // required root-scope drop would silently disappear.
        let source = "def route(method: str) -> str:\n  return method\n\ndef choose(value: str = (partial route(method=\"GET\")) + b\"x\") -> str:\n  first = \"first\"\n  second = \"second\"\n  return first\n";
        let (module, _diagnostics) =
            build_after_expected_typecheck_errors(source, &["m", "default_ownership_rollback"])?;
        let choose = module
            .bodies
            .iter()
            .find(|body| body.name == "choose")
            .ok_or("expected the choose Body IR")?;
        let value = choose.params.first().ok_or("expected choose's value parameter")?;
        assert!(matches!(
            &value.default,
            bir::CallableParamDefault::Unsupported { description, .. } if description == "bytes literal"
        ));
        let second = choose
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some("second"))
            .ok_or("expected second binding after refused default")?;
        assert!(
            choose.block.stmts.iter().any(|statement| matches!(
                &statement.kind,
                bir::StatementKind::Drop { local } if *local == second.id
            )),
            "a stale speculative move must not suppress second's required drop: {choose:?}"
        );

        Ok(())
    }

    #[test]
    fn invalid_callable_defaults_remain_body_ir_refusals_without_implicit_captures()
    -> Result<(), Box<dyn std::error::Error>> {
        let cases = [
            (
                "earlier parameter",
                "def choose(first: str, second: str = first) -> str:\n  return second\n",
                "first",
            ),
            (
                "receiver",
                "model Label:\n  text: str\n\n  def choose(self, value: str = self.text) -> str:\n    return value\n",
                "self.text",
            ),
            (
                "bare field",
                "model Label:\n  text: str\n\n  def choose(self, value: str = text) -> str:\n    return value\n",
                "text",
            ),
            (
                "bare property",
                "model Label:\n  text: str\n\n  property display -> str:\n    return self.text\n\n  def choose(self, value: str = display) -> str:\n    return value\n",
                "display",
            ),
        ];

        for (case, source, default_spelling) in cases {
            let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "default_capture"])?;
            let rejected_name = match default_spelling.split('.').next() {
                Some(name) => name,
                None => default_spelling,
            };
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic.contains(rejected_name)),
                "the source checker must reject the {case} default before Body IR: {diagnostics:?}"
            );
            let choose = module
                .bodies
                .iter()
                .find(|body| body.name == "choose")
                .ok_or("expected the choose Body IR")?;
            let parameter = choose.params.last().ok_or("expected the defaulted parameter")?;
            let bir::CallableParamDefault::Unsupported { span, description } = &parameter.default else {
                return Err(format!("a {case} default must not fabricate a callable-frame or instance capture").into());
            };
            let default_start = source
                .rfind(default_spelling)
                .ok_or("missing invalid default source spelling")?;
            assert_eq!(
                *span,
                HirSourceSpan::new(default_start, default_start + default_spelling.len()),
                "the {case} refusal must preserve the whole default expression span"
            );
            assert!(description.contains(rejected_name));
            assert!(
                !choose
                    .locals
                    .iter()
                    .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
                "a direct consumer must not need an implicit lexical lookup for the refused {case} default: {choose:?}"
            );
        }

        Ok(())
    }

    #[test]
    fn validated_newtype_default_remains_a_visible_body_ir_refusal() -> Result<(), Box<dyn std::error::Error>> {
        let source = "type Attempts = newtype int:\n  def from_underlying(n: int) -> Result[Attempts, ValidationError]:\n    return Ok(Attempts(n))\n\ndef choose(value: Attempts = 3) -> Attempts:\n  return value\n";
        let module = build(source, &["m", "newtype_default"])?;
        let choose = module
            .bodies
            .iter()
            .find(|body| body.name == "choose")
            .ok_or("expected the choose Body IR")?;
        let value = choose.params.first().ok_or("expected the newtype default parameter")?;
        let bir::CallableParamDefault::Unsupported { span, description } = &value.default else {
            return Err(
                "a default requiring validated-newtype coercion must not become a raw source computation".into(),
            );
        };
        let default_start = source.rfind("3)").ok_or("missing newtype default spelling")?;
        assert_eq!(*span, HirSourceSpan::new(default_start, default_start + 1));
        assert_eq!(
            description,
            "default requires a validated-newtype coercion Body IR does not yet represent"
        );
        assert_eq!(choose.locals.len(), 1);
        assert!(
            !choose
                .locals
                .iter()
                .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
            "the newtype refusal must not leave a hidden source lookup in the callable body: {choose:?}"
        );

        Ok(())
    }

    #[test]
    fn aliased_method_parameter_type_retains_the_checked_callable_type() -> Result<(), Box<dyn std::error::Error>> {
        // `UserId` is a type alias for `int` (RFC-style `type X = Y`). A naive re-parse of the raw `id: UserId`
        // annotation inside Body IR (with no alias table of its own) could only produce `Named("UserId")`; the
        // checked callable type resolves the alias all the way through, so the local must show `int`.
        let source = "type UserId = int\n\nmodel Account:\n  balance: int\n\n  def credit(self, id: UserId, amount: int) -> int:\n    return self.balance + amount\n";
        let module = build(source, &["m", "aliased_param"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("local 1 id : int [param]"),
            "an aliased parameter type must resolve through the alias like any other checked expression, not stay \
             the raw `UserId` annotation spelling: {snapshot}"
        );

        Ok(())
    }

    #[test]
    fn generic_method_parameter_type_retains_the_owner_type_variable() -> Result<(), Box<dyn std::error::Error>> {
        let source = "class Box[T]:\n  value: T\n\n  def replace(mut self, other: T) -> None:\n    self.value = other\n\n  def wrap(mut self, items: list[T]) -> None:\n    pass\n";
        let module = build(source, &["m", "generic_param"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("local 1 other : T [param]"),
            "a bare owner type-variable parameter must retain the checked type variable: {snapshot}"
        );
        assert!(
            snapshot.contains("local 1 items : List[T] [param]"),
            "a generic collection parameter must retain its checked element type variable: {snapshot}"
        );

        Ok(())
    }

    #[test]
    fn static_method_parameter_types_are_recorded_like_ordinary_methods() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model Counter:\n  value: int\n\n  def from_value(amount: int) -> Counter:\n    return Counter(value=amount)\n";
        let module = build(source, &["m", "static_param"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("body from_value decl:m::static_param::Counter::from_value"));
        assert!(
            !snapshot.contains("[receiver"),
            "a static/associated method (receiver: None) must not declare a receiver local: {snapshot}"
        );
        assert!(
            snapshot.contains("local 0 amount : int [param]"),
            "a static method's ordinary parameters must resolve the same way an instance method's do: {snapshot}"
        );

        Ok(())
    }

    #[test]
    fn overloaded_method_declarations_retain_distinct_parameter_types_by_declaration_span()
    -> Result<(), Box<dyn std::error::Error>> {
        // Two `add` methods on the same owner, distinguished only by adopting two instantiations of the same
        // generic trait (RFC 042 multi-instantiation) -- the language surface's one legitimate way to declare
        // same-name, same-owner method overloads with genuinely different parameter types. If the checked binding
        // table were keyed by `(owner, method_name)` alone (like `decorated_method_bindings`), the second
        // declaration would silently overwrite the first and both bodies would report the same parameter type.
        let source = "trait Adder[T]:\n  def add(self, x: T) -> T: ...\n\nmodel Calc with Adder[int], Adder[str]:\n  count: int\n\n  def add(self, x: int) -> int:\n    return x\n\n  def add(self, x: str) -> str:\n    return x\n";
        let module = build(source, &["m", "overload_param"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("local 1 x : int [param]"),
            "the int-instantiated overload must keep its own checked parameter type: {snapshot}"
        );
        assert!(
            snapshot.contains("local 1 x : str [param]"),
            "the str-instantiated overload must keep its own distinct checked parameter type, not collide with the \
             int overload recorded under the same method name: {snapshot}"
        );

        Ok(())
    }

    #[test]
    fn method_parameter_type_falls_back_to_unknown_only_when_the_typechecker_binding_is_absent()
    -> Result<(), Box<dyn std::error::Error>> {
        // A successful typecheck always populates `method_bindings_by_span` for every method Body IR actually
        // lowers a body for (see `TypeChecker::check_method_with_self_ty`), so the only way to observe the
        // fallback honestly is to simulate the checked fact genuinely being absent -- exercising the same
        // defence-in-depth path `lower_method_body` falls back to, rather than asserting on a state ordinary
        // typechecking can never produce.
        let source =
            "model Counter:\n  value: int\n\n  def add(self, amount: int) -> int:\n    return self.value + amount\n";
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let module_path: Vec<String> = vec!["m".to_string(), "fallback_param".to_string()];
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

        let mut type_info = checker.type_info().clone();
        type_info.declarations.method_bindings_by_span.clear();

        let module = build_body_ir_module_v0(&program, &module_path, &type_info);
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("local 1 amount : ? [param]"),
            "with no recorded checked binding for this declaration, the parameter must fall back to the explicit \
             Unknown type rather than guessing from the raw annotation: {snapshot}"
        );

        Ok(())
    }

    #[test]
    fn lowers_compound_assignment_as_a_read_modify_write() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def accumulate(step: int) -> int:\n  mut total = 0\n  total += step\n  return total\n";
        let module = build(source, &["m", "compound"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "compound assignment should not fall back: {snapshot}"
        );
        assert!(
            snapshot.contains(" + "),
            "compound assignment should desugar through a binary op: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_compound_string_assignment_through_the_string_concat_helper() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def greet(name: str) -> str:\n  mut out = \"hi \"\n  out += name\n  return out\n";
        let module = build(source, &["m", "compound_str"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("call helper:str_concat"),
            "string compound assignment should route through the same helper as `+`: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_field_assignment_on_a_mutable_model_parameter() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model Counter:\n  count: int\n\ndef bump(mut c: Counter) -> int:\n  c.count = c.count + 1\n  return c.count\n";
        let module = build(source, &["m", "field_assign"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "field assignment should not fall back: {snapshot}"
        );
        assert!(
            snapshot.contains(".count = "),
            "should assign into the `.count` projection: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_index_assignment_on_a_mutable_list_parameter() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def set_first(mut items: list[int], value: int) -> None:\n  items[0] = value\n  return\n";
        let module = build(source, &["m", "index_assign"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "index assignment should not fall back: {snapshot}"
        );
        assert!(
            snapshot.contains("[const(0)] = "),
            "should assign into the `[0]` projection: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn index_assignment_evaluates_object_before_index() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def make_items() -> list[int]:\n  return [1]\n\ndef make_index() -> int:\n  return 0\n\ndef assign() -> None:\n  make_items()[make_index()] = 7\n  return\n";
        let module = build(source, &["m", "index_assignment_order"])?;
        let snapshot = module.render_snapshot();
        let object_call = snapshot
            .find("call fn:make_items()")
            .ok_or("missing index-assignment object call")?;
        let index_call = snapshot
            .find("call fn:make_index()")
            .ok_or("missing index-assignment index call")?;

        assert!(
            object_call < index_call,
            "index assignment must evaluate its object before its index: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_expression_position_if_as_unit_typed() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def maybe_print(flag: bool) -> None:\n  if flag:\n    pass\n  else:\n    pass\n  return\n";
        // `if` used purely as a statement already covers the statement-position path; this test instead exercises
        // the expression-position path via a plain expression statement wrapping an `if` expression's value.
        let source_expr = "def maybe(flag: bool) -> None:\n  _ = if flag:\n    pass\n  else:\n    pass\n  return\n";
        let _ = build(source, &["m", "if_stmt"])?; // sanity: statement-position if still works unchanged
        let module = build(source_expr, &["m", "if_expr"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "expression-position if should not fall back: {snapshot}"
        );
        assert!(
            snapshot.contains("const(())"),
            "an if-expression's value should be the Unit constant: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_loop_expression_break_value_into_a_merged_result_place() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def find(flag: bool) -> int:\n  return loop:\n    if flag:\n      break 42\n    break 7\n";
        let module = build(source, &["m", "loop_expr"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "loop-expression should not fall back: {snapshot}"
        );
        // Both `break 42` and `break 7` should have been rewritten into an assignment to the shared result local
        // followed by a plain, valueless `break`, rather than carrying a value on `Break` itself.
        assert!(snapshot.contains("const(42)"));
        assert!(snapshot.contains("const(7)"));
        assert!(
            !snapshot.contains("break const"),
            "break value should be assigned into the result place, not carried on `break`: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn nested_while_break_inside_a_loop_expression_does_not_target_the_outer_loop()
    -> Result<(), Box<dyn std::error::Error>> {
        // A plain `break` inside a nested `while` must exit the `while`, not accidentally get rewritten into an
        // assignment to the outer `loop:` expression's result place.
        let source = "def find(limit: int) -> int:\n  return loop:\n    mut i = 0\n    while i < limit:\n      if i == 5:\n        break\n      i = i + 1\n    break i\n";
        let module = build(source, &["m", "nested_loop"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "nested while/loop should not fall back: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_try_into_an_explicit_try_propagate_statement() -> Result<(), Box<dyn std::error::Error>> {
        let source = "enum E:\n  Bad\n\ndef half(x: int) -> Result[int, E]:\n  if x % 2 != 0:\n    return Err(E.Bad)\n  return Ok(x // 2)\n\ndef quarter(x: int) -> Result[int, E]:\n  h = half(x)?\n  return half(h)\n";
        let module = build(source, &["m", "try_expr"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("= try?("),
            "`?` should lower to an explicit try-propagate statement: {snapshot}"
        );
        assert!(
            snapshot.contains("same_error_type=E")
                && snapshot.contains("result_ok(")
                && snapshot.contains("result_err("),
            "Result constructors and exact error routing must stay explicit in Body IR: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_an_fstring_into_a_format_rvalue_with_literal_and_display_parts() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = "def greet(name: str) -> str:\n  return f\"hello {name}\"\n";
        let module = build(source, &["m", "fstring_display"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("fstring(lit(\"hello \"), move(_0, last_use):display"),
            "f-string should lower to an explicit Format rvalue with literal and display parts: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_an_fstring_debug_interpolation_using_the_debug_style() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def show(n: int) -> str:\n  return f\"n={n:?}\"\n";
        let module = build(source, &["m", "fstring_debug"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains(":debug"),
            "`{{n:?}}` should lower to a Debug-styled format part: {snapshot}"
        );
        assert!(
            !snapshot.contains(":display"),
            "a debug interpolation should not also render as display: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn fstring_records_the_fstring_runtime_helper_and_allocator_requirements() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = "def label(x: int) -> str:\n  return f\"x={x}\"\n";
        let module = build(source, &["m", "fstring_reqs"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("runtime_requirements:"));
        assert!(snapshot.contains("runtime_helper(fstring)"));
        assert!(snapshot.contains("allocator"));
        Ok(())
    }

    #[test]
    fn fstring_embedded_expression_participates_in_last_use_tracking() -> Result<(), Box<dyn std::error::Error>> {
        // `s` is read twice: once as a plain binding RHS and once inside the f-string. The f-string's embedded read
        // must still count toward `s`'s last-use countdown (see `count_reads_in_expr`'s `ast::Expr::FString` arm),
        // so the first (non-last) read clones and only the f-string's read -- the true last use -- moves.
        let source = "def dup(s: str) -> str:\n  first = s\n  return f\"value={s}\"\n";
        let module = build(source, &["m", "fstring_last_use"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("clone(_0)"),
            "the first, non-last read of `s` should clone: {snapshot}"
        );
        assert!(
            snapshot.contains("fstring(lit(\"value=\"), move(_0, last_use):display"),
            "the f-string's embedded read is the true last use and should move: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn comprehension_embedded_expression_participates_in_last_use_tracking() -> Result<(), Box<dyn std::error::Error>> {
        // Mirrors `fstring_embedded_expression_participates_in_last_use_tracking`'s regression shape for the same
        // class of bug: `count_reads_in_expr` must recurse into `ast::Expr::ListComp`'s element expression, or the
        // earlier, non-comprehension read of `s` on the first line would be miscounted as the last use (`Move`)
        // even though the list comprehension on the next line reads `s` again -- an unsound move, not merely an
        // imprecise clone. `s` is read twice: once as a plain binding RHS, once inside the comprehension's element.
        let source = "def dup(s: str, items: list[int]) -> list[str]:\n  first = s\n  return [s for n in items]\n";
        let module = build(source, &["m", "comp_last_use"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("clone(_0)"),
            "the first, non-last read of `s` should clone because the comprehension reads it again: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_closure_capturing_nothing_with_an_empty_capture_list() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def make(step: int) -> int:\n  add: (int) -> int = (x) => x + 1\n  return add(step)\n";
        let module = build(source, &["m", "closure_no_capture"])?;
        let snapshot = module.render_snapshot();
        let make = module.bodies.first().ok_or("expected the make function Body IR")?;
        let closure_params = make
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Assign {
                    rvalue: bir::Rvalue::Closure { params, .. },
                    ..
                } => Some(params),
                _ => None,
            })
            .ok_or("expected the closure literal")?;
        let x = closure_params.first().ok_or("expected the closure parameter")?;

        assert!(
            !snapshot.contains("unsupported("),
            "a closure literal should lower fully, not fall back: {snapshot}"
        );
        assert!(
            snapshot.contains("captures=[]"),
            "a closure that reads no outer variable should capture nothing: {snapshot}"
        );
        assert!(
            snapshot.contains("closure(params=[x: int local=_"),
            "the closure's own parameter should be recorded: {snapshot}"
        );
        assert_eq!(x.name, "x");
        assert_eq!(x.local, bir::LocalId(1));
        assert_eq!(
            x.span,
            HirSourceSpan::new(
                source.find("(x)").ok_or("missing closure parameter spelling")? + 1,
                source.find("(x)").ok_or("missing closure parameter spelling")? + 2,
            )
        );
        assert!(matches!(&x.default, bir::CallableParamDefault::Required));
        Ok(())
    }

    #[test]
    fn source_closure_default_syntax_is_refused_before_body_ir_exists() -> Result<(), Box<dyn std::error::Error>> {
        // Closure parameter parsing deliberately accepts identifiers only. Keeping this source-level failure explicit
        // means #1172 does not invent an executable local-closure default from parser-unrepresentable syntax.
        let source = "def make() -> int:\n  value: (int) -> int = (x = 1) => x\n  return value(2)\n";
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let errors = match parser::parse(&tokens) {
            Ok(_) => return Err("closure-default source syntax must not parse into a Body-IR input".into()),
            Err(errors) => errors,
        };
        let parameter_start = source
            .find("x = 1")
            .ok_or("missing closure default parameter spelling")?;
        let parameter_end = parameter_start + "x = 1".len();

        assert!(
            errors
                .iter()
                .any(|error| { error.span.start >= parameter_start && error.span.end <= parameter_end }),
            "the parser must refuse the closure-default spelling at its own source parameter range: {errors:?}"
        );

        Ok(())
    }

    #[test]
    fn lowers_a_closure_capturing_an_outer_variable_with_a_real_clone_fact() -> Result<(), Box<dyn std::error::Error>> {
        // `name` is read once inside the closure (a capture) and again afterward by `return name`, so the capture
        // is not the last use: it must clone, not move -- a real Duckborrower fact, not a placeholder.
        let source = "def greet(name: str) -> str:\n  make_msg: () -> str = () => name\n  return name\n";
        let module = build(source, &["m", "closure_capture_clone"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("captures=[clone(_0)]"),
            "capturing `name` before its last use should clone: {snapshot}"
        );
        assert!(snapshot.contains("local 1 name : str [captured]"));
        Ok(())
    }

    #[test]
    fn lowers_a_closure_capturing_an_outer_variable_at_its_last_use() -> Result<(), Box<dyn std::error::Error>> {
        // `name` is read once, inside the closure, and never again -- the capture itself is `name`'s last use, so
        // it should move rather than clone.
        let source = "def greet(name: str) -> str:\n  make_msg: () -> str = () => name\n  return make_msg()\n";
        let module = build(source, &["m", "closure_capture_move"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("captures=[move(_0, last_use)]"),
            "capturing `name` at its only/last use should move: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn invokes_a_stored_closure_through_its_local_operand_and_preserves_its_capture_ownership()
    -> Result<(), Box<dyn std::error::Error>> {
        // The local `decorate` is a value with a lexical environment, not a declaration named `decorate`.
        // Its call target must therefore retain the closure-local read (including its ownership fact) rather than
        // being approximated as a direct function call and losing the relationship to the captured `prefix`.
        let source = "def greet(prefix: str) -> str:\n  decorate: (str) -> str = (suffix) => prefix + suffix\n  return decorate(\"!\")\n";
        let module = build(source, &["m", "stored_closure_call"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("captures=[move(_0, last_use)]"),
            "the closure must own its last-use capture explicitly: {snapshot}"
        );
        assert!(
            snapshot.contains("call local:move(_"),
            "the stored closure must be invoked through its local operand: {snapshot}"
        );
        assert!(
            !snapshot.contains("call fn:decorate("),
            "a stored closure must never be misrepresented as a named function: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn closure_body_can_still_read_its_capture_after_lowering_restores_outer_bindings()
    -> Result<(), Box<dyn std::error::Error>> {
        // The closure's own capture-binding local must resolve inside the closure body (via `result:`), and the
        // enclosing function's own read of `step` afterward must resolve back to the *outer* local, not the
        // closure's capture -- i.e. `Self::lower_closure`'s save/restore of `self.bindings` must round-trip.
        let source = "def make(step: int) -> int:\n  add: () -> int = () => step\n  return step\n";
        let module = build(source, &["m", "closure_capture_restore"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("result: copy(_1)"),
            "the closure body should read its own capture-binding local for `step` (an `int`, so `copy`): {snapshot}"
        );
        assert!(
            snapshot.contains("return copy(_0)"),
            "the function's own trailing `return step` must resolve back to the *outer* local `_0`, not the \
             closure's capture-binding local `_1`, proving the save/restore round-trips: {snapshot}"
        );
        assert!(
            !snapshot.contains("unsupported("),
            "nothing here should fall back: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_partial_callable_into_a_forwarding_closure() -> Result<(), Box<dyn std::error::Error>> {
        // A local partial retains every target parameter in its callable surface. The captured `method` is a
        // defaulted, overrideable slot, while `path` remains required and `content_type` keeps the target default.
        // `method` is read again after construction, so the non-Copy preset capture must be a real clone fact.
        let source = "def route(method: str, path: str, content_type: str = \"text\") -> str:\n  return method + path + content_type\n\ndef make(method: str) -> str:\n  get = partial route(method=method)\n  named = get(method=\"POST\", path=\"/named\")\n  return method + get(\"/health\")\n";
        let module = build(source, &["m", "partial_callable"])?;
        let snapshot = module.render_snapshot();
        let make = module
            .bodies
            .iter()
            .find(|body| body.name == "make")
            .ok_or("expected the make function Body IR")?;
        let (partial_params, captured_operands, closure_body) = make
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Assign {
                    rvalue:
                        bir::Rvalue::Closure {
                            params,
                            captured_operands,
                            body,
                        },
                    ..
                } => Some((params, captured_operands, body)),
                _ => None,
            })
            .ok_or("expected the synthesized partial closure")?;
        let method = partial_params
            .iter()
            .find(|param| param.name == "method")
            .ok_or("expected the captured method parameter")?;
        let content_type = partial_params
            .iter()
            .find(|param| param.name == "content_type")
            .ok_or("expected the target default parameter")?;

        assert!(matches!(
            &method.default,
            bir::CallableParamDefault::PartialPreset { .. }
        ));
        let bir::CallableParamDefault::PartialPreset { capture } = &method.default else {
            return Err("the partial preset must retain its capture local".into());
        };
        assert_eq!(closure_body.capture_locals, vec![*capture]);
        assert!(matches!(
            captured_operands.first(),
            Some(bir::Operand::Place(bir::PlaceOperand {
                fact: bir::OwnershipFact::Clone,
                ..
            }))
        ));
        let bir::CallableParamDefault::Source(content_type_default) = &content_type.default else {
            return Err("the unpresetted checked default must remain a deferred source computation".into());
        };
        let content_type_start = source.find("\"text\"").ok_or("missing content_type default spelling")?;
        assert_eq!(
            content_type_default.span,
            HirSourceSpan::new(content_type_start, content_type_start + "\"text\"".len())
        );
        assert!(content_type_default.stmts.is_empty());
        assert_eq!(
            content_type_default.result,
            bir::Operand::Constant(bir::Constant::Str("text".to_string()))
        );
        let supplied_slots: Vec<Vec<usize>> = make
            .block
            .stmts
            .iter()
            .filter_map(|statement| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Local(target)),
                    ..
                } => match &target.binding {
                    bir::ArgumentBinding::Resolved { arguments, .. } => {
                        Some(arguments.iter().map(|argument| argument.slot).collect())
                    }
                    bir::ArgumentBinding::UnresolvedPositional => None,
                },
                _ => None,
            })
            .collect();
        assert!(
            supplied_slots.iter().any(|slots| slots.as_slice() == [0, 1]),
            "a named argument must override the captured preset in its declaration slot: {make:?}"
        );
        assert!(
            supplied_slots.iter().any(|slots| slots.as_slice() == [1]),
            "a positional residual argument must omit the preset and trailing source default by declaration slot: {make:?}"
        );
        assert!(
            snapshot.contains("call local:move(_"),
            "a stored partial must be invoked through its local operand: {snapshot}"
        );
        assert!(
            !snapshot.contains("call fn:get("),
            "a stored partial must never be misrepresented as a named function: {snapshot}"
        );
        assert!(
            snapshot.contains("call fn:route("),
            "the synthesized closure body should forward into a call to the target function: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn stored_partial_refuses_too_few_or_too_many_residual_arguments() -> Result<(), Box<dyn std::error::Error>> {
        let too_few = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(9)\n";
        let (too_few_module, diagnostics) = build_after_expected_typecheck_errors(too_few, &["m", "partial_too_few"])?;
        let too_few_snapshot = too_few_module.render_snapshot();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("Missing required argument 'c'")),
            "the source checker must diagnose the missing residual parameter: {diagnostics:?}"
        );
        assert!(
            too_few_snapshot
            .contains("unsupported(local callable `add_with_one` expects at least 2 required arguments, got 1; missing required parameter `c`)"),
            "a partial invocation may not omit a required residual argument: {too_few_snapshot}"
        );

        let too_many = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(9, 2, 3)\n";
        let (too_many_module, diagnostics) =
            build_after_expected_typecheck_errors(too_many, &["m", "partial_too_many"])?;
        let too_many_snapshot = too_many_module.render_snapshot();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.contains("expects 2 argument(s), got 3")),
            "the source checker must use the residual arity: {diagnostics:?}"
        );
        assert!(
            too_many_snapshot
                .contains("unsupported(local callable `add_with_one` expects at most 2 positional arguments, got 3)"),
            "a partial invocation may not provide more residual positional arguments than its target accepts: {too_many_snapshot}"
        );
        assert!(
            !too_many_snapshot.contains("call fn:add_with_one("),
            "invalid residual arity must not be approximated as a named-function call: {too_many_snapshot}"
        );
        Ok(())
    }

    #[test]
    fn stored_partial_passes_positional_residual_arguments_in_target_declaration_order()
    -> Result<(), Box<dyn std::error::Error>> {
        // Positional calls skip the defaulted preset `a`, while Body IR records their target slots explicitly.
        let source = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(9, 2)\n";
        let module = build(source, &["m", "partial_order"])?;
        let snapshot = module.render_snapshot();
        let local_call = snapshot
            .lines()
            .find(|line| line.contains("call local:"))
            .ok_or("stored partial call missing from Body IR snapshot")?;
        assert!(
            local_call.contains("const(9), const(2)"),
            "residual positional arguments must remain b/c ordered while the preset default stays captured: {local_call}"
        );
        assert!(
            local_call.contains("slots=[1, 2]"),
            "positional residual arguments must map to their target declaration slots: {local_call}"
        );
        assert!(
            local_call.contains("defaults=[0]"),
            "the skipped preset slot must be recorded as a defaulted slot rather than left implicit: {local_call}"
        );
        assert!(
            !snapshot.contains("unsupported("),
            "the residual Body IR call itself should be executable once admitted by the typechecker: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn stored_partial_allows_a_named_preset_override() -> Result<(), Box<dyn std::error::Error>> {
        // The construction-time capture remains the default, but a named argument replaces it for this invocation.
        let source = "def add3(a: int, b: int, c: int) -> int:\n  return a + b + c\n\ndef make() -> int:\n  add_with_one = partial add3(a=1)\n  return add_with_one(a=7, b=9, c=2)\n";
        let module = build(source, &["m", "partial_named_override"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "a named preset override must lower as an ordinary local callable invocation: {snapshot}"
        );
        assert!(
            snapshot.contains("const(7), const(9), const(2)"),
            "the local invocation must retain the explicit target slots for the override and residual values: {snapshot}"
        );
        // An absent slot/defaults suffix is the identity binding: every declared slot filled, in declaration order.
        // That is precisely what distinguishes a named override from the positional call above, which skips the
        // preset and therefore renders `slots=[1, 2] defaults=[0]`.
        let local_call = snapshot
            .lines()
            .find(|line| line.contains("call local:"))
            .ok_or("stored partial call missing from Body IR snapshot")?;
        assert!(
            !local_call.contains("slots=") && !local_call.contains("defaults="),
            "a named override must occupy the captured preset's declaration slot rather than skipping it: {local_call}"
        );
        Ok(())
    }

    #[test]
    fn partial_callable_restores_enclosing_bindings_after_lowering() -> Result<(), Box<dyn std::error::Error>> {
        // `partial join(prefix="hi ")` synthesizes a residual closure parameter called `suffix`, but that internal
        // binding must not replace the enclosing function parameter of the same name. The trailing return must read
        // the original function parameter (`_0`), not the closure-only parameter allocated while lowering the
        // partial expression.
        let source = "def join(prefix: str, suffix: str) -> str:\n  return prefix + suffix\n\ndef keep_outer(suffix: str) -> str:\n  formatter = partial join(prefix=\"hi \")\n  return suffix\n";
        let module = build(source, &["m", "partial_binding_restore"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("return move(_0, last_use)"),
            "the trailing return must resolve the enclosing `suffix` parameter, not a synthesized partial local: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_single_yield_and_marks_the_body_a_generator() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def numbers() -> Generator[int]:\n  yield 1\n";
        let module = build(source, &["m", "single_yield"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("yield const(1)"),
            "yield should lower to an explicit Yield statement: {snapshot}"
        );
        assert!(
            !snapshot.contains("unsupported("),
            "statement-position yield with a value must not fall back to Unsupported: {snapshot}"
        );
        let body = module
            .bodies
            .iter()
            .find(|b| b.name == "numbers")
            .ok_or("numbers body missing from module")?;
        assert!(
            body.is_generator(),
            "a body containing a yield must report is_generator()"
        );
        Ok(())
    }

    #[test]
    fn lowers_multiple_yields_across_control_flow_inside_a_loop() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def counter(n: int) -> Generator[int]:\n  mut i = 0\n  while i < n:\n    yield i\n    i = i + 1\n  yield -1\n";
        let module = build(source, &["m", "loop_yield"])?;
        let snapshot = module.render_snapshot();

        // Two yields: one nested inside the normalized `loop:` the `while` desugars into, one at the top level
        // after the loop.
        assert_eq!(
            snapshot.matches("yield ").count(),
            2,
            "expected exactly two yield statements: {snapshot}"
        );
        assert!(
            snapshot.contains("loop:"),
            "while should still desugar to a normalized loop: {snapshot}"
        );
        let body = module
            .bodies
            .iter()
            .find(|b| b.name == "counter")
            .ok_or("counter body missing from module")?;
        assert!(
            body.is_generator(),
            "a yield nested inside a loop must still be found by is_generator()"
        );
        Ok(())
    }

    #[test]
    fn a_non_generator_function_is_not_reported_as_a_generator() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def add(x: int, y: int) -> int:\n  return x + y\n";
        let module = build(source, &["m", "not_a_generator"])?;
        let body = module
            .bodies
            .iter()
            .find(|b| b.name == "add")
            .ok_or("add body missing from module")?;
        assert!(
            !body.is_generator(),
            "an ordinary function body must not be reported as a generator"
        );
        Ok(())
    }

    #[test]
    fn yield_records_the_generator_runtime_requirements() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def numbers() -> Generator[int]:\n  yield 1\n";
        let module = build(source, &["m", "yield_requirements"])?;
        let snapshot = module.render_snapshot();

        assert!(snapshot.contains("runtime_requirements:"));
        assert!(snapshot.contains("runtime_helper(generator)"));
        assert!(snapshot.contains("hosted_std"));
        assert!(snapshot.contains("allocator"));
        Ok(())
    }

    #[test]
    fn yielded_expression_participates_in_last_use_tracking() -> Result<(), Box<dyn std::error::Error>> {
        // `s` is read once, inside the yielded value, and never again afterward -- it should read as a last-use
        // `move`, not fall back to an undercounted `clone`/`borrow` the way #1101's f-string bucket found and fixed
        // for embedded f-string reads (`count_reads_in_expr`'s `FString` arm); `Yield` needed the same fix.
        let source = "def one(s: str) -> Generator[str]:\n  yield s\n";
        let module = build(source, &["m", "yield_last_use"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("yield move(_0, last_use)"),
            "the yielded value should be a last-use move: {snapshot}"
        );
        Ok(())
    }

    // ---- #1101 B6: match ----

    #[test]
    fn lowers_a_literal_and_wildcard_match_as_a_single_structured_rvalue() -> Result<(), Box<dyn std::error::Error>> {
        let source = concat!(
            "def classify(x: int) -> str:\n",
            "  match x:\n",
            "    case 0:\n",
            "      return \"zero\"\n",
            "    case _:\n",
            "      return \"other\"\n",
            "  return \"unreachable\"\n",
        );
        let module = build(source, &["m", "match_literal"])?;
        let snapshot_first = module.render_snapshot();
        let snapshot_second = build(source, &["m", "match_literal"])?.render_snapshot();
        assert_eq!(snapshot_first, snapshot_second, "lowering must be deterministic");

        assert!(
            snapshot_first.contains("match borrow(_0)"),
            "the scrutinee should be a single explicit read, not decomposed into ifs: {snapshot_first}"
        );
        assert!(
            snapshot_first.contains("const(0)"),
            "the literal pattern should render: {snapshot_first}"
        );
        assert!(
            snapshot_first.contains(" _ =>"),
            "the wildcard pattern should render: {snapshot_first}"
        );
        Ok(())
    }

    #[test]
    fn lowers_an_enum_variant_pattern_that_binds_a_field() -> Result<(), Box<dyn std::error::Error>> {
        let source = concat!(
            "def unwrap_or_zero(x: Option[int]) -> int:\n",
            "  match x:\n",
            "    case Some(value):\n",
            "      return value\n",
            "    case None:\n",
            "      return 0\n",
        );
        let module = build(source, &["m", "match_enum"])?;
        let snapshot = module.render_snapshot();

        // `Some`'s field type is not resolved (v0 does not mirror the existing backend's constructor field-type
        // projection -- see `Pattern`'s own docs), so the binding reads through the conservative
        // non-Copy/projected-read fallback (`borrow`, never `move`) even though `value`'s actual type is `int`.
        assert!(
            snapshot.contains("Some(bind(_1, borrow))"),
            "a positional constructor pattern should bind its field: {snapshot}"
        );
        assert!(
            snapshot.contains("const(none)"),
            "a bare `None` pattern is a literal, not a zero-field constructor: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_guarded_arm_with_the_guard_seeing_the_pattern_binding() -> Result<(), Box<dyn std::error::Error>> {
        let source = concat!(
            "def sign(x: int) -> str:\n",
            "  match x:\n",
            "    case n if n > 0:\n",
            "      return \"positive\"\n",
            "    case n if n < 0:\n",
            "      return \"negative\"\n",
            "    case _:\n",
            "      return \"zero\"\n",
        );
        let module = build(source, &["m", "match_guard"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains(" if "),
            "a guarded arm should render its guard: {snapshot}"
        );
        // `n` binds `_1`/`_3` in the two arms; the guard should read that same pattern-bound local, not the
        // scrutinee's own `_0` -- confirming the guard sees the pattern binding, not a re-read of the scrutinee.
        assert!(
            snapshot.contains("bind(_1, copy) if { _2 = copy(_1) > const(0);"),
            "the first arm's guard should read the pattern-bound `n` (`_1`): {snapshot}"
        );
        assert!(
            snapshot.contains("bind(_3, copy) if { _4 = copy(_3) < const(0);"),
            "the second arm's guard should read its own pattern-bound `n` (`_3`): {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_nested_tuple_pattern_with_field_projected_bindings() -> Result<(), Box<dyn std::error::Error>> {
        let source = concat!(
            "def sum_pair(pair: (int, int)) -> int:\n",
            "  match pair:\n",
            "    case (a, b):\n",
            "      return a + b\n",
        );
        let module = build(source, &["m", "match_tuple"])?;
        let snapshot = module.render_snapshot();

        // Unlike a `Struct`/`Enum` constructor pattern's fields (`Unknown`-typed, see the enum test above), a
        // `Tuple` pattern's element types are resolved precisely via the already-established `tuple_element_types`
        // helper (`BodyBuilder::lower_tuple_unpack`'s own precedent), so both bindings declare as real `int`s...
        assert!(snapshot.contains("local 1 a : int [binding]"));
        assert!(snapshot.contains("local 2 b : int [binding]"));
        // ...and, being Copy `int`s read through a non-empty (tuple-element) projection, read as `copy`, never
        // `move` -- a projected read never moves (see `ownership_fact_for_place`'s own docs).
        assert!(
            snapshot.contains("(bind(_1, copy), bind(_2, copy))"),
            "a tuple pattern should recursively bind each element as a copy: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn byte_string_literal_pattern_lowers_to_an_explicit_placeholder() -> Result<(), Box<dyn std::error::Error>> {
        // `bir::Constant` has no byte-string variant (mirrors `lower_literal`'s own gap for a plain literal
        // *expression*), so a match with an unrepresentable arm bails the whole expression to `Unsupported` before
        // lowering the scrutinee, rather than silently mis-rendering the pattern as a catch-all wildcard the way
        // the existing Rust-emission backend's own `lower_pattern` does.
        let source = concat!(
            "def check(data: bytes) -> str:\n",
            "  match data:\n",
            "    case b\"\\x00\":\n",
            "      return \"null\"\n",
            "    case _:\n",
            "      return \"other\"\n",
        );
        let module = build(source, &["m", "match_bytes"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("unsupported(match arm with a byte-string literal pattern)"),
            "should record an explicit placeholder rather than mis-rendering the pattern: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn or_pattern_alternatives_share_one_local_for_a_bound_name() -> Result<(), Box<dyn std::error::Error>> {
        // RFC 071 requires every `A(x) | B(x)` alternative to bind an identical name/type set, so Rust's own
        // compiled target has exactly one shared binding slot for `x`, not one per alternative -- `seen` in
        // `BodyBuilder::lower_match_pattern` reuses the same local for the second occurrence rather than declaring
        // a second one.
        let source = concat!(
            "enum Shape:\n",
            "  Circle(int)\n",
            "  Square(int)\n",
            "\n",
            "def get_size(s: Shape) -> int:\n",
            "  match s:\n",
            "    case Circle(x) | Square(x):\n",
            "      return x\n",
        );
        let module = build(source, &["m", "match_or_binding"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("Circle(bind(_1, borrow)) | Square(bind(_1, borrow))"),
            "both alternatives should bind the same shared local `_1`: {snapshot}"
        );
        Ok(())
    }

    /// Extract the `_N` place a loop's `IterNext` writes each produced item into, so a destructuring test can assert
    /// on projections off that exact local without hard-coding a local number unrelated lowering changes would churn.
    fn iter_next_destination(snapshot: &str) -> Option<String> {
        snapshot.lines().find_map(|line| {
            let (destination, _) = line.trim().split_once(" = iter_next(")?;
            Some(destination.to_string())
        })
    }

    /// Find the `_N` spelling of the local declared for source binding `name`, so a test can assert on reads of that
    /// binding without pinning a local number.
    fn local_for_binding(snapshot: &str, name: &str) -> Option<String> {
        snapshot.lines().find_map(|line| {
            let (id, tail) = line.trim().strip_prefix("local ")?.split_once(' ')?;
            tail.starts_with(&format!("{name} : ")).then(|| format!("_{id}"))
        })
    }

    #[test]
    fn lowers_a_wildcard_for_pattern_without_declaring_a_binding() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def count(items: list[int]) -> int:\n  mut n = 0\n  for _ in items:\n    n = n + 1\n  return n\n";
        let module = build(source, &["m", "wildcard_for"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "a wildcard loop pattern must lower, not fall back to a placeholder: {snapshot}"
        );
        assert!(
            snapshot.contains(", builtin)"),
            "wildcard iteration still polls the builtin protocol: {snapshot}"
        );
        assert!(
            !snapshot.contains(" _ : "),
            "`_` binds nothing, so it must not become a named local: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_wildcard_for_pattern_over_a_range() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "def count(n: int) -> int:\n  mut total = 0\n  for _ in 0..n:\n    total = total + 1\n  return total\n";
        let module = build(source, &["m", "wildcard_range_for"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "a wildcard range loop must keep the normalized counting-loop shape: {snapshot}"
        );
        assert!(
            snapshot.contains("loop:") && snapshot.contains("break"),
            "the range path still desugars to a normalized loop: {snapshot}"
        );
        assert!(
            !snapshot.contains(" _ : "),
            "`_` binds nothing over a range either: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_tuple_for_pattern_into_one_binding_per_element() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def total(pairs: list[tuple[int, int]]) -> int:\n  mut acc = 0\n  for a, b in pairs:\n    acc = acc + a + b\n  return acc\n";
        let module = build(source, &["m", "tuple_for"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "a tuple loop pattern must lower to real bindings: {snapshot}"
        );
        assert!(
            snapshot.contains(" a : int [binding]"),
            "`a` must be a real source binding carrying its resolved element type: {snapshot}"
        );
        assert!(
            snapshot.contains(" b : int [binding]"),
            "`b` must be a real source binding carrying its resolved element type: {snapshot}"
        );

        let destination = iter_next_destination(&snapshot).ok_or("expected an IterNext statement")?;
        assert!(
            snapshot.contains(&format!("copy({destination}.0)")),
            "`a` must bind the produced item's first tuple field: {snapshot}"
        );
        assert!(
            snapshot.contains(&format!("copy({destination}.1)")),
            "`b` must bind the produced item's second tuple field: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn tuple_for_pattern_bindings_are_readable_inside_the_loop_body() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def total(pairs: list[tuple[int, int]]) -> int:\n  mut acc = 0\n  for a, b in pairs:\n    acc = acc + a + b\n  return acc\n";
        let module = build(source, &["m", "tuple_for_reads"])?;
        let snapshot = module.render_snapshot();

        for name in ["a", "b"] {
            let local = local_for_binding(&snapshot, name)
                .ok_or_else(|| format!("expected a local for `{name}`: {snapshot}"))?;
            assert!(
                snapshot.contains(&format!("copy({local})")),
                "the loop body must read `{name}` through its own binding {local}: {snapshot}"
            );
        }
        Ok(())
    }

    #[test]
    fn lowers_a_tuple_for_pattern_over_a_user_defined_iteration_protocol() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model PairIter:\n  value: int\n\n  def __next__(self) -> Option[tuple[int, int]]:\n    return Some((self.value, self.value))\n\nmodel Pairs:\n  def __iter__(self) -> PairIter:\n    return PairIter(value=0)\n\ndef total() -> int:\n  mut acc = 0\n  for a, b in Pairs():\n    acc = acc + a + b\n  return acc\n";
        let module = build(source, &["m", "protocol_tuple_for"])?;
        let snapshot = module.render_snapshot();

        // Scoped to the loop-pattern refusal specifically: this source's `PairIter(value=0)` constructor also
        // trips Body IR's separate, pre-existing "call with named or unpack arguments" gap, which #1125 does not own.
        assert!(
            !snapshot.contains("unsupported(for-loop pattern"),
            "protocol-driven tuple iteration must lower to real bindings: {snapshot}"
        );
        assert!(
            snapshot.contains("user_defined(__next__)"),
            "the resolved protocol must still drive the poll: {snapshot}"
        );
        assert!(
            snapshot.contains(" a : int [binding]") && snapshot.contains(" b : int [binding]"),
            "both tuple elements must bind with their resolved types: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_nested_tuple_for_pattern_through_projected_subfields() -> Result<(), Box<dyn std::error::Error>> {
        // `for_binding_pattern_item` (`crates/incan_syntax/src/parser/stmts.rs`) admits only `_` or a bare
        // identifier, so a nested loop pattern has no source spelling yet -- see
        // `nested_tuple_for_patterns_have_no_source_spelling_yet`. The typechecker's own
        // `define_for_pattern_bindings` already recurses through nested `Pattern::Tuple` specifically so a
        // hand-built AST cannot reach lowering with a shape lowering does not understand, so this test builds that
        // AST directly and drives the real typecheck-then-lower pipeline over it.
        let source = "def total(pairs: list[tuple[int, tuple[int, int]]]) -> int:\n  mut acc = 0\n  for a, b in pairs:\n    acc = acc + a + b + c\n  return acc\n";
        let module = build_with_nested_for_pattern(source, &["m", "nested_tuple_for"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "a nested tuple loop pattern must lower to real bindings: {snapshot}"
        );
        for name in ["a", "b", "c"] {
            assert!(
                snapshot.contains(&format!(" {name} : int [binding]")),
                "`{name}` must be a real source binding carrying its resolved element type: {snapshot}"
            );
        }

        let destination = iter_next_destination(&snapshot).ok_or("expected an IterNext statement")?;
        assert!(
            snapshot.contains(&format!("copy({destination}.0)")),
            "`a` must bind the outer tuple's first field: {snapshot}"
        );
        assert!(
            snapshot.contains(&format!("copy({destination}.1.0)")),
            "`b` must bind through the nested tuple's first field: {snapshot}"
        );
        assert!(
            snapshot.contains(&format!("copy({destination}.1.1)")),
            "`c` must bind through the nested tuple's second field: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn nested_tuple_for_patterns_have_no_source_spelling_yet() -> Result<(), Box<dyn std::error::Error>> {
        // Pins the boundary `lowers_a_nested_tuple_for_pattern_through_projected_subfields` works around: Body IR
        // lowers nested loop patterns structurally, but no source syntax produces one today, in a `for` statement or
        // in a comprehension `for` clause (both parse their header through `for_binding_pattern`). #1125 explicitly
        // does not add new source syntax, so this stays a parser-surface gap rather than a lowering gap. When the
        // parser does learn this spelling, this test fails and the nested case can move onto the ordinary `build`
        // path.
        let source = "def total(pairs: list[tuple[int, tuple[int, int]]]) -> int:\n  for a, (b, c) in pairs:\n    pass\n  return 0\n";
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        assert!(
            parser::parse(&tokens).is_err(),
            "a parenthesized nested loop pattern is not part of the source surface yet"
        );
        Ok(())
    }

    #[test]
    fn destructured_for_pattern_bindings_do_not_escape_the_loop_scope() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def keep_outer(a: int, pairs: list[tuple[int, int]]) -> int:\n  for a, b in pairs:\n    pass\n  return a\n";
        let module = build(source, &["m", "tuple_for_scope"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("return copy(_0)"),
            "the trailing read must resolve the enclosing parameter, not the destructured loop local: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn destructured_for_pattern_bindings_carry_ownership_and_drop_facts() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def widths(pairs: list[tuple[str, str]]) -> int:\n  mut n = 0\n  for head, tail in pairs:\n    n = n + len(head)\n  return n\n";
        let module = build(source, &["m", "tuple_for_drops"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains(" head : str [binding]") && snapshot.contains(" tail : str [binding]"),
            "non-Copy tuple elements must still bind carrying their resolved element type: {snapshot}"
        );

        let destination = iter_next_destination(&snapshot).ok_or("expected an IterNext statement")?;
        assert!(
            snapshot.contains(&format!("borrow({destination}.0)")),
            "a non-Copy element read through a projection borrows rather than moving: {snapshot}"
        );

        // `head` is its call argument's recorded last use and therefore moves; unread `tail` remains live and owes
        // one loop-scope drop. Count exact ids so the enclosing parameter's root-scope drop is not conflated with
        // either binding.
        let body = module.bodies.first().ok_or("expected the widths Body IR")?;
        let head = body
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some("head"))
            .ok_or("missing loop binding `head`")?;
        let tail = body
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some("tail"))
            .ok_or("missing loop binding `tail`")?;
        assert!(snapshot.contains(&format!("move(_{}", head.id.0)));
        assert_eq!(snapshot.matches(&format!("drop _{}", head.id.0)).count(), 0);
        assert_eq!(snapshot.matches(&format!("drop _{}", tail.id.0)).count(), 1);
        Ok(())
    }

    #[test]
    fn a_closure_does_not_capture_names_a_nested_destructuring_pattern_binds() -> Result<(), Box<dyn std::error::Error>>
    {
        // `a` and `b` are bound by the comprehension's own `for` clause, so they are *not* free variables of the
        // enclosing closure and must never be captured from the enclosing scope -- where they do not exist at all.
        // Before #1125 the free-variable walk only treated a plain `Pattern::Binding` as binding a name, so a
        // destructuring clause pattern left both names looking free.
        let source = "def outer(pairs: list[tuple[int, int]]) -> int:\n  sums: () -> list[int] = () => [a + b for a, b in pairs]\n  return 0\n";
        let module = build(source, &["m", "closure_pattern_capture"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains(" a : ") && !snapshot.contains(" b : "),
            "clause-bound names must not become captured locals of the enclosing closure: {snapshot}"
        );
        assert!(
            snapshot.contains("[captured]"),
            "the closure should still capture the one name it really reads from the enclosing scope: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn a_tuple_for_pattern_over_a_non_tuple_item_type_is_a_type_error() -> Result<(), Box<dyn std::error::Error>> {
        // Regression for the P1 on #1125: this used to typecheck silently, binding both names as `Unknown`, and
        // Body IR then projected `.0`/`.1` out of an `int`.
        let source = "def total(items: list[int]) -> int:\n  for left, right in items:\n    pass\n  return 0\n";
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["m".to_string(), "non_tuple_for".to_string()]));

        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("destructuring a non-tuple iteration item must be rejected, not silently bound as Unknown")?;
        let rendered = format!("{errors:?}");
        assert!(
            rendered.contains("Cannot destructure 2 values from iteration item of type 'int'"),
            "the diagnostic should name the offending item type: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn a_tuple_for_pattern_over_a_mismatched_arity_item_type_is_a_type_error() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = "def total(pairs: list[tuple[int, int]]) -> int:\n  for a, b, c in pairs:\n    pass\n  return 0\n";
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["m".to_string(), "arity_for".to_string()]));

        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("a wrong-arity tuple loop pattern must be rejected")?;
        let rendered = format!("{errors:?}");
        assert!(
            rendered.contains("Cannot unpack 3 values from tuple with 2 elements"),
            "the arity mismatch should be reported: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn lowering_fails_closed_on_a_tuple_pattern_whose_item_type_is_not_a_tuple()
    -> Result<(), Box<dyn std::error::Error>> {
        // Defence in depth for the same P1: the typechecker rejects this program, so lowering should only ever see
        // it from a hand-built AST -- and must refuse rather than project `.0`/`.1` out of an `int`.
        let source = "def total(items: list[int]) -> int:\n  for value in items:\n    pass\n  return 0\n";
        let module = build_with_for_pattern_widened_after_typecheck(source, &["m", "fail_closed_for"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("unsupported(for-loop tuple pattern over non-tuple item type `int`)"),
            "lowering must refuse, naming the item type it cannot destructure: {snapshot}"
        );
        assert!(
            !snapshot.contains(".0)") && !snapshot.contains(".1)"),
            "lowering must not emit tuple-field projections into a non-tuple value: {snapshot}"
        );
        assert!(
            !snapshot.contains(" second : "),
            "no binding may be declared for a refused pattern: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn a_tuple_for_pattern_over_an_unconstrained_type_variable_is_a_type_error()
    -> Result<(), Box<dyn std::error::Error>> {
        // An unconstrained `T` can be instantiated as `int`, and Incan has no tuple-shaped bound that could
        // promise otherwise, so this can never be proven safe.
        let source = "def total[T](items: list[T]) -> int:\n  for left, right in items:\n    pass\n  return 0\n";
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["m".to_string(), "typevar_for".to_string()]));

        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("destructuring an unconstrained type variable must be rejected")?;
        let rendered = format!("{errors:?}");
        assert!(
            rendered.contains("Cannot destructure 2 values from iteration item of type"),
            "the diagnostic should name the underdetermined item type: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn a_tuple_for_pattern_over_type_variable_elements_still_binds() -> Result<(), Box<dyn std::error::Error>> {
        // The shape `crates/incan_stdlib/stdlib/collections.incn` actually uses: the *item* is a tuple, and only
        // its elements are type variables. Rejecting bare type variables must not catch this too.
        let source = "def keys[K, V](items: list[Tuple[K, V]]) -> int:\n  mut n = 0\n  for key, value in items:\n    n = n + 1\n  return n\n";
        let module = build(source, &["m", "typevar_elements_for"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported(for-loop"),
            "a tuple item whose elements are type variables must still bind: {snapshot}"
        );
        assert!(
            snapshot.contains(" key : ") && snapshot.contains(" value : "),
            "both names must bind as real locals: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn lowering_fails_closed_on_a_tuple_pattern_over_an_unconstrained_type_variable()
    -> Result<(), Box<dyn std::error::Error>> {
        // Lowering must apply the same rule the typechecker does, so the two stages cannot disagree about which
        // programs are bindable.
        let source = "def total[T](items: list[T]) -> int:\n  for value in items:\n    pass\n  return 0\n";
        let module = build_with_for_pattern_widened_after_typecheck(source, &["m", "fail_closed_typevar"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("unsupported(for-loop tuple pattern over non-tuple item type"),
            "lowering must refuse an unconstrained type variable, matching the typechecker: {snapshot}"
        );
        assert!(
            !snapshot.contains(".0)") && !snapshot.contains(".1)"),
            "lowering must not emit tuple-field projections into a type variable: {snapshot}"
        );
        Ok(())
    }
    // ========================================================================
    // #1158 -- named, defaulted, and explicitly generic call arguments
    // ========================================================================

    /// Return the single `Call` statement in `body`, failing when there is not exactly one.
    ///
    /// The #1158 tests assert on one call's resolved binding, so a body that lowered to several calls would make a
    /// positional "first call" assertion silently test the wrong statement.
    fn single_call(body: &bir::Body) -> Result<&bir::StatementKind, Box<dyn std::error::Error>> {
        let calls: Vec<&bir::StatementKind> = body
            .block
            .stmts
            .iter()
            .map(|stmt| &stmt.kind)
            .filter(|kind| matches!(kind, bir::StatementKind::Call { .. }))
            .collect();
        match calls.as_slice() {
            [only] => Ok(only),
            other => Err(format!("expected exactly one call statement, found {}", other.len()).into()),
        }
    }

    /// Return the resolved argument binding carried by a call statement's callee.
    fn call_binding(kind: &bir::StatementKind) -> Result<&bir::ArgumentBinding, Box<dyn std::error::Error>> {
        let bir::StatementKind::Call { callee, .. } = kind else {
            return Err("not a call statement".into());
        };
        match callee {
            bir::Callee::Function(bir::CallableTarget::Named(target)) => Ok(&target.binding),
            bir::Callee::Function(bir::CallableTarget::Local(target)) => Ok(&target.binding),
            bir::Callee::Method(target) => Ok(&target.binding),
            bir::Callee::Helper(_) => Err("a helper call carries no declared argument binding".into()),
        }
    }

    /// A resolved binding's two lists: the per-operand records, and the slots left to a default.
    type ResolvedBindingParts<'a> = (&'a [bir::BoundArgument], &'a [usize]);

    /// Return a call's resolved argument binding, failing when the call recorded no declared-slot binding.
    ///
    /// Insisting on [`bir::ArgumentBinding::Resolved`] is the point: a test that accepted
    /// `UnresolvedPositional` would silently pass against an implementation that stopped binding named arguments.
    fn resolved_binding(kind: &bir::StatementKind) -> Result<ResolvedBindingParts<'_>, Box<dyn std::error::Error>> {
        match call_binding(kind)? {
            bir::ArgumentBinding::Resolved {
                arguments,
                defaulted_slots,
            } => Ok((arguments, defaulted_slots)),
            bir::ArgumentBinding::UnresolvedPositional => {
                Err("expected a resolved declared-slot binding, found an unresolved positional call".into())
            }
        }
    }

    /// Return the named body from a lowered module.
    fn body_named<'a>(module: &'a bir::BodyIrModule, name: &str) -> Result<&'a bir::Body, Box<dyn std::error::Error>> {
        module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or_else(|| format!("body `{name}` missing from the lowered module").into())
    }

    #[test]
    fn named_construction_lowers_to_a_constructor_aggregate_with_a_resolved_field_binding()
    -> Result<(), Box<dyn std::error::Error>> {
        // The canonical README spelling. Before #1158 this was the *only* accepted construction spelling and it
        // lowered to `unsupported`, so no nominal value was representable in Body IR at all.
        let source = "model P:\n  x: int\n  y: int\n\ndef make() -> P:\n  return P(x=1, y=2)\n";
        let module = build(source, &["m", "ctor"])?;
        let snapshot = module.render_snapshot();
        assert_eq!(
            snapshot,
            build(source, &["m", "ctor"])?.render_snapshot(),
            "lowering must be deterministic"
        );

        assert!(
            !snapshot.contains("unsupported("),
            "named construction must lower to real Body IR: {snapshot}"
        );
        assert!(
            snapshot.contains("constructor(P)[const(1), const(2)]"),
            "construction must lower to a constructor aggregate in declared field order: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn out_of_order_named_construction_binds_by_field_and_records_written_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "model P:\n  x: int\n  y: int\n\ndef make() -> P:\n  return P(y=2, x=1)\n";
        let module = build(source, &["m", "ctor_order"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "out-of-order named construction must lower: {snapshot}"
        );
        // Operands follow declared field order (`x` then `y`) even though the source wrote `y` first, and the
        // written order is recorded rather than discarded.
        assert!(
            snapshot.contains("constructor(P) written=[1, 0][const(1), const(2)]"),
            "field binding must reorder operands while preserving the written order fact: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn construction_records_an_omitted_field_default_as_an_explicit_slot() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model P:\n  x: int\n  y: int = 5\n\ndef make() -> P:\n  return P(x=1)\n";
        let module = build(source, &["m", "ctor_default"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "construction omitting a defaulted field must lower: {snapshot}"
        );
        // The default's *computation* stays owned by the declaration; the call site records only that slot 1 took it.
        assert!(
            snapshot.contains("constructor(P) defaults=[1][const(1)]"),
            "an omitted field must be recorded as a defaulted slot, not left implicit: {snapshot}"
        );
        Ok(())
    }

    /// Retain the exact local model layout a direct executor needs instead of treating a constructor spelling as an
    /// identity.
    #[test]
    fn source_local_model_construction_retains_its_declaration_identity_and_canonical_field_layout()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "model Pair:\n  left: int\n  right: int\n\ndef main() -> int:\n  pair = Pair(right=2, left=40)\n  return pair.left + pair.right\n";
        let module = build(source, &["m", "nominal_identity"])?;
        let declaration = match module.nominal_declarations.as_slice() {
            [declaration] => declaration,
            declarations => {
                return Err(format!("expected one retained local model declaration, found {declarations:?}").into());
            }
        };
        assert_eq!(declaration.name, "Pair");
        assert_eq!(declaration.fields, vec!["left", "right"]);
        assert_eq!(declaration.type_parameter_count, 0);

        let body = body_named(&module, "main")?;
        let target = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Assign {
                    rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _),
                    ..
                } => Some(target),
                _ => None,
            })
            .ok_or("the local model construction must lower as a constructor aggregate")?;
        assert_eq!(target.name, "Pair");
        assert_eq!(
            target.direct_declaration_id.as_ref(),
            Some(&declaration.direct_declaration_id)
        );
        let bir::ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        } = &target.binding
        else {
            return Err("local model construction must retain its resolved field binding".into());
        };
        assert!(defaulted_slots.is_empty());
        assert_eq!(
            arguments
                .iter()
                .map(|argument| (argument.slot, argument.written_position))
                .collect::<Vec<_>>(),
            vec![(0, 1), (1, 0)],
            "constructor operands retain declaration slots while written positions retain source evaluation order"
        );
        Ok(())
    }

    /// Retain the exact local value-enum member selected by source lowering rather than recovering it from a
    /// qualified spelling in a direct runtime.
    #[test]
    fn source_local_value_enum_member_retains_exact_enum_and_variant_identities()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "enum HttpStatus(int):\n  Ok = 200\n  NotFound = 404\n\ndef main() -> int:\n  return HttpStatus.NotFound.value()\n";
        let module = build(source, &["m", "value_enum_identity"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("value_enum HttpStatus id=decl:m::value_enum_identity#decl."),
            "the module must retain the source-local enum declaration identity: {snapshot}"
        );
        assert!(
            snapshot.contains("variant NotFound id=decl:m::value_enum_identity#decl."),
            "the module must retain the source-local member declaration identity: {snapshot}"
        );
        assert!(
            snapshot.contains("value_enum_variant(HttpStatus::NotFound"),
            "the member expression must lower to an identity-bearing rvalue instead of an external field place: {snapshot}"
        );
        Ok(())
    }

    /// Retain the exact local fieldless normal-enum member selected by source lowering rather than treating a
    /// qualified spelling as a value any backend may recover.
    #[test]
    fn source_local_fieldless_enum_member_retains_exact_enum_and_variant_identities()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "enum Signal:\n  Ready\n  Stop\n\ndef main() -> bool:\n  return Signal.Ready == Signal.Stop\n";
        let module = build(source, &["m", "fieldless_enum_identity"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("fieldless_enum Signal id=decl:m::fieldless_enum_identity#decl."),
            "the module must retain the source-local enum declaration identity: {snapshot}"
        );
        assert!(
            snapshot.contains("variant Ready id=decl:m::fieldless_enum_identity#decl."),
            "the module must retain the source-local member declaration identity: {snapshot}"
        );
        assert!(
            snapshot.contains("fieldless_enum_variant(Signal::Ready"),
            "the member expression must lower to an identity-bearing rvalue instead of an external field place: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn mixed_positional_and_named_call_arguments_bind_to_declared_parameters() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(1, b=2)\n";
        let module = build(source, &["m", "mixed"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "a mixed positional/named call must lower: {snapshot}"
        );
        assert!(
            snapshot.contains("call fn:add(const(1), const(2))"),
            "a mixed call binding in declaration order needs no slot map: {snapshot}"
        );
        // The rendered string alone would also match an implementation that ignored named binding entirely and
        // lowered arguments in written order, so assert the resolved binding itself.
        let (arguments, defaulted_slots) = resolved_binding(single_call(body_named(&module, "use")?)?)?;
        assert!(defaulted_slots.is_empty(), "nothing was omitted: {defaulted_slots:?}");
        assert_eq!(
            arguments
                .iter()
                .map(|argument| (argument.slot, argument.written_position))
                .collect::<Vec<_>>(),
            vec![(0, 0), (1, 1)],
            "`b=2` must resolve to declared slot 1 rather than being taken positionally: {arguments:?}"
        );
        Ok(())
    }

    #[test]
    fn out_of_order_named_call_arguments_evaluate_in_written_source_order() -> Result<(), Box<dyn std::error::Error>> {
        // The effect-ordering contract: `g()` is written first, so it must be *called* first, even though its value
        // binds to the later declared parameter. A consumer executing operands in slot order would swap the effects.
        let source = "def f() -> int:\n  return 1\n\ndef g() -> int:\n  return 2\n\ndef add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(b=g(), a=f())\n";
        let module = build(source, &["m", "written_order"])?;
        let snapshot = module.render_snapshot();
        let use_body = body_named(&module, "use")?;
        let rendered = use_body.render_snapshot();

        let g_at = rendered.find("call fn:g(").ok_or("missing call to g")?;
        let f_at = rendered.find("call fn:f(").ok_or("missing call to f")?;
        assert!(
            g_at < f_at,
            "argument sub-expressions must be evaluated in written source order: {rendered}"
        );
        assert!(
            rendered.contains("written=[1, 0]"),
            "the written order must be recorded on the call, not merely implied by statement order: {rendered}"
        );
        assert!(
            !snapshot.contains("unsupported("),
            "no part of this program should refuse: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn an_omitted_defaulted_argument_is_recorded_as_a_defaulted_slot() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def add(a: int, b: int = 2) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(1)\n";
        let module = build(source, &["m", "call_default"])?;
        let use_body = body_named(&module, "use")?;
        let (arguments, defaulted_slots) = resolved_binding(single_call(use_body)?)?;

        assert_eq!(
            defaulted_slots,
            [1],
            "an omitted default must be an explicit call-site fact: {defaulted_slots:?}"
        );
        assert_eq!(
            arguments.len(),
            1,
            "only the supplied argument gets an operand: {arguments:?}"
        );
        Ok(())
    }

    #[test]
    fn an_omitted_interior_default_binds_without_compacting_later_arguments() -> Result<(), Box<dyn std::error::Error>>
    {
        // #1124 had to refuse this: a flat operand vector could not say that `9` fills slot 2 rather than slot 1.
        // The recorded binding is exactly that sparse argument map, so the call is now representable.
        let source = "def at(a: int, b: int = 2, c: int = 3) -> int:\n  return a + b + c\n\ndef use() -> int:\n  return at(1, c=9)\n";
        let module = build(source, &["m", "interior_default"])?;
        let snapshot = module.render_snapshot();
        let use_body = body_named(&module, "use")?;
        let (arguments, defaulted_slots) = resolved_binding(single_call(use_body)?)?;

        assert!(
            !snapshot.contains("unsupported("),
            "an interior default hole must now lower: {snapshot}"
        );
        assert_eq!(
            defaulted_slots,
            [1],
            "slot 1 takes its declared default: {defaulted_slots:?}"
        );
        assert_eq!(
            arguments.iter().map(|argument| argument.slot).collect::<Vec<_>>(),
            vec![0, 2],
            "the supplied operands must keep their real declaration slots: {arguments:?}"
        );
        Ok(())
    }

    #[test]
    fn method_call_named_arguments_bind_after_the_borrowed_receiver() -> Result<(), Box<dyn std::error::Error>> {
        let source = "class C:\n  def add(self, a: int, b: int) -> int:\n    return a + b\n\ndef use(c: C) -> int:\n  return c.add(b=2, a=1)\n";
        let module = build(source, &["m", "method_named"])?;
        let snapshot = module.render_snapshot();
        let use_body = body_named(&module, "use")?;
        let (arguments, _) = resolved_binding(single_call(use_body)?)?;

        assert!(
            !snapshot.contains("unsupported("),
            "a named method call must lower: {snapshot}"
        );
        // The receiver stays `args[0]` and is deliberately outside the binding, whose slots index the method's own
        // declared parameters.
        assert_eq!(
            arguments.iter().map(|argument| argument.slot).collect::<Vec<_>>(),
            vec![0, 1],
            "method argument slots must index declared parameters, not the receiver: {arguments:?}"
        );
        assert_eq!(
            arguments
                .iter()
                .map(|argument| argument.written_position)
                .collect::<Vec<_>>(),
            vec![1, 0],
            "the written order of `b=2, a=1` must survive the reorder into declaration order: {arguments:?}"
        );
        assert!(
            use_body.render_snapshot().contains("borrow(_0)"),
            "the receiver must still lower as a borrowed first argument: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn explicit_call_site_type_arguments_survive_lowering() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def pick[T](v: T) -> T:\n  return v\n\ndef use() -> int:\n  return pick[int](1)\n";
        let module = build(source, &["m", "generic_call"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "an explicitly generic call must lower: {snapshot}"
        );
        assert!(
            snapshot.contains("call fn:pick[int](const(1))"),
            "resolved call-site type arguments belong to the callee's identity: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn explicit_method_call_type_arguments_survive_lowering() -> Result<(), Box<dyn std::error::Error>> {
        // The other half of `CallSiteGenerics`' canonical surface: `session.read_csv[Order](path)`. The typechecker
        // substitutes the receiver's generics before recording the signature, so the method's slots are already
        // concrete here and the resolved type argument still has to reach the callee.
        let source = "class S:\n  def read[T](self, v: T) -> T:\n    return v\n\ndef use(s: S) -> int:\n  return s.read[int](1)\n";
        let module = build(source, &["m", "generic_method"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "an explicitly generic method call must lower: {snapshot}"
        );
        assert!(
            snapshot.contains("call method:read[int]("),
            "a method call's resolved type arguments belong to its callee identity: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn direct_and_local_named_binding_go_through_one_mechanism() -> Result<(), Box<dyn std::error::Error>> {
        // #1158's "one mechanism" criterion: the direct `Callee::Function` path and the #1124 local-callable path
        // must produce the same binding facts for the same spelling, not merely both succeed.
        // Deliberately an out-of-order spelling: its binding is *not* the identity, so this cannot be satisfied by
        // two independent mechanisms that merely agree on the trivial case, nor by a path that never bound at all.
        let direct = "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(b=2, a=1)\n";
        let local =
            "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  g = add\n  return g(b=2, a=1)\n";

        let direct_module = build(direct, &["m", "one_direct"])?;
        let local_module = build(local, &["m", "one_local"])?;
        let direct_binding = call_binding(single_call(body_named(&direct_module, "use")?)?)?;
        let local_binding = call_binding(single_call(body_named(&local_module, "use")?)?)?;

        assert_eq!(
            direct_binding, local_binding,
            "a direct call and a local-callable call must resolve one spelling identically"
        );
        let bir::ArgumentBinding::Resolved { arguments, .. } = direct_binding else {
            return Err("the shared mechanism must produce a resolved binding, not a positional fallback".into());
        };
        assert_eq!(
            arguments
                .iter()
                .map(|argument| (argument.slot, argument.written_position))
                .collect::<Vec<_>>(),
            vec![(0, 1), (1, 0)],
            "the shared binding must be the non-trivial one this spelling implies: {arguments:?}"
        );
        Ok(())
    }

    #[test]
    fn an_overloaded_call_binds_against_the_declaration_the_typechecker_selected()
    -> Result<(), Box<dyn std::error::Error>> {
        // Regression: `function_bindings` is keyed by bare name, so it holds only one of two same-name
        // declarations. Binding against the wrong overload's parameter *names* silently swaps the arguments --
        // a wrong answer where the previous refusal was at least honest.
        let source = "def pick(a: int, b: int) -> int:\n  return a - b\n\ndef pick(b: str, a: str) -> str:\n  return a\n\ndef use() -> int:\n  return pick(a=10, b=1)\n";
        let module = build(source, &["m", "overload"])?;
        let use_body = body_named(&module, "use")?;
        let rendered = use_body.render_snapshot();
        let (arguments, _) = resolved_binding(single_call(use_body)?)?;

        // The selected overload is `pick(a: int, b: int)`, so `a=10` fills slot 0 and `b=1` fills slot 1. Binding
        // against the *second* declaration would map `a` to slot 1 and emit the operands as `const(1), const(10)`.
        assert_eq!(
            arguments
                .iter()
                .map(|argument| (argument.slot, argument.written_position))
                .collect::<Vec<_>>(),
            vec![(0, 0), (1, 1)],
            "the call must bind against the overload the typechecker selected: {arguments:?}"
        );
        assert!(
            rendered.contains("call fn:pick(const(10), const(1))"),
            "operands must follow the selected overload's declaration order: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn an_overloaded_call_retains_the_typechecker_selected_same_module_declaration_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "def pick(a: int, b: int) -> int:\n  return a - b\n\ndef pick(b: str, a: str) -> str:\n  return a\n\ndef use() -> int:\n  return pick(a=10, b=1)\n";
        let module = build(source, &["m", "overload_identity"])?;
        let use_body = body_named(&module, "use")?;
        let bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            ..
        } = single_call(use_body)?
        else {
            return Err("expected an identity-selected named function call".into());
        };
        let target_id = target
            .direct_call_id
            .as_ref()
            .ok_or("same-module overloaded call must retain a direct declaration identity")?;
        let selected = module
            .bodies
            .iter()
            .find(|body| body.direct_call_id == *target_id)
            .ok_or("direct call identity must select a Body-IR declaration")?;

        assert_eq!(selected.name, "pick");
        assert!(
            selected.render_snapshot().contains("local 0 a : int [param]"),
            "the direct identity must select the integer overload: {}",
            selected.render_snapshot()
        );
        Ok(())
    }

    #[test]
    fn an_overload_set_that_changes_arity_does_not_refuse_a_valid_call() -> Result<(), Box<dyn std::error::Error>> {
        // The other half of the same defect: with the two-parameter declaration written first, a name-keyed lookup
        // could resolve `pick(1, 2)` against the one-parameter overload and refuse a call the typechecker accepted.
        let source = "def pick(a: int, b: int) -> int:\n  return a + b\n\ndef pick(a: str) -> str:\n  return a\n\ndef use() -> int:\n  return pick(1, 2)\n";
        let module = build(source, &["m", "overload_arity"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "a call the typechecker accepted must not be refused by overload confusion: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn a_rest_parameter_callee_still_lowers_its_positional_arguments() -> Result<(), Box<dyn std::error::Error>> {
        // Variadics are a delivered language capability. Routing the direct path through the shared planner must not
        // silently narrow what Body IR represents; the call keeps lowering, it simply makes no declared-slot claim.
        let source = "def total(a: int, *xs: int) -> int:\n  return a\n\ndef use() -> int:\n  return total(1, 2, 3)\n";
        let module = build(source, &["m", "rest"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "a positional call into a rest-parameter signature must still lower: {snapshot}"
        );
        assert!(
            snapshot.contains("call fn:total unbound(const(1), const(2), const(3))"),
            "a rest signature has no one-to-one declared slots, so the binding must say so: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn argument_ownership_facts_are_sequenced_by_written_order_not_operand_index()
    -> Result<(), Box<dyn std::error::Error>> {
        // The invariant `ArgumentBinding` documents: operands are reordered into declaration order, but their
        // ownership facts were decided in written order. Read left to right this vector moves `_0` and then clones
        // it; `written=[1, 0]` is what tells a consumer the clone happened first.
        let source =
            "def two(p: str, q: str) -> str:\n  return p + q\n\ndef use(a: str) -> str:\n  return two(q=a, p=a)\n";
        let module = build(source, &["m", "own_order"])?;
        let rendered = body_named(&module, "use")?.render_snapshot();

        assert!(
            rendered.contains("call fn:two written=[1, 0](move(_0, last_use), clone(_0))"),
            "ownership facts must stay sequenced by written order: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn class_construction_binds_inherited_fields_in_declared_layout_order() -> Result<(), Box<dyn std::error::Error>> {
        // Constructor ABI order puts the parent's fields first. A subclass construction must bind against that
        // flattened order, not against the subclass's own declarations alone.
        let source = "class Base:\n  a: int\n\nclass Sub extends Base:\n  b: int = 7\n\ndef make() -> Sub:\n  return Sub(b=1, a=2)\n";
        let module = build(source, &["m", "subclass"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !snapshot.contains("unsupported("),
            "subclass construction must lower: {snapshot}"
        );
        assert!(
            snapshot.contains("constructor(Sub) written=[1, 0][const(2), const(1)]"),
            "inherited fields come first in constructor layout order: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn a_construction_the_checker_declined_to_bind_is_refused_as_a_construction()
    -> Result<(), Box<dyn std::error::Error>> {
        // A duplicate field leaves no recorded binding. Falling through to the direct-call path would refuse this as
        // a call to an unknown function, naming the wrong construct entirely.
        let source = "model P:\n  x: int = 1\n  y: int = 2\n\ndef make() -> P:\n  return P(x=1, x=2)\n";
        let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "dup_field"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !diagnostics.is_empty(),
            "the typechecker must reject a duplicated field first"
        );
        assert!(
            snapshot.contains("construction of `P` with an unresolved field layout"),
            "a refused construction must be named as a construction: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn an_argument_spread_is_refused_by_name_rather_than_as_a_generic_call_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        // The typechecker rejects these first; lowering must stay fail-closed and name the specific spelling, since
        // #1159 owns spread representation while #1158 owns named binding.
        let source =
            "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use(xs: List[int]) -> int:\n  return add(*xs)\n";
        let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "spread"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !diagnostics.is_empty(),
            "the source checker must reject an unmatched positional spread first"
        );
        assert!(
            snapshot.contains("positional argument spread"),
            "a spread must be refused as a spread, not as a named-argument failure: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn a_named_argument_with_no_matching_parameter_is_refused_by_name() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(a=1, zz=2)\n";
        let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "bad_named"])?;
        let snapshot = module.render_snapshot();

        assert!(
            !diagnostics.is_empty(),
            "the typechecker must reject an unknown parameter name first"
        );
        assert!(
            snapshot.contains("has no parameter `zz`"),
            "lowering must name the unresolvable parameter rather than accepting it silently: {snapshot}"
        );
        Ok(())
    }

    // ========================================================================
    // #1164 -- `await` and `race for`
    // ========================================================================

    const ASYNC_PRELUDE: &str =
        "import std.async\n\nasync def fast() -> int:\n  return 1\n\nasync def slow() -> int:\n  return 2\n\n";

    #[test]
    fn lowers_await_as_an_explicit_suspension_point_with_a_destination() -> Result<(), Box<dyn std::error::Error>> {
        let source = format!("{ASYNC_PRELUDE}async def f() -> int:\n  v = await fast()\n  return v\n");
        let module = build(&source, &["m", "await_one"])?;
        let snapshot = module.render_snapshot();
        assert_eq!(
            snapshot,
            build(&source, &["m", "await_one"])?.render_snapshot(),
            "lowering must be deterministic"
        );

        assert!(!snapshot.contains("unsupported("), "await must lower: {snapshot}");
        // The suspension carries a destination and the awaited operand's own ownership fact -- the two facts that
        // distinguish it from a generator `yield`, which produces outward and has no destination.
        assert!(
            snapshot.contains("_1 = await copy(_0, last_use)"),
            "await must record its destination and the awaited read's ownership fact: {snapshot}"
        );
        assert!(
            !snapshot.contains("yield"),
            "await must not be represented as a generator yield: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn records_the_async_runtime_requirement_on_the_awaiting_body() -> Result<(), Box<dyn std::error::Error>> {
        let source = format!("{ASYNC_PRELUDE}async def f() -> int:\n  return await fast()\n");
        let module = build(&source, &["m", "await_req"])?;
        let rendered = body_named(&module, "f")?.render_snapshot();

        assert!(
            rendered.contains("async_runtime"),
            "the requirement must be recorded on the awaiting body itself, not merely somewhere in the module: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn an_async_body_without_any_await_is_still_marked_async() -> Result<(), Box<dyn std::error::Error>> {
        // The reason `is_async` is a stored declaration fact rather than derived the way `is_generator` is: this
        // body contains no `await` at all, yet its caller still gets an awaitable. Deriving async-ness by scanning
        // for a suspension point would report this body as synchronous.
        let source = "import std.async\n\nasync def f() -> int:\n  return 1\n";
        let module = build(source, &["m", "async_plain"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("body async f"),
            "an `async def` with no await must still be marked async: {snapshot}"
        );
        assert!(
            !snapshot.contains("await "),
            "this body genuinely contains no suspension point, so the async fact cannot have been derived from one: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn a_synchronous_body_is_not_marked_async() -> Result<(), Box<dyn std::error::Error>> {
        let module = build("def f() -> int:\n  return 1\n", &["m", "sync"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("body f "),
            "a plain function must render unmarked: {snapshot}"
        );
        assert!(
            !snapshot.contains("body async"),
            "a synchronous body must not be marked async: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn sequential_awaits_keep_their_effect_ordering_across_suspension() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            format!("{ASYNC_PRELUDE}async def f() -> int:\n  x = await fast()\n  y = await slow()\n  return x + y\n");
        let module = build(&source, &["m", "await_seq"])?;
        let rendered = body_named(&module, "f")?.render_snapshot();

        assert!(!rendered.contains("unsupported("), "both awaits must lower: {rendered}");
        let first = rendered.find("call fn:fast(").ok_or("missing first awaitable")?;
        let first_await = rendered.find("await ").ok_or("missing first suspension")?;
        let second = rendered.find("call fn:slow(").ok_or("missing second awaitable")?;
        assert!(
            first < first_await && first_await < second,
            "statements before a suspension must precede it and statements after must follow it: {rendered}"
        );
        assert_eq!(
            rendered.matches("await ").count(),
            2,
            "each source `await` must produce its own suspension point: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn await_inside_a_branch_stays_inside_that_branch() -> Result<(), Box<dyn std::error::Error>> {
        let source = format!(
            "{ASYNC_PRELUDE}async def f(flag: bool) -> int:\n  total = 0\n  if flag:\n    total = await fast()\n  else:\n    total = 7\n  return total\n"
        );
        let module = build(&source, &["m", "await_branch"])?;
        let rendered = body_named(&module, "f")?.render_snapshot();

        assert!(
            !rendered.contains("unsupported("),
            "await in a branch must lower: {rendered}"
        );
        let branch_line = rendered
            .lines()
            .find(|line| line.contains("await "))
            .ok_or("missing suspension")?;
        assert!(
            branch_line.starts_with("    "),
            "the suspension must stay nested inside the branch block: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn await_inside_a_loop_stays_inside_the_loop_body() -> Result<(), Box<dyn std::error::Error>> {
        let source = format!(
            "{ASYNC_PRELUDE}async def f() -> int:\n  total = 0\n  i = 0\n  while i < 3:\n    total = total + await fast()\n    i = i + 1\n  return total\n"
        );
        let module = build(&source, &["m", "await_loop"])?;
        let rendered = body_named(&module, "f")?.render_snapshot();

        assert!(
            !rendered.contains("unsupported("),
            "await in a loop must lower: {rendered}"
        );
        let await_line = rendered
            .lines()
            .find(|line| line.contains("await "))
            .ok_or("missing suspension")?;
        assert!(
            await_line.starts_with("    "),
            "the suspension must stay inside the loop body: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn lowers_a_two_arm_race_with_per_arm_bindings_and_pre_selection_awaitables()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = format!(
            "{ASYNC_PRELUDE}async def f() -> int:\n  race for value:\n    await fast() => value\n    await slow() => value\n"
        );
        let module = build(&source, &["m", "race_two"])?;
        let rendered = body_named(&module, "f")?.render_snapshot();

        assert!(
            !rendered.contains("unsupported("),
            "a two-arm race must lower: {rendered}"
        );
        // Every awaitable is evaluated before selection, in source order -- observable here as both calls being
        // emitted ahead of the race statement rather than inside an arm.
        let fast_at = rendered.find("call fn:fast(").ok_or("missing first awaitable")?;
        let slow_at = rendered.find("call fn:slow(").ok_or("missing second awaitable")?;
        let race_at = rendered.find("race:").ok_or("missing race statement")?;
        assert!(
            fast_at < slow_at && slow_at < race_at,
            "all arm awaitables must be evaluated, in source order, before selection: {rendered}"
        );
        // The source spells one binding name, but each arm re-scopes it, so each arm owns its own local.
        assert_eq!(
            rendered.matches("value : int [binding]").count(),
            2,
            "each arm must bind its own local rather than sharing one: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn a_race_arm_binding_does_not_escape_its_arm() -> Result<(), Box<dyn std::error::Error>> {
        // The arm binding shadows an enclosing name only for the duration of its own arm. Restoring it is the same
        // discipline `lower_closure` follows, and getting it wrong is silent: reads after the race would resolve to
        // the last arm's local, so the body would compute the wrong value with no unsupported node to show for it.
        let source = format!(
            "{ASYNC_PRELUDE}async def f() -> int:\n  value = 100\n  winner = race for value:\n    await fast() => value\n    await slow() => value\n  return value + winner\n"
        );
        let module = build(&source, &["m", "race_shadow"])?;
        let body = body_named(&module, "f")?;
        let rendered = body.render_snapshot();

        // `value` is declared first, so it is local 0; the trailing `value + winner` must read exactly that local.
        let outer = local_for_binding(&rendered, "value").ok_or("missing outer binding")?;
        assert_eq!(
            outer, "_0",
            "the outer binding should be the first declared local: {rendered}"
        );
        let sum_line = rendered
            .lines()
            .find(|line| line.contains(" + "))
            .ok_or("missing the trailing sum")?;
        assert!(
            sum_line.contains("copy(_0)"),
            "a read after the race must resolve to the enclosing binding, not an arm local: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn a_block_arm_local_does_not_leak_past_its_arm() -> Result<(), Box<dyn std::error::Error>> {
        // A block arm lowers ordinary statements, so `let total = ...` inside it declares a lexical local through
        // the same path any assignment uses. Restoring only the shared race binding would leave that arm-local
        // installed, and the trailing read of `total` would silently resolve to it instead of the outer binding --
        // a wrong value with no unsupported node to show for it.
        let source = format!(
            "{ASYNC_PRELUDE}async def f() -> int:\n  let total = 100\n  winner = race for value:\n    await fast() => value\n    await slow() =>\n      let total = value * 2\n      total\n  return total + winner\n"
        );
        let module = build(&source, &["m", "race_arm_local"])?;
        let body = body_named(&module, "f")?;
        let rendered = body.render_snapshot();

        // The outer binding is distinct from the arm's `total`, and the post-race expression must use that outer
        // local regardless of earlier parameters or temporaries that might affect local numbering.
        let outer = local_for_binding(&rendered, "total").ok_or("missing outer binding")?;
        assert!(
            rendered.matches("total : int [binding]").count() >= 2,
            "the arm must declare its own `total` rather than reusing the outer one: {rendered}"
        );
        let sum_line = rendered
            .lines()
            .find(|line| line.contains(" + ") && !line.starts_with("      "))
            .ok_or("missing the trailing sum")?;
        assert!(
            sum_line.contains(&format!("copy({outer})")),
            "a read after the race must resolve to the enclosing local, not one an arm body declared: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn a_race_arm_block_body_lowers_its_statements_and_trailing_value() -> Result<(), Box<dyn std::error::Error>> {
        let source = format!(
            "{ASYNC_PRELUDE}async def f() -> int:\n  race for value:\n    await fast() => value\n    await slow() =>\n      doubled = value * 2\n      doubled\n"
        );
        let module = build(&source, &["m", "race_block"])?;
        let rendered = body_named(&module, "f")?.render_snapshot();

        assert!(
            !rendered.contains("unsupported("),
            "a block arm body must lower: {rendered}"
        );
        // The arm body's statements live inside the arm, indented under it -- only the winning arm runs, so they
        // must not be hoisted into the enclosing block alongside the awaitables.
        let arm_stmt = rendered
            .lines()
            .find(|line| line.contains("* const(2)"))
            .ok_or("missing the arm body computation")?;
        assert!(
            arm_stmt.starts_with("      "),
            "an arm body statement must stay nested inside its arm: {rendered}"
        );
        // The block's trailing expression becomes the arm's result, not merely a statement inside it.
        assert!(
            rendered.contains("-> copy(_5)"),
            "the block's trailing expression must become the arm's result operand: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn an_unsupported_construct_in_a_race_arm_does_not_collapse_the_whole_race()
    -> Result<(), Box<dyn std::error::Error>> {
        // The issue's explicit requirement: a construct Body IR cannot represent keeps its own node *inside* a
        // represented race, so a consumer loses only that construct rather than the entire expression.
        let source = format!(
            "{ASYNC_PRELUDE}async def f() -> int:\n  race for value:\n    await fast() => value\n    await slow() => value ** 2\n"
        );
        let module = build(&source, &["m", "race_partial"])?;
        let rendered = body_named(&module, "f")?.render_snapshot();

        assert!(
            rendered.contains("race:"),
            "the race itself must still be represented: {rendered}"
        );
        // Asserting the node exists somewhere in the body would also pass if it had been hoisted into the enclosing
        // block -- the exact regression this test exists to catch -- so require it to be indented inside an arm.
        let refusal = rendered
            .lines()
            .find(|line| line.contains("unsupported("))
            .ok_or("missing the refusal for the unrepresentable arm construct")?;
        assert!(
            refusal.starts_with("      "),
            "the refusal must stay inside its arm rather than collapsing or escaping the race: {rendered}"
        );
        Ok(())
    }

    #[test]
    fn an_async_method_body_is_marked_async() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import std.async\n\nclass C:\n  async def m(self) -> int:\n    return 1\n";
        let module = build(source, &["m", "async_method"])?;
        let snapshot = module.render_snapshot();

        assert!(
            snapshot.contains("body async m"),
            "an async method body must carry the same async fact as an async function: {snapshot}"
        );
        Ok(())
    }

    #[test]
    fn a_prefix_surface_keyword_that_is_not_await_is_refused_rather_than_treated_as_a_suspension() {
        // `SurfaceExprPayload::PrefixUnary` is generic over any prefix soft keyword; `await` is merely the only one
        // registered today. Dispatching on the payload alone would silently lower a future prefix keyword as a
        // suspension point, so lowering matches the surface *key*. This pins that.
        let type_info = TypeCheckInfo::default();
        let function_default_sources = FunctionDefaultSources::new();
        let local_function_declarations = LocalFunctionDeclarations::new();
        let local_nominal_declarations = LocalNominalDeclarations::new();
        let local_fieldless_enum_declarations = LocalFieldlessEnumDeclarations::new();
        let local_value_enum_declarations = LocalValueEnumDeclarations::new();
        let mut builder = BodyBuilder::new(
            &type_info,
            &function_default_sources,
            &local_function_declarations,
            &local_nominal_declarations,
            &local_fieldless_enum_declarations,
            &local_value_enum_declarations,
            "m",
            IncanType::Unknown,
        );
        let scope = builder.new_scope(None, HirSourceSpan::new(0, 1));
        let mut out = Vec::new();
        let surface = ast::SurfaceExpr {
            key: SurfaceFeatureKey::SoftKeyword(KeywordId::Async),
            payload: ast::SurfaceExprPayload::PrefixUnary(Box::new(ast::Spanned::new(
                ast::Expr::Ident("placeholder".to_string()),
                ast::Span::new(0, 1),
            ))),
        };

        let _ = builder.lower_surface_expr(&surface, ast::Span::new(0, 1), scope, &mut out);

        assert!(
            out.iter().any(|stmt| matches!(
                &stmt.kind,
                bir::StatementKind::Unsupported { description } if description.contains("prefix-keyword")
            )),
            "a non-`await` prefix keyword must keep the generic surface refusal, not become an await: {out:?}"
        );
        assert!(
            !out.iter()
                .any(|stmt| matches!(&stmt.kind, bir::StatementKind::Await { .. })),
            "no suspension point may be emitted for a keyword that is not `await`: {out:?}"
        );
    }
}

#[cfg(test)]
mod tuple_destructure_interop_tests {
    use super::{IncanType, unsupported_tuple_destructure};

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
