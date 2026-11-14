//! Verified C descriptors projected once into native call facts; no class implementation is manufactured.

use super::args::{DeclaredSlot, fixed_elements, plan_declared_args};
use super::{BodyBuilder, HirSourceSpan, IncanType, ast, bir, hir_span, parameter_types};
use crate::typechecker::CBindingType;
use incan_lang::lang::c_abi;
use incan_lang::lang::types::numerics::NumericTypeId;
use incan_semantics_core::{AbiV0RuntimeRequirement, IncanCallableParamKind, IncanPrimitiveType};

/// Project exact checked scalar carriers, refusing all contracts whose native storage has not been retained.
///
/// C `int` by value, as a parameter, a result or an output slot, is carried as `i32`: it is 32 bits wide on every
/// target an Incan toolchain builds for, and the selected target's Clang accepted the declaration against its
/// header. The checker gives such a value the Incan `int`; the call and the slot read convert, as 0.5.1 did.
pub(super) fn carrier(ty: &CBindingType) -> Option<IncanType> {
    match ty {
        CBindingType::Void => Some(IncanType::Primitive(IncanPrimitiveType::Unit)),
        CBindingType::Scalar(c_abi::ScalarTypeId::CInt) => {
            Some(IncanType::Primitive(IncanPrimitiveType::Numeric(NumericTypeId::I32)))
        }
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

/// Whether a declared type is C `int` by value, which the checker carries as an ordinary Incan `int`.
pub(super) fn is_c_int(ty: &CBindingType) -> bool {
    matches!(ty, CBindingType::Scalar(c_abi::ScalarTypeId::CInt))
}

/// The inclusive Incan `int` range a C integer parameter narrower than `int` holds, with its source spelling.
fn integer_range(ty: &CBindingType) -> Option<(i64, i64, &'static str)> {
    let CBindingType::Scalar(scalar) = ty else {
        return None;
    };
    let (low, high) = match scalar {
        c_abi::ScalarTypeId::I8 => (i64::from(i8::MIN), i64::from(i8::MAX)),
        c_abi::ScalarTypeId::U8 => (0, i64::from(u8::MAX)),
        c_abi::ScalarTypeId::I16 => (i64::from(i16::MIN), i64::from(i16::MAX)),
        c_abi::ScalarTypeId::U16 => (0, i64::from(u16::MAX)),
        c_abi::ScalarTypeId::I32 | c_abi::ScalarTypeId::CInt => (i64::from(i32::MIN), i64::from(i32::MAX)),
        c_abi::ScalarTypeId::U32 => (0, i64::from(u32::MAX)),
        c_abi::ScalarTypeId::U64 | c_abi::ScalarTypeId::U128 | c_abi::ScalarTypeId::Size => (0, i64::MAX),
        _ => return None,
    };
    Some((low, high, c_abi::scalar_type_as_str(*scalar)))
}

impl BodyBuilder<'_, '_> {
    /// Stop before a foreign call whose Incan `int` argument its C parameter cannot hold, as 0.5.1's checked
    /// conversion did; the argument is never narrowed silently.
    #[allow(clippy::too_many_arguments)]
    fn assert_c_integer_range(
        &mut self,
        value: &bir::Operand,
        low: i64,
        high: i64,
        message: &str,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let boolean = IncanType::Primitive(IncanPrimitiveType::Bool);
        let lowest = bir::Operand::Constant(bir::Constant::Int(low));
        let highest = bir::Operand::Constant(bir::Constant::Int(high));
        let above = self.push_assign_temp(
            bir::Rvalue::BinaryOp(bir::BinOp::Ge, value.clone(), lowest),
            boolean.clone(),
            scope,
            span,
            out,
        );
        let below = self.push_assign_temp(
            bir::Rvalue::BinaryOp(bir::BinOp::Le, value.clone(), highest),
            boolean.clone(),
            scope,
            span,
            out,
        );
        let cond = self.push_assign_temp(
            bir::Rvalue::BinaryOp(bir::BinOp::And, above, below),
            boolean,
            scope,
            span,
            out,
        );
        self.panic_facts.push(bir::PanicFact {
            span,
            reason: bir::PanicReason::AssertFailure,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::PanicStrategy);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assert {
                kind: bir::AssertionKind::Condition { cond },
                message: Some(bir::Operand::Constant(bir::Constant::Str(message.to_owned()))),
                may_panic: true,
            },
            span,
        });
    }

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
            // A Clove binding links the archive of the configuration it names, under the library's link name that
            // verification resolved from the unit's description.
            // An Apple framework links by its framework name, as 0.5.1's `c.framework` did.
            let framework = descriptor.link_capability == c_abi::LinkCapabilityId::Framework;
            if descriptor.link_capability != c_abi::LinkCapabilityId::SystemLibrary && !framework {
                return Err(format!(
                    "unsupported checked C native link capability {}",
                    c_abi::link_capability_as_str(descriptor.link_capability)
                ));
            }
            let (library, clove, configuration) = if descriptor.configuration.is_empty() {
                (descriptor.system_library.clone(), String::new(), String::new())
            } else {
                (
                    self.type_info
                        .c_abi
                        .clove_libraries
                        .get(&selected.binding)
                        .cloned()
                        .ok_or("checked C Clove binding has no verified library")?,
                    descriptor.system_library.clone(),
                    descriptor.configuration.clone(),
                )
            };
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
            let widened = is_c_int(&symbol.return_type);
            let ranges = symbol
                .parameters
                .iter()
                .map(|parameter| {
                    integer_range(&parameter.ty).map(|(low, high, spelling)| {
                        let message = format!(
                            "checked C argument {} for {}.{} is outside the declared {spelling} range",
                            parameter.name, selected.binding, symbol.name
                        );
                        (low, high, message)
                    })
                })
                .collect::<Vec<_>>();
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
                    library,
                    clove,
                    configuration,
                    framework,
                    header: descriptor.header.clone(),
                    parameters,
                    return_type,
                },
                slots,
                widened,
                ranges,
            ))
        })();
        let (target, slots, widened, ranges) = match result {
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
        let int = IncanType::Primitive(IncanPrimitiveType::Int);
        for (value, range) in values.iter().zip(&ranges) {
            let Some((low, high, message)) = range else {
                continue;
            };
            if self.retained_operand_type(value).as_ref() == Some(&int) {
                self.assert_c_integer_range(value, *low, *high, message, scope, hir_span(span), out);
            }
        }
        let output = target.return_type.clone();
        let result = self.push_call_temp(
            bir::Callee::CheckedForeign(Box::new(target)),
            fixed_elements(values),
            output,
            scope,
            hir_span(span),
            false,
            out,
        );
        if !widened {
            return Some(result);
        }
        // A C `int` result becomes the Incan `int` the checker gave the call.
        Some(self.push_assign_temp(
            bir::Rvalue::Use(result),
            checked_result_type,
            scope,
            hir_span(span),
            out,
        ))
    }
}
