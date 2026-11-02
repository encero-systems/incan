//! Access projection for fresh model values and primitive text formatting. Neither route calls user hooks implicitly.

use std::collections::BTreeSet;

use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};

use super::{Builder, ResolvedType, Unavailable};
use crate::ast::{CallArg, Expr, FStringFormat, FStringPart, ListEntry, Span, Spanned};

impl Builder<'_> {
    /// A fresh primitive list evaluates each Copy scalar in written order and retains no borrowed dependency.
    /// Spreads, non-Copy elements and reference-carrying aggregates need separate retained-owner projection.
    pub(super) fn scalar_list(
        &mut self,
        entries: &[ListEntry],
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        for entry in entries {
            let value = match entry {
                ListEntry::Element(value) => value,
                ListEntry::Spread(value) => {
                    return Err(Unavailable {
                        span: value.span,
                        reason: "list expansion needs checked element access".into(),
                    });
                }
            };
            if !matches!(
                self.checker.type_info.expr_type(value.span),
                Some(
                    ResolvedType::Int
                        | ResolvedType::Float
                        | ResolvedType::Numeric(_)
                        | ResolvedType::Bool
                        | ResolvedType::Unit
                )
            ) {
                return Err(Unavailable {
                    span: value.span,
                    reason: "list element needs checked retained-owner projection".into(),
                });
            }
            incoming = self.expression(value, incoming, false)?;
        }
        Ok(incoming)
    }

    /// A complete local model or class field binding constructs fresh storage after evaluating supplied values in
    /// written order. Deferred defaults, native constructors and non-Copy place transfer require separate proofs.
    pub(super) fn model_construction(
        &mut self,
        callee: &Spanned<Expr>,
        arguments: &[CallArg],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let owner = self.checker.type_info.resolved_identity(callee.span);
        if !matches!(callee.node, Expr::Ident(_))
            || owner.is_none_or(|owner| {
                !matches!(
                    owner.kind,
                    SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class
                ) || !matches!(owner.origin, SymbolOrigin::Module(_))
            })
        {
            return Err(Unavailable {
                span,
                reason: "construction needs a checked local model or class declaration".into(),
            });
        }
        let binding = self
            .checker
            .type_info
            .constructor_field_binding(span)
            .ok_or(Unavailable {
                span,
                reason: "construction has no checked field binding".into(),
            })?;
        let mut selected = BTreeSet::new();
        if !binding.defaulted_slots.is_empty()
            || binding.field_count != arguments.len()
            || binding.argument_slots.len() != arguments.len()
            || binding
                .argument_slots
                .iter()
                .any(|slot| *slot >= binding.field_count || !selected.insert(*slot))
        {
            return Err(Unavailable {
                span,
                reason: "construction needs complete distinct fields without deferred defaults".into(),
            });
        }
        for argument in arguments {
            let value = match argument {
                CallArg::Positional(value) | CallArg::Named(_, value) => value,
                _ => {
                    return Err(Unavailable {
                        span,
                        reason: "construction expansion lacks checked field access".into(),
                    });
                }
            };
            if !self.fresh_or_copy_field(value) {
                return Err(Unavailable {
                    span: value.span,
                    reason: "non-Copy constructor field needs checked ownership transfer materialization".into(),
                });
            }
            incoming = self.expression(value, incoming, false)?;
        }
        Ok(incoming)
    }

    /// Copy values need no transfer; literal, formatted and nested constructed values own their fresh storage.
    /// A source place with a non-Copy type is never reclassified as fresh merely because its last use is here.
    pub(super) fn fresh_or_copy_field(&self, value: &Spanned<Expr>) -> bool {
        if self
            .checker
            .type_info
            .expr_type(value.span)
            .is_some_and(|ty| self.copy_value(ty))
        {
            return true;
        }
        match &value.node {
            Expr::Literal(_) | Expr::FString(_) => true,
            Expr::Paren(inner) => self.fresh_or_copy_field(inner),
            Expr::Call(_, _, _) => self.checker.type_info.constructor_field_binding(value.span).is_some(),
            _ => false,
        }
    }

    /// Primitive formatting observes each interpolation in source order and creates a new string. Nominal/native
    /// display hooks and unsupported format expressions cannot acquire ordinary read semantics through this route.
    pub(super) fn formatted_text(
        &mut self,
        parts: &[FStringPart],
        span: Span,
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        let mut operands = Vec::new();
        for part in parts {
            let FStringPart::Expr { expr, format } = part else {
                continue;
            };
            if !matches!(format, FStringFormat::Display | FStringFormat::Debug)
                || !matches!(
                    self.checker.type_info.expr_type(expr.span),
                    Some(
                        ResolvedType::Str
                            | ResolvedType::FrozenStr
                            | ResolvedType::Int
                            | ResolvedType::Float
                            | ResolvedType::Bool
                            | ResolvedType::Unit
                    )
                )
            {
                return Err(Unavailable {
                    span: expr.span,
                    reason: "formatting needs selected display-hook access facts".into(),
                });
            }
            incoming = self.expression(expr, incoming, false)?;
            let (evaluated, dependency) = self.retain_operation_operand(expr, incoming)?;
            incoming = evaluated;
            operands.extend(dependency);
        }
        self.finish_operation(span, operands, incoming)
    }
}
