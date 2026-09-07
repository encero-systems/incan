//! Building one `Body` per lowered function or method declaration, including receiver resolution.

use super::*;

/// Lower every non-abstract method in `methods` (owned by the declaration named `owner_name`) into one
/// [`bir::Body`] each, skipping abstract methods (`body: None`). `receiver_ty` is the typechecker-equivalent type
/// for a declared receiver: a concrete nominal type for models, classes, newtypes, and enums, or
/// [`IncanType::SelfType`] for trait defaults.
///
/// Exactly five declaration kinds carry a `methods` field -- model, class, trait, newtype, and enum (see
/// `crates/incan_syntax/src/ast/decls.rs`) -- and all five reach this function. No kind that carries methods is
/// skipped, which matters because a skipped kind is the one failure this module cannot make visible: every other
/// unsupported construct leaves a `StatementKind::Unsupported` or `Operand::Unknown` marker behind, while a skipped
/// declaration produces no [`bir::Body`] at all and a consumer counting bodies reads the program as fully
/// represented. `every_declaration_kind_that_carries_methods_lowers_its_bodies` pins that.
pub(super) fn lower_owner_method_bodies(
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
/// Lower one function declaration's body into Body IR v0.
pub(super) fn lower_function_body(
    function: &ast::FunctionDecl,
    decl_span: ast::Span,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> bir::Body {
    let direct_call_id =
        CompilerNodeId::declaration_span(lowering_facts.module_identity, decl_span.start, decl_span.end);
    let decl_id = direct_call_id.clone();
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

    let mut builder = BodyBuilder::new(lowering_facts, owner_return_type.clone());
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
        canonical: binding.and_then(|binding| binding.identity.clone()),
        name: function.name.clone(),
        span: hir_span(decl_span),
        return_type: owner_return_type,
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
pub(super) fn lower_method_body(
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

    let mut builder = BodyBuilder::new(lowering_facts, owner_return_type.clone());
    let root_scope = builder.new_scope(None, hir_span(decl_span));

    let mut params = Vec::with_capacity(method.params.len() + 1);
    let mut param_locals = Vec::with_capacity(method.params.len() + 1);
    if let Some(receiver) = method.receiver {
        let mutable = matches!(receiver, ast::Receiver::Mutable);
        let receiver_span = method
            .receiver_binding
            .as_ref()
            .map_or(decl_span, |binding| binding.span);
        let self_local =
            builder.declare_receiver_local(receiver_ty.clone(), mutable, root_scope, hir_span(receiver_span));
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
        canonical: binding.and_then(|binding| binding.identity.clone()),
        name: method.name.clone(),
        span: hir_span(decl_span),
        return_type: owner_return_type,
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
pub(super) fn owner_self_type(owner_name: &str, owner_type_params: &[ast::TypeParam]) -> IncanType {
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
