//! Source-backed coverage for operation-duration dependencies in the admitted straight-line profile.

use std::collections::BTreeSet;

use incan_lang::lang::builtins::BuiltinFnId;
use incan_semantics_core::{HirSourceSpan, SemanticSourceTargetKind};

use super::{CheckedInputBody, flow};
use crate::ast::{AssertKind, CallArg, Expr, FStringPart, FunctionDecl, ListEntry, Spanned, Statement};

impl CheckedInputBody<'_> {
    /// Require every admitted borrowed operand's operation record, including after restoring persisted checker
    /// facts. Recomputed liveness over a graph with removed consumption reads cannot substitute for this coverage.
    pub(in crate::typechecker::ownership_contracts) fn validate_operation_coverage(
        &self,
        function: &FunctionDecl,
    ) -> Result<(), &'static str> {
        let mut seen = BTreeSet::new();
        for statement in &function.body {
            match &statement.node {
                Statement::Pass => {}
                Statement::Assignment(value) => self.visit_operation_expression(&value.value, &mut seen)?,
                Statement::CompoundAssignment(value) => self.visit_operation_expression(&value.value, &mut seen)?,
                Statement::FieldAssignment(value) => {
                    self.visit_operation_expression(&value.object, &mut seen)?;
                    self.visit_operation_expression(&value.value, &mut seen)?;
                }
                Statement::Expr(value) => self.visit_operation_expression(value, &mut seen)?,
                Statement::Return(value) => {
                    if let Some(value) = value {
                        self.visit_operation_expression(value, &mut seen)?;
                    }
                    break;
                }
                Statement::Assert(value) => {
                    let AssertKind::Condition(condition) = &value.kind else {
                        return Err("operation coverage requires a checked ordinary assertion");
                    };
                    self.visit_operation_expression(condition, &mut seen)?;
                    if let Some(message) = &value.message {
                        self.visit_operation_expression(message, &mut seen)?;
                    }
                }
                _ => return Err("operation coverage requires the admitted straight-line statement profile"),
            }
        }
        if seen.len() != self.graph.operations.len() {
            return Err("operation dependency has no matching source operation");
        }
        Ok(())
    }

    /// Follow the supported expression forms and their operand order. This is an admission validator: extending
    /// source-flow coverage does not automatically grant execution to another expression or evaluation policy.
    fn visit_operation_expression(
        &self,
        expression: &Spanned<Expr>,
        seen: &mut BTreeSet<HirSourceSpan>,
    ) -> Result<(), &'static str> {
        match &expression.node {
            Expr::Ident(_) | Expr::SelfExpr | Expr::Literal(_) => {}
            Expr::Paren(inner) | Expr::Unary(_, inner) | Expr::Field(inner, _) => {
                self.visit_operation_expression(inner, seen)?;
            }
            Expr::Binary(left, _, right) => {
                self.visit_operation_expression(left, seen)?;
                self.visit_operation_expression(right, seen)?;
                self.require_operation(expression.span, &[left.as_ref(), right.as_ref()], seen)?;
            }
            Expr::FString(parts) => {
                let operands = parts
                    .iter()
                    .filter_map(|part| match part {
                        FStringPart::Expr { expr, .. } => Some(expr),
                        FStringPart::Literal(_) => None,
                    })
                    .collect::<Vec<_>>();
                for operand in &operands {
                    self.visit_operation_expression(operand, seen)?;
                }
                self.require_operation(expression.span, &operands, seen)?;
            }
            Expr::Call(callee, _, arguments) => {
                self.visit_operation_expression(callee, seen)?;
                let mut operands = Vec::new();
                for argument in arguments {
                    let (CallArg::Positional(value) | CallArg::Named(_, value)) = argument else {
                        return Err("operation coverage needs checked fixed call arguments");
                    };
                    self.visit_operation_expression(value, seen)?;
                    operands.push(value);
                }
                if matches!(
                    self.facts.resolved_builtin_call(expression.span),
                    Some(BuiltinFnId::Print | BuiltinFnId::Len)
                ) {
                    self.require_operation(expression.span, &operands, seen)?;
                }
            }
            Expr::List(entries) => {
                for entry in entries {
                    let ListEntry::Element(value) = entry else {
                        return Err("operation coverage cannot infer list-spread access");
                    };
                    self.visit_operation_expression(value, seen)?;
                }
            }
            _ => return Err("operation coverage requires an admitted expression"),
        }
        Ok(())
    }

    /// Authenticate exact operand coverage against checked alias identities. A marker at another span, reordered
    /// operands, or an otherwise well-formed read through another loan cannot fulfill this operation's contract.
    fn require_operation(
        &self,
        operation: HirSourceSpan,
        operands: &[&Spanned<Expr>],
        seen: &mut BTreeSet<HirSourceSpan>,
    ) -> Result<(), &'static str> {
        let mut required = Vec::new();
        for operand in operands {
            let ty = self
                .facts
                .expr_type(operand.span)
                .ok_or("operation operand has no checked type")?;
            if !crate::typechecker::semantic_type_from_resolved(ty)
                .abi_v0_facts()
                .ownership
                .is_trivially_copy()
                && let Some((source, loan)) = self.operation_alias_place(operand)?
            {
                required.push((operand.span, source, loan));
            }
        }
        let retained = self.graph.operations.get(&operation);
        if required.is_empty() {
            if retained.is_some() {
                return Err("operation retains a dependency absent from its source operands");
            }
            return Ok(());
        }
        let retained = retained.ok_or("borrowed operation has no retained consumption dependency")?;
        if !seen.insert(operation) || retained.operands.len() != required.len() {
            return Err("operation dependencies do not match source operand coverage");
        }
        for (actual, (operand, source, loan)) in retained.operands.iter().zip(required) {
            if actual.operand != operand || actual.source != source || actual.loan != loan {
                return Err("operation dependency differs from its checked operand source");
            }
        }
        Ok(())
    }

    /// Resolve only stored alias paths using canonical expression identities and the already authenticated local
    /// loan. Fresh expression results retain no upstream alias dependency after their own operation completes.
    fn operation_alias_place(
        &self,
        expression: &Spanned<Expr>,
    ) -> Result<Option<(flow::Place, flow::LoanId)>, &'static str> {
        match &expression.node {
            Expr::Ident(_) => {
                let Some(identity) = self.facts.resolved_identity(expression.span) else {
                    return Ok(None);
                };
                let Some(id) = self.graph.local_loans.get(identity) else {
                    return Ok(None);
                };
                let loan = self.graph.loans.get(id.0).ok_or("operation alias has no source loan")?;
                Ok(Some((loan.source.clone(), *id)))
            }
            Expr::Paren(inner) => self.operation_alias_place(inner),
            Expr::Field(base, _) => {
                let Some((mut place, loan)) = self.operation_alias_place(base)? else {
                    return Ok(None);
                };
                let field = self
                    .facts
                    .resolved_identity(expression.span)
                    .filter(|field| field.kind == SemanticSourceTargetKind::Field)
                    .ok_or("operation alias projection lacks its canonical field")?;
                if self.facts.computed_property_access(expression.span).is_some() {
                    return Err("operation alias cannot infer computed property access");
                }
                place.projection.push(flow::Projection::Field(field.clone()));
                Ok(Some((place, loan)))
            }
            _ => Ok(None),
        }
    }
}
