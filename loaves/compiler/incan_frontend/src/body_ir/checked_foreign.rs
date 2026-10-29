//! Verified C descriptors projected once into native call facts; no class implementation is manufactured.

use super::args::{DeclaredSlot, fixed_elements, plan_declared_args};
use super::{BodyBuilder, IncanType, ast, bir, hir_span, parameter_types};
use crate::typechecker::CBindingType;
use incan_lang::lang::c_abi;
use incan_semantics_core::{IncanCallableParamKind, IncanPrimitiveType};

/// Project exact checked scalar carriers, refusing all contracts whose native storage has not been retained.
pub(super) fn carrier(ty: &CBindingType) -> Option<IncanType> {
    match ty {
        CBindingType::Void => Some(IncanType::Primitive(IncanPrimitiveType::Unit)),
        CBindingType::Scalar(scalar) => c_abi::scalar_numeric_type(*scalar)
            .map(|numeric| IncanType::Primitive(IncanPrimitiveType::Numeric(numeric))),
        CBindingType::Pointer { mutable, pointee } => {
            // Only the C string view and the `bounds`-paired span elements have a checked pointer carrier.
            let pointee = match pointee.as_ref() {
                CBindingType::Scalar(c_abi::ScalarTypeId::CChar) => IncanType::NativeCChar,
                CBindingType::Scalar(c_abi::ScalarTypeId::U8 | c_abi::ScalarTypeId::F32) => carrier(pointee)?,
                _ => return None,
            };
            Some(IncanType::NativePointer {
                mutable: *mutable,
                pointee: Box::new(pointee),
            })
        }
        CBindingType::Output { value, .. } => {
            let CBindingType::Scalar(_) = value.as_ref() else {
                return None;
            };
            Some(IncanType::NativePointer {
                mutable: true,
                pointee: Box::new(carrier(value)?),
            })
        }
        _ => None,
    }
}

impl BodyBuilder<'_, '_> {
    /// Preserve the target-verified descriptor before evaluating arguments in their source order.
    /// Missing verification or an unrepresented carrier leaves an explicit refusal at the original call span.
    pub(super) fn lower_checked_foreign_call(
        &mut self,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let selected = self.type_info.c_abi.raw_calls.iter().find(|call| call.span == span)?;
        let checked_result_type = self.resolve_ty(span);
        let result = (|| {
            if !self.type_info.c_abi.verified_bindings.contains(&selected.binding) {
                return Err("checked C descriptor has no successful target verification".to_string());
            }
            let descriptor = self
                .type_info
                .c_abi
                .bindings
                .get(&selected.binding)
                .ok_or("checked C call lost its binding descriptor")?;
            if descriptor.link_capability != c_abi::LinkCapabilityId::SystemLibrary {
                return Err(format!(
                    "unsupported checked C native link capability {}",
                    c_abi::link_capability_as_str(descriptor.link_capability)
                ));
            }
            let symbol = descriptor
                .symbols
                .iter()
                .find(|symbol| symbol.name == selected.symbol)
                .ok_or("checked C call lost its selected symbol")?;
            let binding = parameter_types::retained_nominal_identity(&selected.binding, self.type_info)
                .cloned()
                .ok_or("checked C call lost its canonical binding identity")?;
            let parameters = symbol
                .parameters
                .iter()
                .map(|parameter| {
                    carrier(&parameter.ty)
                        .ok_or_else(|| format!("unsupported checked C parameter carrier {}", parameter.name))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let return_type = carrier(&symbol.return_type).ok_or("unsupported checked C result carrier")?;
            let slots = symbol
                .parameters
                .iter()
                .zip(&parameters)
                .map(|(parameter, ty)| DeclaredSlot {
                    name: Some(parameter.name.clone()),
                    has_default: false,
                    is_partial_preset: false,
                    kind: IncanCallableParamKind::Normal,
                    ty: ty.clone(),
                    is_mut: false,
                })
                .collect::<Vec<_>>();
            Ok((
                bir::CheckedForeignCall {
                    binding,
                    member: symbol.name.clone(),
                    symbol: symbol.native.clone(),
                    library: descriptor.system_library.clone(),
                    header: descriptor.header.clone(),
                    parameters,
                    return_type,
                },
                slots,
            ))
        })();
        let (target, slots) = match result {
            Ok(value) => value,
            Err(reason) => {
                return Some(self.unsupported_typed_operand(reason, checked_result_type, scope, hir_span(span), out));
            }
        };
        let planned = match plan_declared_args("checked C call", &slots, args) {
            Ok(planned) => planned,
            Err(reason) => {
                return Some(self.unsupported_typed_operand(reason, checked_result_type, scope, hir_span(span), out));
            }
        };
        let values = match self.lower_planned_args(&planned, slots.len(), &[], &target.parameters, scope, out) {
            Ok((mut values, _binding)) => {
                for value in &mut values {
                    if let bir::Operand::Place(read) = value {
                        read.fact = bir::OwnershipFact::Copy;
                    }
                }
                values
            }
            Err(reason) => {
                return Some(self.unsupported_typed_operand(reason, checked_result_type, scope, hir_span(span), out));
            }
        };
        let output = target.return_type.clone();
        Some(self.push_call_temp(
            bir::Callee::CheckedForeign(Box::new(target)),
            fixed_elements(values),
            output,
            scope,
            hir_span(span),
            false,
            out,
        ))
    }
}
