//! Canonical RFC 164 contracts, source access checks and staged executable-body admission.
//!
//! A direct input annotation is admitted only with its complete supported body proof. Returned dependencies,
//! deferred boundaries and unsupported storage shapes remain refused rather than acquiring repair copies.

use incan_lang::lang::types::ownership::{self, OwnershipMode};
use incan_semantics_core::SymbolOrigin;

mod admission;
mod checked;
mod copy;
pub(super) mod flow;
pub(super) mod projection;
mod report;
mod source;

pub(super) use admission::{direct_function_boundary, direct_method_boundary};
pub(super) use source::{CallableSource, Unavailable};

use super::TypeChecker;
use crate::ast::{AssignmentStmt, BindingKind, Expr, Span, Spanned, Type};
use crate::diagnostics::CompileError;
use crate::symbols::{ResolvedType, TypeInfo};

/// One body-owned source analysis. Unsupported projection and malformed internal facts remain distinct from a
/// completed analysis with user-program conflicts; none of these variants alone enables executable lowering.
#[derive(Debug, Clone)]
pub(super) enum InputFlow {
    Checked {
        graph: flow::Graph,
        analysis: flow::Analysis,
    },
    Unavailable(source::Unavailable),
    Invalid {
        graph: flow::Graph,
        fault: flow::InvalidGraph,
    },
}

/// Canonical local annotation facts captured before a later binding can shadow a prelude or alias name.
#[derive(Debug, Clone)]
pub(super) struct LocalMode {
    pub owner: incan_semantics_core::CanonicalSymbolId,
    pub mode: OwnershipMode,
    pub value_type: ResolvedType,
    pub writable: bool,
    pub annotation: Span,
}

/// One successfully checked call retains the exact contract owner and instantiated parameter values. A call in
/// declaration/default evaluation has no callable body owner, but still cannot cross a record profile unaware of it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct InputCall {
    pub kind: InputCallKind,
    pub owner: Option<incan_semantics_core::CanonicalSymbolId>,
    pub callee: incan_semantics_core::CanonicalSymbolId,
    pub parameters: Vec<crate::symbols::CallableParam>,
    pub receiver: Option<crate::ast::Receiver>,
}

/// Declaration classification is separate from argument access. Ordinary facts are provisional until their body
/// independently requires an input proof, and cannot replace an explicit contract missing from the declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputCallKind {
    /// The selected declaration carries an explicit input contract.
    Explicit,
    /// A synchronous plain declaration retains its existing inferred ABI.
    Ordinary,
}

impl TypeChecker {
    /// The body writer starts with isolated references. Retain the prepared parameter bindings before statement
    /// checking can shadow their names, so source admission and persisted restoration use the same canonical facts.
    pub(super) fn retain_callable_input_bindings(&mut self, callable: CallableSource<'_>) {
        let parameters = match callable {
            CallableSource::Function(function) => &function.params,
            CallableSource::Method(method) => &method.params,
        };
        for parameter in parameters {
            self.record_write_target_identity(parameter.span, &parameter.node.name);
        }
    }

    /// Capture explicit input flow while the checked lexical frame and body-owned expression facts are active.
    /// Statement output owns the result; it is never rediscovered by lowering or published as a valid empty graph.
    pub(super) fn record_explicit_input_flow(&mut self, callable: CallableSource<'_>, span: Span) {
        let Some(identity) = self.current_body_identity().cloned() else {
            return;
        };
        if !self.type_info.declarations.ownership_contracts.contains_key(&identity)
            && !self
                .type_info
                .protocols
                .explicit_local_modes
                .values()
                .any(|mode| mode.owner == identity)
            && !self
                .type_info
                .protocols
                .explicit_input_calls
                .values()
                .any(|call| call.owner.as_ref() == Some(&identity) && call.kind == InputCallKind::Explicit)
        {
            self.type_info
                .protocols
                .explicit_input_calls
                .retain(|_, call| call.owner.as_ref() != Some(&identity) || call.kind == InputCallKind::Explicit);
            return;
        }
        self.retain_input_copy_values(&identity, span);
        let (graph, view_errors) = source::build_reporting_views(self, callable, span, &identity);
        let view_refused = !view_errors.is_empty();
        self.errors.extend(view_errors);
        let output = match graph {
            // A return that does not borrow the declared view source leaves the body without a proof.
            Ok(_) if view_refused => InputFlow::Unavailable(Unavailable {
                span,
                reason: "returned view does not borrow its declared source".into(),
            }),
            Ok(graph) => match flow::analyze(&graph) {
                Ok(analysis) => {
                    self.errors.extend(report::conflicts(&graph, &analysis));
                    InputFlow::Checked { graph, analysis }
                }
                Err(fault) => InputFlow::Invalid { graph, fault },
            },
            Err(reason) => InputFlow::Unavailable(reason),
        };
        self.type_info
            .protocols
            .explicit_input_flow
            .insert(identity.clone(), output);
        let admission = match callable {
            CallableSource::Function(function) => self.type_info.executable_function_input(&identity, function),
            CallableSource::Method(method) => self.type_info.executable_method_input(&identity, method),
        };
        // A refused returned view already names the offending return; admission would only repeat it.
        if let Err(reason) = admission
            && !view_refused
        {
            self.errors.push(CompileError::type_error(
                format!("Ownership contract cannot be proved: {reason}"), span,
            ).with_stable_code("INCAN-T0123")
                .with_note("The entire owning body must retain supported access, storage and cleanup facts; a valid annotation alone is insufficient"));
        }
    }

    /// Warn when a new local copies a place that a place binding borrows, without changing runtime behavior or
    /// inferring a repair clone.
    ///
    /// An annotation does not opt out. Calls, constructed values, computed properties and explicit clone results are
    /// not storage places; reassignment is handled before this new-binding hook.
    pub(super) fn warn_place_binding_migration(&mut self, assignment: &AssignmentStmt) {
        if assignment.binding == BindingKind::Reassign || !self.migration_place(&assignment.value) {
            return;
        }
        let borrow = if assignment.binding == BindingKind::Mutable {
            "with write-through replacement"
        } else {
            "read-only"
        };
        self.warnings.push(CompileError::warning(
            format!("Local '{}' copies a place that a place binding borrows {borrow}", assignment.name),
            assignment.value.span,
        ).with_stable_code("INCAN-T0125")
            .with_note("This declaration receives an independent value; a type annotation does not request a copy")
            .with_hint("Use an explicit clone for an independent snapshot, or preserve the source through the borrow's last use"));
    }

    /// Admit only existing storage roots and known projections, excluding property calls and temporary owners.
    fn migration_place(&self, value: &Spanned<Expr>) -> bool {
        match &value.node {
            Expr::Ident(name) => self.lookup_variable_info_in_scope_chain(name).is_some(),
            Expr::Paren(inner) => self.migration_place(inner),
            Expr::Field(base, field) if self.migration_place(base) => {
                if self
                    .type_info
                    .resolved_identity(value.span)
                    .is_some_and(|identity| identity.kind == incan_semantics_core::SemanticSourceTargetKind::Field)
                {
                    return true;
                }
                let name = match self.type_info.expr_type(base.span) {
                    Some(ResolvedType::Named(name) | ResolvedType::Generic(name, _)) => name,
                    _ => return false,
                };
                match self.lookup_type_info(name) {
                    Some(TypeInfo::Model(model)) => model.fields.contains_key(field),
                    Some(TypeInfo::Class(class)) => class.fields.contains_key(field),
                    _ => false,
                }
            }
            Expr::Index(base, _) if self.migration_place(base) => matches!(self.type_info.expr_type(base.span),
                Some(ResolvedType::Generic(name, _)) if name == "List" || name == "Dict"),
            _ => false,
        }
    }

    /// Resolve a mode only when ordinary lookup selects its canonical builtin declaration.
    pub(super) fn explicit_ownership_mode(&self, name: &str) -> Option<OwnershipMode> {
        let symbol = self
            .symbols
            .lookup(name)
            .or_else(|| ownership::from_str(name).and_then(|mode| self.symbols.lookup(mode.as_str())))?;
        let identity = self.symbols.identity_of(symbol)?;
        if identity.origin != SymbolOrigin::Builtin {
            return None;
        }
        ownership::from_str(&identity.declaration_name)
    }

    /// Keep prelude access contracts from becoming accidental runtime wrapper constructors.
    pub(super) fn refuse_explicit_ownership_constructor(&mut self, name: &str, span: Span) -> bool {
        let Some(mode) = self.explicit_ownership_mode(name) else {
            return false;
        };
        self.errors.push(
            CompileError::type_error(
                format!(
                    "'{}' is an ownership annotation, not a runtime constructor",
                    mode.as_str()
                ),
                span,
            )
            .with_stable_code("INCAN-T0123")
            .with_hint("Write the contract in a type annotation instead of wrapping the value in a call"),
        );
        true
    }

    /// Refuse explicit mode annotations at every nesting depth, including aliases, fields and callable signatures.
    ///
    /// The annotation retains its generic shape; this diagnostic prevents interpreting that shape as a value wrapper
    /// or turning it into a native reference before the contract's executable semantics exist.
    pub(super) fn refuse_unimplemented_ownership_annotation(&mut self, annotation: &Spanned<Type>) {
        match &annotation.node {
            Type::Simple(name) | Type::Generic(name, _) => {
                if let Some(mode) = self.explicit_ownership_mode(name) {
                    let valid_arity = matches!(&annotation.node, Type::Generic(_, arguments) if arguments.len() == 1);
                    let message = if valid_arity {
                        format!(
                            "Explicit ownership contract '{}[T]' cannot be proved at this boundary",
                            mode.as_str()
                        )
                    } else {
                        format!(
                            "Ownership contract '{}' requires exactly one type argument",
                            mode.as_str()
                        )
                    };
                    self.errors.push(CompileError::type_error(message, annotation.span)
                        .with_stable_code("INCAN-T0123")
                        .with_note("This compiler refuses the contract rather than compiling it with implicit copy or clone semantics"));
                }
                if let Type::Generic(_, arguments) = &annotation.node {
                    for argument in arguments {
                        self.refuse_unimplemented_ownership_annotation(argument);
                    }
                }
            }
            Type::DottedGeneric(_, arguments) | Type::Tuple(arguments) => {
                for argument in arguments {
                    self.refuse_unimplemented_ownership_annotation(argument);
                }
            }
            Type::Function(parameters, result) => {
                for parameter in parameters {
                    self.refuse_unimplemented_ownership_annotation(parameter);
                }
                self.refuse_unimplemented_ownership_annotation(result);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.refuse_unimplemented_ownership_annotation(inner);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
