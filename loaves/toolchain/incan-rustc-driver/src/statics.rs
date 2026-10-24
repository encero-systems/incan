//! Native storage for computed module statics and the accessors planned code calls.
//!
//! Legacy declares a computed static as `static S: LazyLock<StaticCell<T>> = LazyLock::new(|| StaticCell::new(init))`
//! (RFC 052): the value is computed once, by whichever comes first of the module's declaration-order forcing and a
//! read, and lives behind a read-write lock. The adapter declares the same lazily initialized, lock-guarded storage,
//! `LazyLock<RwLock<T>>`, initialized by the static's planned initializer function, together with load/store/force
//! accessors and guarded method accessors. Load returns a clone of the current value, store replaces it, and force
//! initializes it unless it already is. A guarded accessor holds the actual storage lock throughout the selected
//! planned method call. The storage and accessors keep rustc's own MIR; only initializers and selected methods are
//! planned bodies.

use crate::plan::{CalleeKind, Function, ListLeaf, PlanType, StaticCell, TerminatorKind, tuple_element_type};
use crate::{declarations, error::PlanError, plan::Plan};
use rustc_ast as ast;
use rustc_middle::ty::{self, Ty, TyCtxt};
use rustc_span::{Ident, Span, Symbol};
use std::collections::BTreeSet;
use thin_vec::{ThinVec, thin_vec};

/// Name the guarded accessor for one exact storage slot and selected planned method.
pub(crate) fn method_name(slot: i64, method: &str) -> String {
    format!("__incan_static_method_{slot}_{method}")
}

/// Require the selected method's receiver to borrow exactly this cell's model carrier and its result to outlive the
/// storage guard. The same signature controls adapter declaration and plan validation.
pub(crate) fn method_target<'plan>(plan: &'plan Plan, slot: i64, name: &str) -> Result<&'plan Function, PlanError> {
    let cell = cell(plan, slot)?;
    let method = plan
        .functions
        .iter()
        .find(|method| method.name == name)
        .ok_or_else(|| PlanError::UnknownCallee(name.into()))?;
    let receiver = method.parameters.first().ok_or_else(|| PlanError::Invalid {
        function: name.into(),
        reason: "unsupported static method without receiver".into(),
    })?;
    let carrier = match &receiver.ty {
        PlanType::ModelRef(owner, name) | PlanType::ModelMutRef(owner, name) => PlanType::Model(*owner, name.clone()),
        _ => {
            return Err(PlanError::Invalid {
                function: method.name.clone(),
                reason: "unsupported static method receiver".into(),
            });
        }
    };
    if cell.ty != carrier || !guard_owned_result(plan, &method.return_type, &mut BTreeSet::new(), None) {
        return Err(PlanError::Invalid {
            function: method.name.clone(),
            reason: "unsupported static method carrier or escaping result".into(),
        });
    }
    Ok(method)
}

/// Require owned source layouts and defer opaque native ownership to the exact metadata type. Preflight has no
/// native type context; `verify_method_results` must discharge those provisional native results before MIR is built.
/// Borrowed source fields and recursive source layouts remain refused.
fn guard_owned_result(
    plan: &Plan,
    ty: &PlanType,
    visiting: &mut BTreeSet<(bool, i64)>,
    tcx: Option<TyCtxt<'_>>,
) -> bool {
    if incan_mir_lowering::numeric_carrier(ty.clone()) {
        return true;
    }
    match ty {
        PlanType::Bool
        | PlanType::Unit
        | PlanType::String
        | PlanType::FunctionPointer(_)
        | PlanType::Decimal
        | PlanType::FrozenStr
        | PlanType::FrozenBytes
        | PlanType::StaticText
        | PlanType::StaticBytes => true,
        PlanType::TypeToken(elements) => matches!(
            elements.as_slice(),
            [PlanType::Int | PlanType::Float | PlanType::Bool | PlanType::String]
        ),
        // A frozen collection is a view of `'static` storage: it borrows nothing from the guarded cell.
        PlanType::FrozenCollection(..) => true,
        PlanType::List(leaf, depth) => *depth > 0 && guard_owned_leaf(plan, leaf, visiting, tcx),
        PlanType::Set(leaf) => guard_owned_leaf(plan, leaf, visiting, tcx),
        PlanType::Dict(key, value) => {
            guard_owned_leaf(plan, key, visiting, tcx) && guard_owned_leaf(plan, value, visiting, tcx)
        }
        PlanType::Tuple(elements) => elements
            .iter()
            .all(|element| guard_owned_result(plan, &tuple_element_type(element.clone()), visiting, tcx)),
        PlanType::Model(index, name) => {
            if !visiting.insert((false, *index)) {
                return false;
            }
            let admitted = usize::try_from(*index)
                .ok()
                .and_then(|index| plan.models.get(index))
                .is_some_and(|model| {
                    model.name == *name
                        && if model.library_item.is_empty() {
                            model
                                .fields
                                .iter()
                                .all(|field| guard_owned_result(plan, &field.ty, visiting, tcx))
                        } else {
                            native_result_is_owned(tcx, ty)
                        }
                });
            visiting.remove(&(false, *index));
            admitted
        }
        PlanType::Enum(index, name) => {
            if !visiting.insert((true, *index)) {
                return false;
            }
            let admitted = usize::try_from(*index)
                .ok()
                .and_then(|index| plan.enums.get(index))
                .is_some_and(|owner| {
                    owner.name == *name
                        && if !owner.library_item.is_empty() {
                            native_result_is_owned(tcx, ty)
                        } else {
                            !owner.variants.is_empty()
                                && owner.variants.iter().all(|variant| {
                                    variant
                                        .fields
                                        .iter()
                                        .all(|field| guard_owned_result(plan, field, visiting, tcx))
                                })
                        }
                });
            visiting.remove(&(true, *index));
            admitted
        }
        _ => false,
    }
}

/// Recurse through each owned collection's exact element representation without accepting borrowed model fields.
fn guard_owned_leaf(
    plan: &Plan,
    leaf: &ListLeaf,
    visiting: &mut BTreeSet<(bool, i64)>,
    tcx: Option<TyCtxt<'_>>,
) -> bool {
    match leaf {
        ListLeaf::Reference(_) => false,
        ListLeaf::Model(index, name) => guard_owned_result(plan, &PlanType::Model(*index, name.clone()), visiting, tcx),
        ListLeaf::Enum(index, name) => guard_owned_result(plan, &PlanType::Enum(*index, name.clone()), visiting, tcx),
        ListLeaf::Tuple(elements) => guard_owned_result(plan, &PlanType::Tuple(elements.clone()), visiting, tcx),
        ListLeaf::List(elements, depth) => {
            *depth > 0
                && elements.len() == 1
                && elements
                    .iter()
                    .all(|element| guard_owned_leaf(plan, element, visiting, tcx))
        }
        ListLeaf::Dict(elements) => {
            elements.len() == 2
                && elements
                    .iter()
                    .all(|element| guard_owned_leaf(plan, element, visiting, tcx))
        }
        ListLeaf::Set(elements) => {
            elements.len() == 1
                && elements
                    .iter()
                    .all(|element| guard_owned_leaf(plan, element, visiting, tcx))
        }
        ListLeaf::Int
        | ListLeaf::Float
        | ListLeaf::Bool
        | ListLeaf::Str
        | ListLeaf::U8
        | ListLeaf::USize
        | ListLeaf::Unit
        | ListLeaf::Decimal
        | ListLeaf::FrozenStr
        | ListLeaf::FrozenBytes
        | ListLeaf::FunctionPointer(_) => true,
    }
}

/// Discharge provisional native results against rustc's exact instantiated type. An opaque ADT can hide borrowed
/// storage only through its lifetime or type arguments; no nonstatic region may escape the guard. Function pointers
/// own their pointer independently of the argument and result lifetimes bound by their callable signature.
fn native_result_is_owned(tcx: Option<TyCtxt<'_>>, planned: &PlanType) -> bool {
    let Some(tcx) = tcx else { return true };
    crate::types::native_type(tcx, planned).is_ok_and(native_type_is_owned)
}

/// Inspect native ownership arguments, including metadata defaults and expanded aliases, without guessing hidden
/// fields or accepting erased lifetimes. Unresolved projections and pointer carriers grant no ownership proof.
fn native_type_is_owned(value: Ty<'_>) -> bool {
    match value.kind() {
        ty::Adt(_, arguments) => arguments.iter().all(|argument| match argument.kind() {
            ty::GenericArgKind::Lifetime(region) => region.is_static(),
            ty::GenericArgKind::Type(value) => native_type_is_owned(value),
            ty::GenericArgKind::Const(_) => true,
        }),
        ty::Tuple(elements) => elements.iter().all(native_type_is_owned),
        ty::Array(element, _) | ty::Slice(element) => native_type_is_owned(*element),
        ty::Ref(region, element, _) => region.is_static() && native_type_is_owned(*element),
        ty::Bool | ty::Char | ty::Int(_) | ty::Uint(_) | ty::Float(_) | ty::Str | ty::Never | ty::FnPtr(..) => true,
        _ => false,
    }
}

/// Validate every selected static method result after native aliases resolve and before any planned MIR is built.
/// This is the ownership half of `method_target`; a provisional opaque result is never executable without it.
pub(crate) fn verify_method_results(tcx: TyCtxt<'_>, plan: &Plan) -> Result<(), PlanError> {
    for function in &plan.functions {
        for block in &function.blocks {
            let TerminatorKind::Call(callee, ..) = &block.terminator.kind else {
                continue;
            };
            let CalleeKind::StaticMethod(slot, name) = &callee.kind else {
                continue;
            };
            let method = method_target(plan, *slot, name)?;
            if !guard_owned_result(plan, &method.return_type, &mut BTreeSet::new(), Some(tcx)) {
                return Err(PlanError::Invalid {
                    function: name.clone(),
                    reason: "static method result contains a borrowed or unresolved native carrier".into(),
                });
            }
        }
    }
    Ok(())
}
/// Declare each selected guarded method once. Caller arguments are evaluated before this accessor acquires its guard;
/// mutation, panic poisoning and guard release then follow the legacy StaticCell call shape.
pub(crate) fn method_items(plan: &Plan, cell: &StaticCell, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let names = plan
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .filter_map(|block| match &block.terminator.kind {
            TerminatorKind::Call(callee, ..) => match &callee.kind {
                CalleeKind::StaticMethod(slot, name) if *slot == cell.slot => Some(name.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    names
        .into_iter()
        .map(|name| guarded_method_item(cell.slot, method_target(plan, cell.slot, name)?, span))
        .collect()
}

/// Emit a normal Rust body that keeps the actual read/write guard alive throughout the selected planned method call.
/// Only the selected method's body is replaced with planned MIR.
fn guarded_method_item(slot: i64, method: &Function, span: Span) -> Result<Box<ast::Item>, PlanError> {
    // ---- Parameters: the selected method's ordinary inputs ----
    let receiver = method
        .parameters
        .first()
        .ok_or_else(|| PlanError::UnknownCallee(method.name.clone()))?;
    let mutable = matches!(receiver.ty, PlanType::ModelMutRef(..));
    let names = method
        .parameters
        .iter()
        .skip(1)
        .enumerate()
        .map(|(index, _)| format!("__incan_arg_{index}"))
        .collect::<Vec<_>>();
    let parameters = names
        .iter()
        .zip(method.parameters.iter().skip(1))
        .map(|(name, parameter)| (name.as_str(), declarations::ty(&parameter.ty, span)))
        .collect();

    // ---- Storage: the actual shared or exclusive guard ----
    let lock = expression(
        ast::ExprKind::AddrOf(
            ast::BorrowKind::Ref,
            ast::Mutability::Not,
            deref(path_expr(&[storage_name(slot).as_str()], false, span), span),
        ),
        span,
    );
    let guard = call(
        &["std", "result", "Result", "unwrap_or_else"],
        vec![
            call(
                &["std", "sync", "RwLock", if mutable { "write" } else { "read" }],
                vec![lock],
                span,
            ),
            path_expr(&["std", "sync", "PoisonError", "into_inner"], true, span),
        ],
        span,
    );
    let reference = expression(
        ast::ExprKind::AddrOf(
            ast::BorrowKind::Ref,
            if mutable {
                ast::Mutability::Mut
            } else {
                ast::Mutability::Not
            },
            deref(guard, span),
        ),
        span,
    );

    // ---- Dispatch: keep the guard alive through the complete call ----
    let mut arguments = vec![reference];
    arguments.extend(names.iter().map(|name| path_expr(&[name.as_str()], false, span)));
    Ok(function(
        &method_name(slot, &method.name),
        parameters,
        Some(declarations::ty(&method.return_type, span)),
        vec![tail(call_local(&method.name, arguments, span), span)],
        span,
    ))
}

/// Name the static item that stores slot `slot`.
fn storage_name(slot: i64) -> String {
    format!("__INCAN_STATIC_{slot}")
}

/// Name the function that builds slot `slot`'s lock from its initializer's value.
fn cell_name(slot: i64) -> String {
    format!("__incan_static_cell_{slot}")
}

/// Name the accessor returning a clone of slot `slot`'s current value.
pub(crate) fn load_name(slot: i64) -> String {
    format!("__incan_static_load_{slot}")
}

/// Name the accessor replacing slot `slot`'s value.
pub(crate) fn store_name(slot: i64) -> String {
    format!("__incan_static_store_{slot}")
}

/// Name the accessor initializing slot `slot` unless it already is.
pub(crate) fn force_name(slot: i64) -> String {
    format!("__incan_static_force_{slot}")
}

/// Select the declared cell of slot `slot`.
pub(crate) fn cell(plan: &Plan, slot: i64) -> Result<&StaticCell, PlanError> {
    plan.statics
        .iter()
        .find(|cell| cell.slot == slot)
        .ok_or_else(|| PlanError::UnknownCallee(format!("static slot {slot}")))
}

/// Name the completion probe for the module keyed by its first computed cell.
pub(crate) fn initialized_name(slot: i64) -> String {
    format!("__incan_module_initialized_{slot}")
}

/// Name the publication operation used after declaration-order initialization succeeds.
pub(crate) fn complete_name(slot: i64) -> String {
    format!("__incan_module_complete_{slot}")
}

/// Declare a monotonic completion flag and its acquire/release accessors. The flag never claims initialization;
/// incomplete and reentrant calls still use the runtime's original claim protocol.
fn completion_items(slot: i64, span: Span) -> Vec<Box<ast::Item>> {
    let name = format!("__INCAN_MODULE_COMPLETE_{slot}");
    let atomic_type = Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path(&["std", "sync", "atomic", "AtomicBool"], true, span)),
        span,
        tokens: None,
    });
    let boolean = |value| {
        expression(
            ast::ExprKind::Lit(ast::token::Lit::new(
                ast::token::LitKind::Bool,
                Symbol::intern(if value { "true" } else { "false" }),
                None,
            )),
            span,
        )
    };
    let flag = || {
        expression(
            ast::ExprKind::AddrOf(
                ast::BorrowKind::Ref,
                ast::Mutability::Not,
                path_expr(&[&name], false, span),
            ),
            span,
        )
    };
    let storage = declarations::item(
        ast::ItemKind::Static(Box::new(ast::StaticItem {
            ident: ident(&name, span),
            ty: atomic_type,
            safety: ast::Safety::Default,
            mutability: ast::Mutability::Not,
            expr: Some(call(
                &["std", "sync", "atomic", "AtomicBool", "new"],
                vec![boolean(false)],
                span,
            )),
            define_opaque: None,
            eii_impls: ThinVec::new(),
        })),
        span,
    );
    let load = call(
        &["std", "sync", "atomic", "AtomicBool", "load"],
        vec![
            flag(),
            path_expr(&["std", "sync", "atomic", "Ordering", "Acquire"], true, span),
        ],
        span,
    );
    let probe = function(
        &initialized_name(slot),
        Vec::new(),
        Some(declarations::ty(&PlanType::Bool, span)),
        vec![tail(load, span)],
        span,
    );
    let store = call(
        &["std", "sync", "atomic", "AtomicBool", "store"],
        vec![
            flag(),
            boolean(true),
            path_expr(&["std", "sync", "atomic", "Ordering", "Release"], true, span),
        ],
        span,
    );
    let complete = function(&complete_name(slot), Vec::new(), None, vec![semi(store, span)], span);
    vec![storage, probe, complete]
}

/// Declare one computed static's storage and its accessors.
pub fn items(cell: &StaticCell, span: Span) -> Vec<Box<ast::Item>> {
    let value_ty = || declarations::ty(&cell.ty, span);
    let lock_ty = || std_type(&["sync", "RwLock"], value_ty(), span);
    let storage = || path_expr(&[storage_name(cell.slot).as_str()], false, span);
    // `&*S` is the lock inside the lazily initialized storage; reaching it initializes the storage first.
    let lock = || {
        expression(
            ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, deref(storage(), span)),
            span,
        )
    };
    let recover_lock = |value: Box<ast::Expr>| {
        call(
            &["std", "result", "Result", "unwrap_or_else"],
            vec![
                value,
                path_expr(&["std", "sync", "PoisonError", "into_inner"], true, span),
            ],
            span,
        )
    };

    // ---- static S: LazyLock<RwLock<T>> = LazyLock::new(cell_fn) ----
    let storage_item = declarations::item(
        ast::ItemKind::Static(Box::new(ast::StaticItem {
            ident: ident(&storage_name(cell.slot), span),
            ty: std_type(&["sync", "LazyLock"], lock_ty(), span),
            safety: ast::Safety::Default,
            mutability: ast::Mutability::Not,
            expr: Some(call(
                &["std", "sync", "LazyLock", "new"],
                vec![path_expr(&[cell_name(cell.slot).as_str()], false, span)],
                span,
            )),
            define_opaque: None,
            eii_impls: ThinVec::new(),
        })),
        span,
    );

    // ---- fn cell() -> RwLock<T> { RwLock::new(initializer()) } ----
    let initializer = call_local(&cell.initializer, Vec::new(), span);
    let cell_item = function(
        &cell_name(cell.slot),
        Vec::new(),
        Some(lock_ty()),
        vec![tail(
            call(&["std", "sync", "RwLock", "new"], vec![initializer], span),
            span,
        )],
        span,
    );

    // ---- Read a snapshot, recovering a poisoned lock as legacy's StaticCell does ----
    let guard = recover_lock(call(&["std", "sync", "RwLock", "read"], vec![lock()], span));
    let current = expression(
        ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, deref(guard, span)),
        span,
    );
    let load_item = function(
        &load_name(cell.slot),
        Vec::new(),
        Some(value_ty()),
        vec![tail(
            call(&["std", "clone", "Clone", "clone"], vec![current], span),
            span,
        )],
        span,
    );

    // ---- Replace the current value with the same lock recovery ----
    let guard = recover_lock(call(&["std", "sync", "RwLock", "write"], vec![lock()], span));
    let assignment = expression(
        ast::ExprKind::Assign(deref(guard, span), path_expr(&["value"], false, span), span),
        span,
    );
    let store_item = function(
        &store_name(cell.slot),
        vec![("value", value_ty())],
        None,
        vec![semi(assignment, span)],
        span,
    );

    // ---- fn force() { LazyLock::force(&S); } ----
    let forced = call(
        &["std", "sync", "LazyLock", "force"],
        vec![expression(
            ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, storage()),
            span,
        )],
        span,
    );
    let force_item = function(&force_name(cell.slot), Vec::new(), None, vec![semi(forced, span)], span);

    let mut items = vec![storage_item, cell_item, load_item, store_item, force_item];
    if cell.module_completion {
        items.extend(completion_items(cell.slot, span));
    }
    items
}

/// Create an identifier from a name the plan admitted.
fn ident(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}

/// Build an AST expression with identities left for rustc's node allocation.
fn expression(kind: ast::ExprKind, span: Span) -> Box<ast::Expr> {
    Box::new(ast::Expr {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        attrs: ThinVec::new(),
        tokens: None,
    })
}

/// Dereference `value`.
fn deref(value: Box<ast::Expr>, span: Span) -> Box<ast::Expr> {
    expression(ast::ExprKind::Unary(ast::UnOp::Deref, value), span)
}

/// Spell a path from its segments, from the crate root when `rooted` (`::std::...`), so no declaration named like a
/// standard module can shadow it.
fn path(segments: &[&str], rooted: bool, span: Span) -> ast::Path {
    let mut path_segments = ThinVec::new();
    if rooted {
        path_segments.push(ast::PathSegment::path_root(span));
    }
    for segment in segments {
        path_segments.push(ast::PathSegment::from_ident(ident(segment, span)));
    }
    ast::Path {
        span,
        segments: path_segments,
        tokens: None,
    }
}

/// A path expression.
fn path_expr(segments: &[&str], rooted: bool, span: Span) -> Box<ast::Expr> {
    expression(ast::ExprKind::Path(None, path(segments, rooted, span)), span)
}

/// Call a rooted standard path with `arguments`.
fn call(segments: &[&str], arguments: Vec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Call(path_expr(segments, true, span), arguments.into_iter().collect()),
        span,
    )
}

/// Call a crate-root function by name with `arguments`.
fn call_local(name: &str, arguments: Vec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Call(
            path_expr(&["crate", name], false, span),
            arguments.into_iter().collect(),
        ),
        span,
    )
}

/// The rooted standard type `::std::<module path>::<Name><argument>`.
fn std_type(segments: &[&str], argument: Box<ast::Ty>, span: Span) -> Box<ast::Ty> {
    let mut full = vec!["std"];
    full.extend_from_slice(segments);
    let mut type_path = path(&full, true, span);
    if let Some(last) = type_path.segments.last_mut() {
        last.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: thin_vec![ast::AngleBracketedArg::Arg(ast::GenericArg::Type(argument))],
        })));
    }
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, type_path),
        span,
        tokens: None,
    })
}

/// A statement evaluating `value` as the block's result.
fn tail(value: Box<ast::Expr>, span: Span) -> ast::Stmt {
    ast::Stmt {
        id: ast::DUMMY_NODE_ID,
        kind: ast::StmtKind::Expr(value),
        span,
    }
}

/// A statement evaluating `value` for its effect.
fn semi(value: Box<ast::Expr>, span: Span) -> ast::Stmt {
    ast::Stmt {
        id: ast::DUMMY_NODE_ID,
        kind: ast::StmtKind::Semi(value),
        span,
    }
}

/// A private crate-root function with named parameters, an optional return type and an authored body.
fn function(
    name: &str,
    parameters: Vec<(&str, Box<ast::Ty>)>,
    output: Option<Box<ast::Ty>>,
    statements: Vec<ast::Stmt>,
    span: Span,
) -> Box<ast::Item> {
    let inputs = parameters
        .into_iter()
        .map(|(parameter, ty)| ast::Param {
            attrs: ThinVec::new(),
            ty,
            pat: Box::new(ast::Pat {
                id: ast::DUMMY_NODE_ID,
                kind: ast::PatKind::Ident(ast::BindingMode::NONE, ident(parameter, span), None),
                span,
                tokens: None,
            }),
            id: ast::DUMMY_NODE_ID,
            span,
            is_placeholder: false,
        })
        .collect();
    let output = match output {
        Some(ty) => ast::FnRetTy::Ty(ty),
        None => ast::FnRetTy::Default(span),
    };
    declarations::item(
        ast::ItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(name, span),
            generics: ast::Generics::default(),
            sig: ast::FnSig {
                header: ast::FnHeader::default(),
                decl: Box::new(ast::FnDecl { inputs, output }),
                span,
            },
            contract: None,
            define_opaque: None,
            body: Some(Box::new(ast::Block {
                stmts: statements.into_iter().collect(),
                id: ast::DUMMY_NODE_ID,
                rules: ast::BlockCheckMode::Default,
                span,
                tokens: None,
            })),
            eii_impls: ThinVec::new(),
        })),
        span,
    )
}
