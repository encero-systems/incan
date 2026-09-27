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

use super::super::super::decl::FunctionParam;
use super::super::super::expr::{IrCallArg, IrCallArgKind, IrDictEntry, IrExprKind, IrListEntry};
use super::super::super::types::{IrType, Mutability};
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
    /// The value itself, each arm of a `match` expression and the value of a block, the elements of a list, set or
    /// tuple literal, and the keys and values of a dict literal are each matched against the corresponding position of
    /// the destination; anything else is left alone. A `match` or block that now yields the destination in every arm
    /// is typed as the destination.
    pub(in crate::lower) fn retain_union_owners_at(expr: &mut TypedExpr, destination: &IrType) {
        if let IrType::Option(owned) = destination
            && matches!(expr.kind, IrExprKind::Call { .. })
        {
            Self::retain_some_payload_union_owner(expr, owned);
            return;
        }
        match (&mut expr.kind, destination) {
            (IrExprKind::Match { arms, .. }, _) => {
                for arm in arms.iter_mut() {
                    Self::retain_union_owners_at(&mut arm.body, destination);
                }
                if arms
                    .iter()
                    .all(|arm| arm.body.ty == *destination || matches!(arm.body.kind, IrExprKind::None))
                {
                    expr.ty = destination.clone();
                }
            }
            (IrExprKind::Block { value: Some(value), .. }, _) => {
                Self::retain_union_owners_at(value, destination);
                if value.ty == *destination {
                    expr.ty = destination.clone();
                }
            }
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

    /// Instantiate a `Some(member)` constructor at the destination's provider-owned union.
    ///
    /// The constructor carries a union parameter when the checker instantiated it at the destination's union (#1724).
    /// The two are the same union exactly when each member of the one the constructor carries is a distinct member of
    /// the provider's union and neither has more; only then is the owner handed over. Where the checker recorded no
    /// instantiation, as in an arm of a `match` expression, the constructor is instantiated at the provider's union
    /// when its one payload is a member of that union.
    fn retain_some_payload_union_owner(expr: &mut TypedExpr, owned: &IrType) {
        if !matches!(owned, IrType::ExternalUnion { .. }) {
            return;
        }
        let IrExprKind::Call {
            func,
            args,
            callable_signature,
            ..
        } = &mut expr.kind
        else {
            return;
        };
        let is_some = matches!(
            &func.kind,
            IrExprKind::Var { name, .. } if constructors::from_str(name) == Some(ConstructorId::Some)
        );
        if !is_some {
            return;
        }
        match callable_signature {
            Some(constructor) => {
                let [payload] = constructor.params.as_mut_slice() else {
                    return;
                };
                if matches!(payload.ty, IrType::ExternalUnion { .. }) || !Self::same_union_members(&payload.ty, owned) {
                    return;
                }
                payload.ty = owned.clone();
            }
            None => {
                let [argument] = args.as_slice() else {
                    return;
                };
                if !matches!(argument.kind, IrCallArgKind::Positional)
                    || argument.expr.ty.is_union()
                    || matches!(argument.expr.ty, IrType::Unknown)
                    || owned.union_variant_index_for_member(&argument.expr.ty).is_none()
                {
                    return;
                }
                *callable_signature = Some(FunctionSignature {
                    params: vec![FunctionParam {
                        name: "__incan_arg_0".to_string(),
                        ty: owned.clone(),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: ParamKind::Normal,
                        default: None,
                    }],
                    return_type: IrType::Unknown,
                });
            }
        }
        expr.ty = IrType::Option(Box::new(owned.clone()));
    }

    /// Give a `Some(member)` assigned to a field of a dependency model the provider-owned union the field declares.
    pub(in crate::lower) fn retain_field_assignment_union_owner(
        &self,
        object: &TypedExpr,
        field: &str,
        value: &mut TypedExpr,
    ) {
        let Some(library) = self.public_library_for_nominal_receiver_type(&object.ty) else {
            return;
        };
        if let Some(declared) = self.declared_field_type_for_imported_pub_type(&library, &object.ty, field) {
            Self::retain_union_owners_at(value, &declared);
        }
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
