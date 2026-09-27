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

/// #1765: `short = partial label(prefix="name")` forwards to `label` on `self`. The generated body carries the owner's
/// declaration span, so no checker fact types its receiver; lowering types it by the owner, which makes the target an
/// owned-argument Incan method call. The same holds when the partial names the target through a method alias, over a
/// generic model's method, and over a class's `mut self` method.
#[test]
fn method_partial_forwards_on_a_receiver_typed_by_its_model_issue1765() -> Result<(), String> {
    for (label, owner, partial, source) in [
        (
            "direct target",
            "User",
            "short",
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
            "User",
            "short",
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
        (
            "generic model",
            "Wrapper",
            "tagged",
            r#"
model Wrapper[T]:
    value: T

    def tag(self, prefix: str) -> str:
        return prefix

    tagged = partial tag(prefix="wrapped")


pub def use_it(wrapper: Wrapper[int]) -> str:
    return wrapper.tagged()
"#,
        ),
        (
            "mut self target",
            "Counter",
            "bump_one",
            r#"
class Counter:
    pub count: int

    def bump(mut self, by: int) -> None:
        self.count += by

    bump_one = partial bump(by=1)


pub def use_it(mut counter: Counter) -> None:
    counter.bump_one()
"#,
        ),
    ] {
        let ir = lower_checked_source(source)?;
        let impl_block = inherent_impl(&ir, owner)?;
        let (receiver, target) = forwarded_call(impl_block, partial)?;
        assert_eq!(
            receiver.ty.nominal_type_name(),
            Some(owner),
            "{label}: the forwarding receiver is typed by `{owner}`, got {:?}",
            receiver.ty
        );
        assert!(
            impl_block
                .methods
                .iter()
                .any(|method| method.name == target && method.name != partial),
            "{label}: the partial forwards to the lowered target method, got `{target}`"
        );
    }
    Ok(())
}
