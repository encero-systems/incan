//! Provider ownership of the union a `Some(member)` value is instantiated at.
//!
//! `Some(member)` checked against `Option[A | B]` records the constructor instantiated at the union as its own call's
//! parameter (#1724), so the member is injected into the union at the constructor's argument. The checker's union is
//! structural: when the destination belongs to a `pub::` dependency, the union it names is the one the dependency
//! generated, and a consumer-local copy of it is a different type (#1743). The owner is known at the destination: a
//! call argument's parameter, an element of a collection argument, a field of a dependency model, the enclosing
//! function's return type or an annotated binding, whose lowered type marks the union as provider-owned. This module
//! hands that owned union to the constructor, where the member is injected.
//!
//! `Ok(member)` and `Err(member)` need no fact of their own: the emitter seeds them from the destination's `Result`
//! type, which already carries the provider-owned union.

use super::super::super::expr::{IrCallArg, IrCallArgKind, IrDictEntry, IrExprKind, IrListEntry};
use super::super::super::types::IrType;
use super::super::super::{FunctionSignature, TypedExpr};
use super::super::AstLowering;
use incan_frontend::ast::ParamKind;
use incan_lang::lang::surface::constructors::{self, ConstructorId};

impl AstLowering {
    /// Give each `Some(member)` a call argument holds the provider-owned union its destination parameter declares.
    ///
    /// Arguments bind to parameters as the call binds them: positional arguments in order up to the first rest
    /// parameter, named arguments by name among the ordinary parameters. An argument that binds to a rest parameter,
    /// and every argument after an unpacked one, has no single declared destination and is left alone.
    pub(in crate::lower) fn retain_argument_union_owners(
        args: &mut [IrCallArg],
        signature: Option<&FunctionSignature>,
    ) {
        let Some(signature) = signature else {
            return;
        };
        let mut next_positional = 0usize;
        for arg in args.iter_mut() {
            let param = match (&arg.kind, arg.name.as_deref()) {
                (IrCallArgKind::Named, Some(name)) => signature
                    .params
                    .iter()
                    .find(|param| param.kind == ParamKind::Normal && param.name == name),
                (IrCallArgKind::Positional, _) => {
                    let param = signature
                        .params
                        .get(next_positional)
                        .filter(|param| param.kind == ParamKind::Normal);
                    next_positional += 1;
                    param
                }
                _ => return,
            };
            if let Some(param) = param {
                Self::retain_union_owners_at(&mut arg.expr, &param.ty);
            }
        }
    }

    /// Give each `Some(member)` in a value the provider-owned union its destination type declares.
    ///
    /// The value itself, the elements of a list, set or tuple literal, and the keys and values of a dict literal are
    /// each matched against the corresponding position of the destination; anything else is left alone.
    pub(in crate::lower) fn retain_union_owners_at(expr: &mut TypedExpr, destination: &IrType) {
        if let IrType::Option(owned) = destination
            && matches!(expr.kind, IrExprKind::Call { .. })
        {
            Self::retain_some_payload_union_owner(expr, owned);
            return;
        }
        match (&mut expr.kind, destination) {
            (IrExprKind::List(entries), IrType::List(element)) => {
                for entry in entries {
                    if let IrListEntry::Element(value) = entry {
                        Self::retain_union_owners_at(value, element);
                    }
                }
            }
            (IrExprKind::Set(items), IrType::Set(element)) => {
                for item in items {
                    Self::retain_union_owners_at(item, element);
                }
            }
            (IrExprKind::Tuple(items), IrType::Tuple(elements)) if items.len() == elements.len() => {
                for (item, element) in items.iter_mut().zip(elements) {
                    Self::retain_union_owners_at(item, element);
                }
            }
            (IrExprKind::Dict(entries), IrType::Dict(key_ty, value_ty)) => {
                for entry in entries {
                    if let IrDictEntry::Pair(key, value) = entry {
                        Self::retain_union_owners_at(key, key_ty);
                        Self::retain_union_owners_at(value, value_ty);
                    }
                }
            }
            _ => {}
        }
    }

    /// Give each named field of a dependency model's construction the provider-owned union the field declares.
    pub(in crate::lower) fn retain_constructor_field_union_owners(
        &self,
        struct_ty: &IrType,
        fields: &mut [(String, TypedExpr)],
    ) {
        let Some(library) = self.public_library_for_nominal_receiver_type(struct_ty) else {
            return;
        };
        for (name, value) in fields.iter_mut().filter(|(name, _)| !name.is_empty()) {
            if let Some(declared) = self.declared_field_type_for_imported_pub_type(&library, struct_ty, name) {
                Self::retain_union_owners_at(value, &declared);
            }
        }
    }

    /// Replace the union a `Some(member)` constructor is instantiated at with the destination's provider-owned union.
    ///
    /// The constructor carries a union parameter only when the checker instantiated it at the destination's union
    /// (#1724). The two are the same union exactly when each member of the one the constructor carries is a distinct
    /// member of the provider's union and neither has more; only then is the owner handed over.
    fn retain_some_payload_union_owner(expr: &mut TypedExpr, owned: &IrType) {
        if !matches!(owned, IrType::ExternalUnion { .. }) {
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
        if !is_some
            || matches!(payload.ty, IrType::ExternalUnion { .. })
            || !Self::same_union_members(&payload.ty, owned)
        {
            return;
        }
        payload.ty = owned.clone();
        expr.ty = IrType::Option(Box::new(owned.clone()));
    }

    /// Return whether a consumer-spelled union and a provider-owned union have the same members.
    fn same_union_members(local: &IrType, owned: &IrType) -> bool {
        let (Some(local_members), Some(owned_members)) = (local.union_members(), owned.union_members()) else {
            return false;
        };
        if local_members.len() != owned_members.len() {
            return false;
        }
        let mut matched = vec![false; owned_members.len()];
        local_members.iter().all(|member| {
            owned
                .union_variant_index_for_member(member)
                .and_then(|index| matched.get_mut(index))
                .is_some_and(|slot| !std::mem::replace(slot, true))
        })
    }
}
