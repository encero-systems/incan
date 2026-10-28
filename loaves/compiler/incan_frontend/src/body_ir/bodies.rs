//! Building one `Body` per lowered function or method declaration, including receiver resolution.

use super::*;
use crate::symbols::ResolvedType;
use crate::typechecker::FunctionBindingInfo;
use incan_lang::lang::decorators::{self, DecoratorId};

/// Retain a nominal owner's methods and concrete property getters under their independent checked identities.
pub(super) fn lower_owner_member_bodies(
    methods: &[ast::Spanned<ast::MethodDecl>],
    properties: &[ast::Spanned<ast::PropertyDecl>],
    owner_name: &str,
    receiver_ty: IncanType,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> Vec<bir::Body> {
    let mut bodies = lower_owner_method_bodies(methods, owner_name, receiver_ty.clone(), lowering_facts);
    bodies.extend(lower_owner_property_bodies(
        properties,
        owner_name,
        receiver_ty,
        lowering_facts,
    ));
    bodies
}

/// Lower every non-abstract method in `methods` (owned by the declaration named `owner_name`) into one
/// [`bir::Body`] each, skipping abstract methods (`body: None`) and documented value-returning trait placeholders.
/// Unit no-ops and real default bodies remain executable. `receiver_ty` is the typechecker-equivalent type for a
/// declared receiver: a concrete nominal type for models, classes, newtypes, and enums, or
/// [`IncanType::SelfType`] for trait defaults.
///
/// Exactly five declaration kinds carry a `methods` field -- model, class, trait, newtype, and enum (see
/// `loaves/kernel/incan_syntax/src/ast/decls.rs`) -- and all five reach this function. No kind that carries methods is
/// skipped, which matters because a skipped kind is the one failure this module cannot make visible: every other
/// unsupported construct leaves a `StatementKind::Unsupported` or `Operand::Unknown` marker behind, while a skipped
/// declaration produces no [`bir::Body`] at all and a consumer counting bodies reads the program as fully
/// represented.
pub(super) fn lower_owner_method_bodies(
    methods: &[ast::Spanned<ast::MethodDecl>],
    owner_name: &str,
    receiver_ty: IncanType,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> Vec<bir::Body> {
    methods
        .iter()
        .filter_map(|method| {
            if documented_trait_value_placeholder(method, &receiver_ty, lowering_facts.type_info) {
                return None;
            }
            lower_method_body(&method.node, method.span, owner_name, &receiver_ty, lowering_facts)
        })
        .collect()
}

/// Keep a documented, value-returning trait placeholder as an interface slot rather than returning its documentation.
/// A real return and a valid unit no-op remain executable; the parser and legacy method/default tables are unchanged.
fn documented_trait_value_placeholder(
    method: &ast::Spanned<ast::MethodDecl>,
    receiver_ty: &IncanType,
    type_info: &TypeCheckInfo,
) -> bool {
    if receiver_ty != &IncanType::SelfType {
        return false;
    }
    let Some((first, remaining)) = method.node.body.as_deref().and_then(<[_]>::split_first) else {
        return false;
    };
    if !matches!(&first.node, ast::Statement::Expr(expr) if matches!(&expr.node, ast::Expr::Literal(ast::Literal::String(_))))
        || remaining.is_empty()
        || !remaining
            .iter()
            .all(|statement| matches!(statement.node, ast::Statement::Pass))
    {
        return false;
    }
    type_info
        .declarations
        .method_bindings_by_span
        .get(&method.span)
        .is_some_and(|binding| binding.return_type != ResolvedType::Unit)
}

/// Lower each computed property of a model or class into a receiver-only getter body, which a property read calls.
///
/// The canonical getter record supplies its immutable receiver, owner parameters and declared return type. Every
/// owner uses the same checked getter builder; abstract properties have no executable body and lower nothing.
pub(super) fn lower_owner_property_bodies(
    properties: &[ast::Spanned<ast::PropertyDecl>],
    owner_name: &str,
    receiver_ty: IncanType,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> Vec<bir::Body> {
    properties
        .iter()
        .filter_map(|property| super::properties::lower_getter_body(property, owner_name, &receiver_ty, lowering_facts))
        .collect()
}

/// The delegation an `@rust.extern` declaration named `name` records, or `None` for an ordinary declaration (#2023).
///
/// The item is the declaration's own name in the file's `rust.module(...)` module, for functions and static methods
/// alike, as the emitted route's delegation wrapper calls it. The checker refuses an extern in a file with no
/// `rust.module(...)`, so that case never reaches lowering; it still lowers fail-closed, to a body whose only statement
/// refuses, rather than to the placeholder.
fn extern_delegation(
    decorators: &[ast::Spanned<ast::Decorator>],
    name: &str,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> Option<Result<bir::ExternDelegation, String>> {
    let is_extern = decorators
        .iter()
        .any(|decorator| decorators::from_segments(&decorator.node.path.segments) == Some(DecoratorId::RustExtern));
    if !is_extern {
        return None;
    }
    Some(match lowering_facts.rust_module {
        Some(module) => Ok(bir::ExternDelegation {
            rust_module: module.to_string(),
            rust_item: name.to_string(),
        }),
        None => Err(format!(
            "`@rust.extern` declaration `{name}` in a file with no `rust.module(...)` binding"
        )),
    })
}

/// A refusal for a declaration whose binding a user-defined decorator chain replaced (RFC 036), or `None`.
///
/// The checker rebinds the decorated name to the callable the chain returns, initialized once before `main`. Body IR
/// has no representation for that binding: a call through the name still targets this declaration's own body. Lowering
/// the body would run the undecorated function and skip the decorator's effects, so the declaration refuses by name
/// until the rebinding is represented.
fn decorator_rebinding(rebound: bool, name: &str) -> Option<Result<bir::ExternDelegation, String>> {
    rebound.then(|| Err(format!("declaration `{name}` rebound by a user-defined decorator")))
}

/// Lower a declaration's statements, unless it is an `@rust.extern` whose `...` placeholder must not become code.
///
/// An ordinary body returns its trailing expression when it has a value-returning type (#2025). Returns the body's
/// statements and its delegation. An extern's statements are empty (or a single refusal when its delegation could not
/// be resolved), and its parameters are not dropped by the body: the delegated Rust item owns them.
fn lower_declaration_statements(
    builder: &mut BodyBuilder<'_, '_>,
    delegation: Option<Result<bir::ExternDelegation, String>>,
    source: &[ast::Spanned<ast::Statement>],
    root_scope: bir::ScopeId,
    span: HirSourceSpan,
) -> (Vec<bir::Statement>, Option<bir::ExternDelegation>) {
    let mut stmts = Vec::new();
    match delegation {
        Some(Ok(delegation)) => return (stmts, Some(delegation)),
        Some(Err(description)) => builder.push_unsupported_stmt(description, span, &mut stmts),
        None => {
            builder.lower_block_into(source, root_scope, &mut stmts);
            let owner_return_type = builder.owner_return_type.clone();
            return_trailing_value(&owner_return_type, &mut stmts);
            builder.insert_scope_drops(&mut stmts, root_scope);
        }
    }
    (stmts, None)
}

/// Lower a computed static's initializer into a body with no parameters that returns the storage carrier
/// ([`bir::StaticInitializer::Computed`]).
///
/// The body returns the initializer expression as a function body returns a value, checked against the static's
/// carrier, under the static's own identity and declaration-span ids. It is not a callable source declaration, so it
/// never joins the module's bodies; a consumer runs it at most once, where legacy initializes the static.
/// A sole local registry with checked literal method descriptions retains its registrations on this same value before
/// publication. No intermediate source computation can observe that bounded profile, so initialization remains
/// once-only. Scope cleanup may follow the terminal return; registrations precede that return without discarding
/// its trailing drop facts.
pub(super) fn lower_static_initializer_body(
    program: &ast::Program,
    storage: &ast::StaticDecl,
    decl_span: ast::Span,
    canonical: &CanonicalSymbolId,
    ty: &IncanType,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> bir::Body {
    let mut builder = BodyBuilder::new(lowering_facts, ty.clone());
    if registry_effects::entry_type(storage, decl_span, lowering_facts.type_info).as_ref() == Some(ty) {
        builder.type_overrides.insert(storage.value.span, ty.clone());
    }
    if registry_descriptions::initialization_type(storage, lowering_facts.type_info).as_ref() == Some(ty)
        && let ast::Expr::MethodCall(receiver, _, _, _) = &storage.value.node
    {
        builder.type_overrides.insert(storage.value.span, ty.clone());
        builder.type_overrides.insert(receiver.span, ty.clone());
    }
    let root_scope = builder.new_scope(None, hir_span(decl_span));
    let source = [ast::Spanned::new(
        ast::Statement::Return(Some(storage.value.clone())),
        storage.value.span,
    )];
    let (mut stmts, _) = lower_declaration_statements(&mut builder, None, &source, root_scope, hir_span(decl_span));
    let terminal = stmts
        .iter()
        .rposition(|statement| !matches!(statement.kind, bir::StatementKind::Drop { .. }));
    if let Some(index) = terminal
        && let bir::StatementKind::Return { value: Some(value) } = &stmts[index].kind
    {
        let value = value.clone();
        let returned = stmts.remove(index);
        let mut cleanup = stmts.split_off(index);
        registry_descriptions::append_registrations(&mut builder, program, storage, &value, root_scope, &mut stmts);
        stmts.push(returned);
        stmts.append(&mut cleanup);
    }
    finish_static_initializer_body(builder, canonical, ty, root_scope, hir_span(decl_span), stmts)
}

/// Lower a computed constant's initializer as a computed static's: a body with no parameters that returns the value,
/// under the constant's own identity and declaration-span ids. A constant's initializer has no effects and nothing
/// writes it, so running it once where the module initializes its storage observes what every read observes.
pub(super) fn lower_constant_initializer_body(
    item: &ast::ConstDecl,
    decl_span: ast::Span,
    canonical: &CanonicalSymbolId,
    ty: &IncanType,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> bir::Body {
    let mut builder = BodyBuilder::new(lowering_facts, ty.clone());
    builder.type_overrides.insert(item.value.span, ty.clone());
    let root_scope = builder.new_scope(None, hir_span(decl_span));
    let source = [ast::Spanned::new(
        ast::Statement::Return(Some(item.value.clone())),
        item.value.span,
    )];
    let (stmts, _) = lower_declaration_statements(&mut builder, None, &source, root_scope, hir_span(decl_span));
    finish_static_initializer_body(builder, canonical, ty, root_scope, hir_span(decl_span), stmts)
}

/// Finish a compiler-owned static frame, retaining its actual ownership and source/module provenance.
/// Registration effects use the same once-only storage boundary after the original declaration initializers.
pub(super) fn finish_static_initializer_body(
    mut builder: BodyBuilder<'_, '_>,
    canonical: &CanonicalSymbolId,
    ty: &IncanType,
    root_scope: bir::ScopeId,
    span: HirSourceSpan,
    stmts: Vec<bir::Statement>,
) -> bir::Body {
    if builder
        .locals
        .iter()
        .any(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
    {
        builder.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
    }
    let named_type_identities =
        builder.finish_type_identities(parameter_types::retained_type_identities(builder.type_info));
    let direct_call_id = CompilerNodeId::declaration_at(builder.module_identity, span);
    work::built_body(&direct_call_id, "initializer");
    bir::Body {
        type_parameters: Vec::new(),
        trait_default_instantiation: None,
        nominal_instantiation: Vec::new(),
        decl_id: direct_call_id.clone(),
        direct_call_id,
        canonical: Some(canonical.clone()),
        name: canonical.declaration_name.clone(),
        span,
        return_type: ty.clone(),
        callable_representation: Some(bir::CallableRepresentation {
            function_pointer_locals: builder.function_pointer_locals,
            closure_holding_locals: builder.closure_holding_locals,
            closure_holding_parameters: Vec::new(),
            closure_holding_return: false,
        }),
        named_type_identities,
        locals: builder.locals,
        params: Vec::new(),
        param_locals: Vec::new(),
        scopes: builder.scopes,
        block: bir::Block {
            scope: root_scope,
            stmts,
        },
        runtime_requirements: builder.runtime_requirements,
        panic_facts: builder.panic_facts,
        is_async: false,
        extern_delegation: None,
        native_attributes: Vec::new(),
    }
}

/// Lower one function declaration's body into Body IR v0.
pub(super) fn lower_function_body(
    function: &ast::FunctionDecl,
    decl_span: ast::Span,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> bir::Body {
    let direct_call_id = CompilerNodeId::declaration_at(lowering_facts.module_identity, decl_span);
    let decl_id = direct_call_id.clone();
    // The bare-name map is a compatibility projection and collapses top-level overloads. A body is one physical
    // declaration, so its parameter types must come from the same span-keyed fact the direct-call identity uses.
    let binding = lowering_facts
        .type_info
        .declarations
        .function_bindings_by_span
        .get(&decl_span);
    let type_parameters = crate::body_ir_parameter_names::source_parameter_names(&function.type_params);
    let shared_method_chain = lowering_facts
        .type_info
        .declarations
        .method_decorator_receiver_slots
        .get(&decl_span)
        .is_some_and(|slot| !slot.mutable);
    let owner_return_type = function_return_type(binding, &type_parameters, shared_method_chain, lowering_facts);

    let mut builder = BodyBuilder::new(lowering_facts, owner_return_type.clone());
    builder.shared_method_chain = shared_method_chain;
    builder.type_parameters = type_parameters;
    let root_scope = builder.new_scope(None, hir_span(decl_span));

    let param_locals = declare_function_parameters(&mut builder, function, binding, decl_span, root_scope);
    let mut params = Vec::with_capacity(function.params.len());
    lower_callable_parameters(
        &mut builder,
        &function.params,
        &param_locals,
        binding,
        root_scope,
        &mut params,
    );
    retain_parameter_ownership(&mut builder, &params, false);
    let rebound = lowering_facts
        .type_info
        .declarations
        .decorated_function_bindings_by_span
        .contains_key(&decl_span)
        && !decorated_bindings::supports_function(function, decl_span, lowering_facts);
    let (stmts, extern_delegation) = lower_declaration_statements(
        &mut builder,
        decorator_rebinding(rebound, &function.name)
            .or_else(|| extern_delegation(&function.decorators, &function.name, lowering_facts)),
        &function.body,
        root_scope,
        hir_span(decl_span),
    );
    builder.retain_empty_collection_context(&stmts);

    if builder
        .locals
        .iter()
        .any(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
    {
        builder.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
    }

    let named_type_identities =
        builder.finish_type_identities(parameter_types::retained_type_identities(lowering_facts.type_info));
    work::built_body(&direct_call_id, "function");
    bir::Body {
        type_parameters: builder.type_parameters,
        trait_default_instantiation: None,
        nominal_instantiation: Vec::new(),
        decl_id,
        direct_call_id,
        canonical: binding.and_then(|binding| binding.identity.clone()),
        name: function.name.clone(),
        span: hir_span(decl_span),
        return_type: owner_return_type,
        callable_representation: Some(bir::CallableRepresentation {
            function_pointer_locals: builder.function_pointer_locals,
            closure_holding_locals: builder.closure_holding_locals,
            closure_holding_parameters: function
                .params
                .iter()
                .zip(&param_locals)
                .filter(|(param, _)| lowering_facts.type_info.is_closure_holding_param(param.span))
                .map(|(_, local)| *local)
                .collect(),
            closure_holding_return: lowering_facts
                .type_info
                .is_closure_returning_type(function.return_type.span),
        }),
        named_type_identities,
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
        extern_delegation,
        native_attributes: lowering_facts
            .type_info
            .declarations
            .native_attribute_applications
            .get(&decl_span)
            .cloned()
            .unwrap_or_default(),
    }
}
/// Lower one method declaration's body into Body IR v0, or `None` for an abstract method (`body: None` — a trait
/// requirement with no implementation, which has no body to lower).
///
/// Ordinary (non-receiver) method parameters declare with the resolved type the typechecker recorded in
/// [`DeclarationArtifacts::method_bindings_by_span`](
/// crate::typechecker::type_info::DeclarationArtifacts::method_bindings_by_span), keyed by this method's own
/// declaration span (#1121) — mirroring exactly how [`lower_function_body`] consumes `function_bindings` for top-level
/// `def` parameters. This lookup can only miss (falling back to [`IncanType::Unknown`], matching
/// `lower_function_body`'s own fallback) when the typechecker genuinely produced no fact for this declaration, such as
/// a method belonging to a declaration kind excluded from `TypeChecker::check_method_with_self_ty`'s call sites; it is
/// not the normal path for an ordinarily checked method. This does not change the accuracy of ownership facts computed
/// for actual *reads* of those parameters inside the body: those go through [`BodyBuilder::resolve_ty`] at each read's
/// own span, which is populated uniformly for every checked expression regardless of whether it sits in a function or a
/// method body.
///
/// The `self`/`mut self` receiver, when present, is declared as the body's first local (before ordinary
/// parameters) via [`BodyBuilder::declare_receiver_local`], typed with the typechecker-equivalent `receiver_ty`.
/// A method with `receiver: None` (a static/associated method) lowers with no receiver local at all, identically
/// in shape to a free function's body; its ordinary parameters still resolve through the same binding lookup. With
/// no receiver frame to close a generic owner, such a body lists the owner's type parameters ahead of its own, and a
/// call supplies the owner's arguments from its checked receiver type. A `Self` return names the owner, as a `Self`
/// parameter does.
pub(super) fn lower_method_body(
    method: &ast::MethodDecl,
    decl_span: ast::Span,
    owner_name: &str,
    receiver_ty: &IncanType,
    lowering_facts: &BodyIrLoweringFacts<'_, '_>,
) -> Option<bir::Body> {
    let body_stmts = method.body.as_ref()?;
    let checked_receiver = lowering_facts.type_info.qualify_semantic_type(receiver_ty.clone());
    let receiver_ty = &checked_receiver;

    // Method names are not unique across a module the way top-level function names are (two classes can each
    // declare a method named `new`), so the method's CompilerNodeId is scoped under its owning declaration's name
    // rather than reusing `CompilerNodeId::declaration(module_identity, &method.name)` directly.
    let decl_id = CompilerNodeId::declaration(
        lowering_facts.module_identity,
        &format!("{owner_name}::{}", method.name),
    );
    let direct_call_id = CompilerNodeId::declaration_at(lowering_facts.module_identity, decl_span);
    // Computed getters use their dedicated checked property frame in `lower_getter_body`.
    let binding = lowering_facts
        .type_info
        .declarations
        .method_bindings_by_span
        .get(&decl_span);
    let mut type_parameters = owner_parameter_names(receiver_ty, owner_name, lowering_facts.type_info);
    type_parameters.extend(method.type_params.iter().map(|parameter| parameter.name.clone()));
    let owner_return_type = method_return_type(binding, &type_parameters, receiver_ty, lowering_facts);
    let mut builder = BodyBuilder::new(lowering_facts, owner_return_type.clone());
    builder.type_parameters = type_parameters;
    builder.retain_newtype_implementation(owner_name);
    let root_scope = builder.new_scope(None, hir_span(decl_span));

    let (params, param_locals) = lower_method_parameters(
        &mut builder,
        method,
        binding,
        receiver_ty,
        body_stmts,
        decl_span,
        root_scope,
    );
    retain_parameter_ownership(&mut builder, &params, true);
    let rebound = lowering_facts
        .type_info
        .declarations
        .decorated_method_bindings
        .contains_key(&(owner_name.to_string(), method.name.clone()))
        && !decorated_bindings::supports_method(method, owner_name, lowering_facts.type_info);
    let (stmts, extern_delegation) = lower_declaration_statements(
        &mut builder,
        decorator_rebinding(rebound, &method.name)
            .or_else(|| extern_delegation(&method.decorators, &method.name, lowering_facts)),
        body_stmts,
        root_scope,
        hir_span(decl_span),
    );
    builder.retain_empty_collection_context(&stmts);

    if builder
        .locals
        .iter()
        .any(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
    {
        builder.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
    }

    let named_type_identities =
        builder.finish_type_identities(parameter_types::retained_type_identities(lowering_facts.type_info));
    work::built_body(&direct_call_id, "method");
    Some(bir::Body {
        trait_default_instantiation: None,
        nominal_instantiation: Vec::new(),
        // Nominal receiver frames close their owner through layout specialization. Trait defaults instead retain
        // their owner binders until the checked adoption specializes them; publication must know the exact scope.
        type_parameters: if method.receiver.is_none() || *receiver_ty == IncanType::SelfType {
            builder.type_parameters.clone()
        } else {
            method
                .type_params
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect()
        },
        decl_id,
        direct_call_id,
        canonical: binding.and_then(|binding| binding.identity.clone()),
        name: method.name.clone(),
        span: hir_span(decl_span),
        return_type: owner_return_type,
        callable_representation: Some(bir::CallableRepresentation {
            function_pointer_locals: builder.function_pointer_locals,
            closure_holding_locals: builder.closure_holding_locals,
            closure_holding_parameters: method
                .params
                .iter()
                .filter(|param| lowering_facts.type_info.is_closure_holding_param(param.span))
                .filter_map(|param| params.iter().find(|retained| retained.span == hir_span(param.span)))
                .map(|param| param.local)
                .collect(),
            closure_holding_return: lowering_facts
                .type_info
                .is_closure_returning_type(method.return_type.span),
        }),
        named_type_identities,
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
        extern_delegation,
        native_attributes: Vec::new(),
    })
}
/// Resolve the checked return contract in a free function's binder context, applying a shared method decorator's
/// planned carrier only when the checker retained a binding. Missing facts keep the existing unknown fallback.
fn function_return_type(
    binding: Option<&FunctionBindingInfo>,
    type_parameters: &[String],
    shared_method_chain: bool,
    facts: &BodyIrLoweringFacts<'_, '_>,
) -> IncanType {
    binding
        .map(|binding| {
            parameter_types::checked_context_type(
                &binding.return_type,
                facts.type_info,
                type_parameters,
                facts.runtime_type_conflicts,
            )
        })
        .map(|ty| {
            if shared_method_chain {
                decorated_bindings::planned_type(ty, facts.type_info)
            } else {
                ty
            }
        })
        .unwrap_or(IncanType::Unknown)
}

/// Resolve a method's checked return in its owner context. Nominal `Self` returns close like `Self` parameters;
/// trait defaults retain their open receiver and absent checked bindings remain unknown.
fn method_return_type(
    binding: Option<&FunctionBindingInfo>,
    type_parameters: &[String],
    receiver_ty: &IncanType,
    facts: &BodyIrLoweringFacts<'_, '_>,
) -> IncanType {
    let ty = binding
        .map(|binding| {
            parameter_types::checked_context_type(
                &binding.return_type,
                facts.type_info,
                type_parameters,
                facts.runtime_type_conflicts,
            )
        })
        .map(|ty| close_self_type(ty, receiver_ty))
        .unwrap_or(IncanType::Unknown);
    if ty == IncanType::SelfType {
        receiver_ty.clone()
    } else {
        ty
    }
}

/// Declare every free-function parameter before lowering defaults, preserving the shared replacement receiver
/// slot's reference type and the checked representation of rest parameters.
fn declare_function_parameters(
    builder: &mut BodyBuilder<'_, '_>,
    function: &ast::FunctionDecl,
    binding: Option<&FunctionBindingInfo>,
    decl_span: ast::Span,
    scope: bir::ScopeId,
) -> Vec<bir::LocalId> {
    let mut locals = Vec::with_capacity(function.params.len());
    for (index, param) in function.params.iter().enumerate() {
        let mut ty = binding
            .and_then(|b| b.params.get(index))
            .map(|p| builder.checked_type(&p.ty))
            .unwrap_or(IncanType::Unknown);
        let shared_replacement = builder
            .type_info
            .declarations
            .method_decorator_receiver_slots
            .get(&decl_span)
            .is_some_and(|slot| {
                !slot.mutable && slot.role == crate::typechecker::MethodDecoratorReceiverRole::Replacement
            });
        if index == 0 && shared_replacement {
            ty = IncanType::Ref(Box::new(ty));
        }
        if param.node.kind != ast::ParamKind::Normal {
            ty = builder.checked_rest_parameter_type(param);
        }
        let local = builder.declare_new_local(param.node.name.clone(), ty, scope, hir_span(param.span), &function.body);
        builder.locals[local.index()].origin = bir::LocalOrigin::Parameter;
        locals.push(local);
    }
    locals
}

/// Declare the optional receiver first, then every ordinary parameter, before evaluating any defaults. The
/// checked receiver closes ordinary `Self` parameter types; rest parameters retain their own checked carrier.
fn lower_method_parameters(
    builder: &mut BodyBuilder<'_, '_>,
    method: &ast::MethodDecl,
    binding: Option<&FunctionBindingInfo>,
    receiver_ty: &IncanType,
    body: &[ast::Spanned<ast::Statement>],
    decl_span: ast::Span,
    scope: bir::ScopeId,
) -> (Vec<bir::CallableParam>, Vec<bir::LocalId>) {
    let mut params = Vec::with_capacity(method.params.len() + 1);
    let mut param_locals = Vec::with_capacity(method.params.len() + 1);
    if let Some(receiver) = method.receiver {
        let mutable = matches!(receiver, ast::Receiver::Mutable);
        let receiver_span = method
            .receiver_binding
            .as_ref()
            .map_or(decl_span, |binding| binding.span);
        let self_local = builder.declare_receiver_local(receiver_ty.clone(), mutable, scope, hir_span(receiver_span));
        param_locals.push(self_local);
        params.push(bir::CallableParam {
            local: self_local,
            kind: IncanCallableParamKind::Normal,
            name: "self".to_string(),
            ty: receiver_ty.clone(),
            span: hir_span(decl_span),
            default: bir::CallableParamDefault::Required,
            mutable,
            native_owned: false,
        });
    }
    let mut ordinary_param_locals = Vec::with_capacity(method.params.len());
    for (index, param) in method.params.iter().enumerate() {
        let ty = binding
            .and_then(|b| b.params.get(index))
            .map(|p| builder.checked_type(&p.ty))
            .unwrap_or(IncanType::Unknown);
        let ty = if param.node.kind != ast::ParamKind::Normal {
            builder.checked_rest_parameter_type(param)
        } else {
            close_self_type(ty, receiver_ty)
        };
        let local = builder.declare_new_local(param.node.name.clone(), ty, scope, hir_span(param.span), body);
        builder.locals[local.index()].origin = bir::LocalOrigin::Parameter;
        param_locals.push(local);
        ordinary_param_locals.push(local);
    }
    lower_callable_parameters(
        builder,
        &method.params,
        &ordinary_param_locals,
        binding,
        scope,
        &mut params,
    );
    (params, param_locals)
}

/// Attach callable parameter contracts to already declared locals in source order. Default lowering sees the
/// complete parameter frame, while native ownership comes from the same span-keyed checked binding.
fn lower_callable_parameters(
    builder: &mut BodyBuilder<'_, '_>,
    source: &[ast::Spanned<ast::Param>],
    locals: &[bir::LocalId],
    binding: Option<&FunctionBindingInfo>,
    scope: bir::ScopeId,
    out: &mut Vec<bir::CallableParam>,
) {
    for (index, (param, local)) in source.iter().zip(locals.iter().copied()).enumerate() {
        let ty = builder.locals[local.index()].ty.clone();
        out.push(bir::CallableParam {
            local,
            kind: parameter_types::source_parameter_kind(param.node.kind),
            name: param.node.name.clone(),
            ty,
            span: hir_span(param.span),
            default: builder.lower_callable_default(param.node.default.as_ref(), scope),
            mutable: param.node.is_mut,
            native_owned: binding
                .and_then(|binding| binding.params.get(index))
                .is_some_and(|parameter| {
                    parameter_types::native_collection_parameter(&parameter.ty, builder.type_info)
                }),
        });
    }
}

/// Seed parameter ownership from the retained callable contracts. Receiver borrowing is already recorded when a
/// method receiver is declared, so method callers exclude it from this ordinary mutable-parameter pass.
fn retain_parameter_ownership(
    builder: &mut BodyBuilder<'_, '_>,
    params: &[bir::CallableParam],
    exclude_receiver: bool,
) {
    builder.borrowed_parameters.extend(
        params
            .iter()
            .filter(|param| param.mutable && !param.native_owned && (!exclude_receiver || param.name != "self"))
            .map(|param| param.local),
    );
    builder.owned_parameters.extend(
        params
            .iter()
            .filter(|param| param.native_owned || param.kind != IncanCallableParamKind::Normal)
            .map(|param| param.local),
    );
}

/// Reconstruct the concrete `self` type for a method declared on `owner_name`, mirroring how
/// `check_method_with_self_ty` (`loaves/compiler/incan_frontend/src/typechecker/check_decl.rs`) derives its own `self`
/// binding's type: a bare [`IncanType::Named`] for a non-generic owner, or an [`IncanType::Generic`] instantiated with
/// the owner's own type parameters (as type variables) for a generic owner. That typechecker-side resolved type is
/// transient checker state, not persisted anywhere in [`TypeCheckInfo`], so lowering rebuilds the equivalent type
/// directly from the AST rather than depending on a lookup table that does not exist.
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

/// Read `Self` in a declared type's own method signature (`other: Self`, `-> Self`, `-> Option[Self]`) as that type,
/// as the checker reads it at every call site, at any depth.
///
/// Only a trait default keeps `Self` open: its receiver type is `Self` itself, so closing changes nothing there, and
/// trait specialization closes it for each adopter.
pub(super) fn close_self_type(ty: IncanType, receiver_ty: &IncanType) -> IncanType {
    match ty {
        IncanType::SelfType => receiver_ty.clone(),
        IncanType::Generic { base, args } => IncanType::Generic {
            base,
            args: args.into_iter().map(|arg| close_self_type(arg, receiver_ty)).collect(),
        },
        IncanType::Tuple(elements) => IncanType::Tuple(
            elements
                .into_iter()
                .map(|element| close_self_type(element, receiver_ty))
                .collect(),
        ),
        IncanType::Ref(inner) => IncanType::Ref(Box::new(close_self_type(*inner, receiver_ty))),
        IncanType::RefMut(inner) => IncanType::RefMut(Box::new(close_self_type(*inner, receiver_ty))),
        other => other,
    }
}

/// Read declaration binders from the explicit owner frame, preserving the trait registry for a `Self` receiver.
/// The frontend constructs this frame from the accepted owner declaration; these placeholders never come from values.
fn owner_parameter_names(receiver_ty: &IncanType, owner_name: &str, type_info: &TypeCheckInfo) -> Vec<String> {
    if let IncanType::Generic { args, .. } = receiver_ty {
        return args
            .iter()
            .filter_map(|argument| match argument {
                IncanType::TypeVar(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
    }
    type_info
        .traits
        .type_params
        .get(owner_name)
        .cloned()
        .unwrap_or_default()
}

/// Make a value-returning body's trailing expression its `return` (#2025).
///
/// The checker accepts a function whose last statement is an expression (`n + 1`, a `match`, or an `if`/`else` whose
/// branches end in expressions) because that trailing value is the function's result. Lowered statement by statement,
/// the tail would be an [`bir::StatementKind::Expr`], which Body IR defines as discarding its value, so every consumer
/// would have to guess at a trailing-statement convention Body IR otherwise avoids. This states the result explicitly
/// instead. A unit function keeps its expression statement, a body whose return type the checker did not resolve is
/// left alone rather than guessed at, and a generator's trailing expression is not its yield sequence's result.
fn return_trailing_value(return_type: &IncanType, stmts: &mut [bir::Statement]) {
    let returns_value = !matches!(
        return_type,
        IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Unit) | IncanType::Never | IncanType::Unknown
    );
    if returns_value && !yields(stmts) {
        return_trailing_value_in(stmts);
    }
}

/// Turn the tail of `stmts` into a `return`: an expression statement directly, an `if`/`else` through the tail of
/// each branch.
///
/// A branch block already carries its scope's drops when this runs, so the tail is the last statement that is not a
/// `Drop`. Rewriting it in place gives the branch the shape an explicit `return` there has: the value is the result,
/// and the scope's drops still follow it as that scope's cleanup. An already-returning body keeps its unreachable
/// match result as a discarded placeholder rather than adding a contradictory return carrier.
fn return_trailing_value_in(stmts: &mut [bir::Statement]) {
    if super::representation_facts::statements_return(stmts) {
        return;
    }
    let Some(last) = stmts
        .iter_mut()
        .rev()
        .find(|stmt| !matches!(stmt.kind, bir::StatementKind::Drop { .. }))
    else {
        return;
    };
    match &mut last.kind {
        bir::StatementKind::Expr { value } => {
            last.kind = bir::StatementKind::Return {
                value: Some(value.clone()),
            };
        }
        bir::StatementKind::If {
            then_block,
            else_block: Some(else_block),
            ..
        } => {
            return_trailing_value_in(&mut then_block.stmts);
            return_trailing_value_in(&mut else_block.stmts);
        }
        _ => {}
    }
}

/// Whether `stmts`, or any block nested in them, yields: the body is a generator.
fn yields(stmts: &[bir::Statement]) -> bool {
    stmts.iter().any(|stmt| match &stmt.kind {
        bir::StatementKind::Yield { .. } => true,
        bir::StatementKind::If {
            then_block, else_block, ..
        } => yields(&then_block.stmts) || else_block.as_ref().is_some_and(|block| yields(&block.stmts)),
        bir::StatementKind::Loop { body } | bir::StatementKind::UnsafeRegion { body, .. } => yields(&body.stmts),
        _ => false,
    })
}
