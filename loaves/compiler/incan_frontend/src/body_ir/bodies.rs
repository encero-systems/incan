//! Building one `Body` per lowered function or method declaration, including receiver resolution.

use super::*;
use incan_lang::lang::decorators::{self, DecoratorId};

/// Lower every non-abstract method in `methods` (owned by the declaration named `owner_name`) into one
/// [`bir::Body`] each, skipping abstract methods (`body: None`). `receiver_ty` is the typechecker-equivalent type
/// for a declared receiver: a concrete nominal type for models, classes, newtypes, and enums, or
/// [`IncanType::SelfType`] for trait defaults.
///
/// Exactly five declaration kinds carry a `methods` field -- model, class, trait, newtype, and enum (see
/// `loaves/kernel/incan_syntax/src/ast/decls.rs`) -- and all five reach this function. No kind that carries methods is
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
            mutable: param.node.is_mut,
        });
    }

    builder
        .borrowed_parameters
        .extend(params.iter().filter(|param| param.mutable).map(|param| param.local));
    let rebound = lowering_facts
        .type_info
        .declarations
        .decorated_function_bindings_by_span
        .contains_key(&(decl_span.start, decl_span.end));
    let (stmts, extern_delegation) = lower_declaration_statements(
        &mut builder,
        decorator_rebinding(rebound, &function.name)
            .or_else(|| extern_delegation(&function.decorators, &function.name, lowering_facts)),
        &function.body,
        root_scope,
        hir_span(decl_span),
    );

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
        named_type_identities: lowering_facts.type_info.declarations.named_type_identities.clone(),
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
            mutable,
        });
    }

    let mut ordinary_param_locals = Vec::with_capacity(method.params.len());
    for (index, param) in method.params.iter().enumerate() {
        let ty = binding
            .and_then(|b| b.params.get(index))
            .map(|p| semantic_type_from_resolved(&p.ty))
            .unwrap_or(IncanType::Unknown);
        // In a declared type's own method, a parameter typed `Self` (`other: Self`) is that type, as the checker reads
        // it; only a trait default keeps `Self` open, and its `receiver_ty` is `Self` already.
        let ty = if ty == IncanType::SelfType {
            receiver_ty.clone()
        } else {
            ty
        };
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
            mutable: param.node.is_mut,
        });
    }

    builder.borrowed_parameters.extend(
        params
            .iter()
            .filter(|param| param.mutable && param.name != "self")
            .map(|param| param.local),
    );
    let rebound = lowering_facts
        .type_info
        .declarations
        .decorated_method_bindings
        .contains_key(&(owner_name.to_string(), method.name.clone()));
    let (stmts, extern_delegation) = lower_declaration_statements(
        &mut builder,
        decorator_rebinding(rebound, &method.name)
            .or_else(|| extern_delegation(&method.decorators, &method.name, lowering_facts)),
        body_stmts,
        root_scope,
        hir_span(decl_span),
    );

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
        named_type_identities: lowering_facts.type_info.declarations.named_type_identities.clone(),
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
    })
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
/// and the scope's drops still follow it as that scope's cleanup.
fn return_trailing_value_in(stmts: &mut [bir::Statement]) {
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
        bir::StatementKind::Loop { body } => yields(&body.stmts),
        _ => false,
    })
}
