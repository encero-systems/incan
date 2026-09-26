//! A model's method partial forwards to its target method on a receiver typed by the model (#1765).

use super::*;
use crate::decl::IrImpl;

/// Return the inherent impl block lowered for the named model.
fn inherent_impl<'a>(ir: &'a IrProgram, owner: &str) -> Result<&'a IrImpl, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Impl(impl_block) if impl_block.target_type == owner && impl_block.trait_name.is_none() => {
                Some(impl_block)
            }
            _ => None,
        })
        .ok_or_else(|| format!("missing the inherent impl of `{owner}`"))
}

/// Return the receiver and method name of the call the named generated partial method forwards to.
fn forwarded_call<'a>(impl_block: &'a IrImpl, partial: &str) -> Result<(&'a TypedExpr, &'a str), String> {
    let method = impl_block
        .methods
        .iter()
        .find(|method| method.name == partial)
        .ok_or_else(|| format!("missing the generated method `{partial}`"))?;
    match method.body.as_slice() {
        [
            IrStmt {
                kind: IrStmtKind::Return(Some(forwarded)),
                ..
            },
        ] => match &forwarded.kind {
            IrExprKind::MethodCall { receiver, method, .. } => Ok((receiver.as_ref(), method.as_str())),
            other => Err(format!("`{partial}` must forward through a method call, got {other:?}")),
        },
        other => Err(format!("`{partial}` must be one forwarding return, got {other:?}")),
    }
}

/// #1765: `short = partial label(prefix="name")` forwards to `label` on `self`. The generated body carries the model's
/// declaration span, so no checker fact types its receiver; lowering types it by the owner, which is what makes the
/// target an owned-argument Incan method call that keeps `label` in the impl. The same holds when the partial names
/// the target through a method alias.
#[test]
fn method_partial_forwards_on_a_receiver_typed_by_its_model_issue1765() -> Result<(), String> {
    for (label, source) in [
        (
            "direct target",
            r#"
model User:
    name: str

    def label(self, prefix: str) -> str:
        return prefix

    short = partial label(prefix="name")


pub def use_it(user: User) -> str:
    return user.short()
"#,
        ),
        (
            "target through a method alias",
            r#"
model User:
    name: str

    def label(self, prefix: str) -> str:
        return prefix

    display = label
    short = partial display(prefix="name")


pub def use_it(user: User) -> str:
    return user.short()
"#,
        ),
    ] {
        let ir = lower_checked_source(source)?;
        let impl_block = inherent_impl(&ir, "User")?;
        let (receiver, target) = forwarded_call(impl_block, "short")?;
        assert_eq!(
            receiver.ty,
            IrType::Struct("User".to_string()),
            "{label}: the forwarding receiver is typed by the model"
        );
        assert!(
            impl_block
                .methods
                .iter()
                .any(|method| method.name == target && method.name != "short"),
            "{label}: the partial forwards to the lowered `label` method, got `{target}`"
        );
    }
    Ok(())
}
