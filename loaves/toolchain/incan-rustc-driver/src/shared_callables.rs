//! Native ABI bridges for SDK callbacks whose immutable primitive captures have been proved by Incan lowering.

use crate::ast_build::{
    binding, block, expression, expression_statement, global_path, ident, inherited, local, path_ty, self_value,
};
use crate::error::PlanError;
use crate::plan::{CalleeKind, Function, OperandKind, Plan, PlanType, Projection, TerminatorKind};
use rustc_ast as ast;
use rustc_span::{Ident, Span, kw, respan};
use std::collections::BTreeSet;
use thin_vec::{ThinVec, thin_vec};

/// Name one closed callable signature independently of its constructor and source spellings.
pub fn name(signature: &[PlanType]) -> String {
    let mut digest = 0xcbf29ce484222325_u64;
    for byte in format!("{signature:?}").bytes() {
        digest = (digest ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("__incan_shared_callable_{digest:016x}")
}

/// Select only the bridge constructor contract, which preserves rustc's own transfer and allocation MIR.
pub fn constructor(function: &Function) -> Option<&[PlanType]> {
    function.blocks.iter().find_map(|block| match &block.terminator.kind {
        TerminatorKind::Call(callee, _, _, _, _) => match &callee.kind {
            CalleeKind::SharedCallableBody(signature) => Some(signature.as_slice()),
            _ => None,
        },
        _ => None,
    })
}

/// Reject any bridge plan differing from the single boxed-input transfer its injected native body performs.
pub fn validate(function: &Function) -> Result<(), PlanError> {
    let Some(signature) = constructor(function) else {
        return Ok(());
    };
    let refused = || PlanError::Invalid {
        function: function.name.clone(),
        reason: "shared callable constructor requires one boxed input and one owned result".into(),
    };
    if signature.len() != 2
        || function.return_type != PlanType::SharedCallable(signature.to_vec())
        || function.parameters.len() != 1
        || function.parameters[0].ty != PlanType::Closure(signature.to_vec())
        || function.blocks.len() != 2
        || function.blocks.iter().any(|block| !block.statements.is_empty())
        || !matches!(function.blocks[1].terminator.kind, TerminatorKind::Return)
    {
        return Err(refused());
    }
    let TerminatorKind::Call(_, arguments, destination, successor, _) = &function.blocks[0].terminator.kind else {
        return Err(refused());
    };
    if *successor != 1
        || destination.local != 0
        || destination.projection != Projection::Whole
        || !matches!(arguments.as_slice(), [argument] if matches!(&argument.kind, OperandKind::Move(place) if place.local == 1 && place.projection == Projection::Whole))
    {
        return Err(refused());
    }
    Ok(())
}

/// Inject each signature's carrier and its exact SDK callable and native Clone implementations once.
pub fn items(plan: &Plan, span: Span) -> Vec<Box<ast::Item>> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for function in &plan.functions {
        let Some(signature) = constructor(function) else {
            continue;
        };
        if !seen.insert(name(signature)) {
            continue;
        }
        result.push(carrier(signature, span));
        result.push(clone_impl(signature, span));
        result.push(callable_impl(signature, span));
    }
    result
}

/// Build a concrete `Rc<Box<dyn Fn(A) -> R>>` carrier; the environment remains owned and never executes during
/// wrapping.
fn carrier(signature: &[PlanType], span: Span) -> Box<ast::Item> {
    let mut boxed = global_path(&["std", "boxed", "Box"], span);
    type_arguments(
        &mut boxed,
        thin_vec![crate::declarations::callable_object_ast(signature, span, false)],
        span,
    );
    let mut shared = global_path(&["std", "rc", "Rc"], span);
    type_arguments(&mut shared, thin_vec![path_ty(boxed, span)], span);
    let field = ast::FieldDef {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        mut_restriction: ast::MutRestriction {
            kind: ast::RestrictionKind::Unrestricted,
            span,
            tokens: None,
        },
        safety: ast::Safety::Default,
        ident: None,
        ty: path_ty(shared, span),
        default: None,
        is_placeholder: false,
    };
    crate::declarations::item(
        ast::ItemKind::Struct(
            ident(&name(signature), span),
            ast::Generics::default(),
            ast::VariantData::Tuple(thin_vec![field], ast::DUMMY_NODE_ID),
        ),
        span,
    )
}

/// Set one path's checked type arguments structurally, without parsing source or inferring native carriers.
pub(crate) fn type_arguments(path: &mut ast::Path, types: ThinVec<Box<ast::Ty>>, span: Span) {
    if let Some(segment) = path.segments.last_mut() {
        segment.args = Some(Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
            span,
            args: types
                .into_iter()
                .map(|ty| ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty)))
                .collect(),
        })));
    }
}

/// Read the private shared environment field from an immutable bridge receiver.
fn environment(span: Span) -> Box<ast::Expr> {
    expression(ast::ExprKind::Field(self_value(span), ident("0", span)), span)
}

/// Clone the shared owner only; lowering admits this bridge only when the captured values have no user-defined cloning
/// behavior.
fn clone_impl(signature: &[PlanType], span: Span) -> Box<ast::Item> {
    let clone = expression(
        ast::ExprKind::MethodCall(Box::new(ast::MethodCall {
            seg: ast::PathSegment::from_ident(ident("clone", span)),
            receiver: environment(span),
            args: ThinVec::new(),
            span,
        })),
        span,
    );
    let body = expression(ast::ExprKind::Call(local("Self", span), thin_vec![clone]), span);
    implementation(
        signature,
        global_path(&["std", "clone", "Clone"], span),
        method(
            "clone",
            None,
            path_ty(ast::Path::from_ident(ident("Self", span)), span),
            body,
            span,
        ),
        span,
    )
}

/// Forward the SDK's nominal Callable1 slot to the existing boxed callback; rustc performs Rc and Box
/// autodereferencing.
fn callable_impl(signature: &[PlanType], span: Span) -> Box<ast::Item> {
    let mut path = global_path(&["incan_stdlib_core", "traits", "callable", "Callable1"], span);
    type_arguments(
        &mut path,
        signature.iter().map(|ty| crate::declarations::ty(ty, span)).collect(),
        span,
    );
    let body = expression(
        ast::ExprKind::Call(environment(span), thin_vec![local("argument", span)]),
        span,
    );
    implementation(
        signature,
        path,
        method(
            "__call__",
            Some(crate::declarations::ty(&signature[0], span)),
            crate::declarations::ty(&signature[1], span),
            body,
            span,
        ),
        span,
    )
}

/// Build an immutable trait method with an optional single checked input.
pub(crate) fn method(
    name: &str,
    input: Option<Box<ast::Ty>>,
    output: Box<ast::Ty>,
    body: Box<ast::Expr>,
    span: Span,
) -> Box<ast::AssocItem> {
    let mut inputs = thin_vec![ast::Param::from_self(
        ThinVec::new(),
        respan(span, ast::SelfKind::Region(None, ast::Mutability::Not)),
        Ident::new(kw::SelfLower, span)
    )];
    if let Some(ty) = input {
        inputs.push(ast::Param {
            attrs: ThinVec::new(),
            ty,
            pat: binding("argument", span),
            id: ast::DUMMY_NODE_ID,
            span,
            is_placeholder: false,
        });
    }
    Box::new(ast::AssocItem {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        tokens: None,
        kind: ast::AssocItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(name, span),
            generics: ast::Generics::default(),
            sig: ast::FnSig {
                header: ast::FnHeader::default(),
                decl: Box::new(ast::FnDecl {
                    inputs,
                    output: ast::FnRetTy::Ty(output),
                }),
                span,
            },
            contract: None,
            define_opaque: None,
            body: Some(block(thin_vec![expression_statement(body, span)], span)),
            eii_impls: ThinVec::new(),
        })),
    })
}

/// Implement the exact crate-owned trait for one compiler-private carrier, leaving its bounds to normal rustc checking.
fn implementation(signature: &[PlanType], path: ast::Path, method: Box<ast::AssocItem>, span: Span) -> Box<ast::Item> {
    let mut item = crate::declarations::item(
        ast::ItemKind::Impl(ast::Impl {
            generics: ast::Generics::default(),
            constness: ast::Const::No,
            of_trait: Some(Box::new(ast::TraitImplHeader {
                defaultness: ast::Defaultness::Implicit,
                safety: ast::Safety::Default,
                polarity: ast::ImplPolarity::Positive,
                trait_ref: ast::TraitRef {
                    path,
                    ref_id: ast::DUMMY_NODE_ID,
                },
            })),
            self_ty: path_ty(ast::Path::from_ident(ident(&name(signature), span)), span),
            items: thin_vec![method],
        }),
        span,
    );
    item.vis = inherited(span);
    item
}

/// Wrap the one boxed input in Rc and the signature's carrier, preserving native allocation and cleanup MIR.
pub fn declaration_body(function: &Function, span: Span) -> Option<Box<ast::Block>> {
    let signature = constructor(function)?;
    let shared = expression(
        ast::ExprKind::Call(
            expression(
                ast::ExprKind::Path(None, global_path(&["std", "rc", "Rc", "new"], span)),
                span,
            ),
            thin_vec![local("__incan_callback", span)],
        ),
        span,
    );
    let wrapped = expression(
        ast::ExprKind::Call(local(&name(signature), span), thin_vec![shared]),
        span,
    );
    Some(block(thin_vec![expression_statement(wrapped, span)], span))
}
