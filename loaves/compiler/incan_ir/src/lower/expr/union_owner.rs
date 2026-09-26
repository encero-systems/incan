//! Provider ownership of the union a `Some(member)` argument is instantiated at.
//!
//! `Some(member)` checked against `Option[A | B]` records the constructor instantiated at the union as its own call's
//! parameter (#1724), so the member is injected into the union at the constructor's argument. The checker's union is
//! structural: when the destination parameter belongs to a `pub::` dependency, the union it names is the one the
//! dependency generated, and a consumer-local copy of it is a different type (#1743). The owner is known at the
//! enclosing call, whose lowered signature marks the parameter's union as provider-owned; this module hands that owned
//! union to the constructor, where the member is injected.

use super::super::super::FunctionSignature;
use super::super::super::expr::{IrCallArg, IrCallArgKind, IrExprKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_lang::lang::surface::constructors::{self, ConstructorId};

impl AstLowering {
    /// Give each `Some(member)` argument the provider-owned union its destination parameter declares.
    ///
    /// Arguments bind to parameters as the call binds them: positional arguments in order, named arguments by name.
    /// An argument after an unpacked one has no statically known parameter and is left alone.
    pub(in crate::lower::expr) fn retain_argument_union_owners(
        args: &mut [IrCallArg],
        signature: Option<&FunctionSignature>,
    ) {
        let Some(signature) = signature else {
            return;
        };
        let mut next_positional = 0usize;
        for arg in args.iter_mut() {
            let param = match (&arg.kind, arg.name.as_deref()) {
                (IrCallArgKind::Named, Some(name)) => signature.params.iter().find(|param| param.name == name),
                (IrCallArgKind::Positional, _) => {
                    let param = signature.params.get(next_positional);
                    next_positional += 1;
                    param
                }
                _ => return,
            };
            if let Some(param) = param {
                Self::retain_some_payload_union_owner(&mut arg.expr, &param.ty);
            }
        }
    }

    /// Replace the union a `Some(member)` constructor is instantiated at with the destination's provider-owned union.
    ///
    /// The constructor carries a union parameter only when the checker instantiated it at the destination's union
    /// (#1724), so that parameter and the destination's union are the same union; only its owner is missing.
    fn retain_some_payload_union_owner(expr: &mut super::super::super::TypedExpr, destination: &IrType) {
        let IrType::Option(owned) = destination else {
            return;
        };
        if !matches!(owned.as_ref(), IrType::ExternalUnion { .. }) {
            return;
        }
        let IrExprKind::Call {
            func,
            callable_signature: Some(constructor),
            ..
        } = &mut expr.kind
        else {
            return;
        };
        let is_some = matches!(
            &func.kind,
            IrExprKind::Var { name, .. } if constructors::from_str(name) == Some(ConstructorId::Some)
        );
        let [payload] = constructor.params.as_mut_slice() else {
            return;
        };
        let same_members = matches!(
            (payload.ty.union_members(), owned.union_members()),
            (Some(local), Some(provider)) if local.len() == provider.len()
        );
        if !is_some || matches!(payload.ty, IrType::ExternalUnion { .. }) || !same_members {
            return;
        }
        payload.ty = owned.as_ref().clone();
        expr.ty = destination.clone();
    }
}
