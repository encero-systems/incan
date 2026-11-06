//! Canonical RFC 164 contracts, source access checks and staged executable-body admission.
//!
//! A direct input annotation is admitted only with its complete supported body proof. Returned dependencies,
//! deferred boundaries and unsupported storage shapes remain refused rather than acquiring repair copies.

use incan_lang::lang::types::ownership::{self, OwnershipMode};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

mod admission;
mod builtins;
mod checked;
mod copy;
pub(super) mod flow;
pub(super) mod native;
pub(super) mod projection;
mod report;
mod source;

pub(super) use admission::{direct_function_boundary, direct_method_boundary};
pub(crate) use builtins::carried_view_binding;
pub(super) use source::{CallableSource, Unavailable};

use super::{TypeCheckInfo, TypeChecker};
use crate::ast::{AssignmentStmt, BindingKind, Expr, Span, Spanned, Statement, Type};
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
    /// A checked callable value uses the parameter surface retained at this call site.
    Indirect,
    /// A native item, whose selected Rust signature gives each slot its access.
    Native,
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

    /// Mark the active callable as containing a checked lazy result. This keeps the ordinary-body activation path
    /// proportional to bodies that can retain a lazy source instead of rescanning every checked callable.
    pub(in crate::typechecker) fn note_lazy_result_body(&mut self) {
        if let Some(identity) = self.current_body_identity().cloned() {
            self.lazy_result_bodies.insert(identity);
        }
    }

    /// Mark the active callable when a selected builtin member returns or transforms a lazy iterator.
    pub(in crate::typechecker) fn note_lazy_result_member(&mut self, identity: &CanonicalSymbolId) {
        if builtins::retained_iterator_member(identity)
            .is_some_and(|method| !incan_lang::lang::surface::iterator_methods::is_terminal(method))
        {
            self.note_lazy_result_body();
        }
    }

    /// Capture explicit input flow while the checked lexical frame and body-owned expression facts are active.
    /// Statement output owns the result; it is never rediscovered by lowering or published as a valid empty graph.
    pub(super) fn record_explicit_input_flow(&mut self, callable: CallableSource<'_>, span: Span) {
        let Some(identity) = self.current_body_identity().cloned() else {
            return;
        };
        // The declaration span covers only its header; the body is the extent of its statements.
        let body = match (callable.body().first(), callable.body().last()) {
            (Some(first), Some(last)) => Some(Span {
                start: first.span.start,
                end: last.span.end,
                ..first.span
            }),
            _ => None,
        };
        let requires_explicit_flow = self.type_info.declarations.ownership_contracts.contains_key(&identity)
            || self
                .type_info
                .protocols
                .explicit_local_modes
                .values()
                .any(|mode| mode.owner == identity)
            || self
                .type_info
                .protocols
                .explicit_input_calls
                .values()
                .any(|call| call.owner.as_ref() == Some(&identity) && call.kind == InputCallKind::Explicit);
        let may_retain_lazy_result = self.lazy_result_bodies.remove(&identity);
        // Existing explicit bodies already need complete flow. Ordinary bodies are scanned only when expression
        // checking observed a generator or authenticated lazy iterator member in that body.
        if !requires_explicit_flow
            && (!may_retain_lazy_result || !body_retains_lazy_result(callable.body(), &self.type_info))
        {
            self.type_info
                .protocols
                .explicit_input_calls
                .retain(|_, call| call.owner.as_ref() != Some(&identity) || call.kind == InputCallKind::Explicit);
            return;
        }
        self.retain_input_copy_values(&identity, span);
        if body.is_some() {
            native::retain_native_input_calls(&mut self.type_info.protocols, &self.type_info.rust, &identity, body);
        }
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
                    self.errors
                        .extend(report::conflicts(&graph, &analysis, &self.type_info));
                    InputFlow::Checked { graph, analysis }
                }
                Err(fault) => InputFlow::Invalid { graph, fault },
            },
            Err(reason) => InputFlow::Unavailable(reason),
        };
        let has_source_conflicts =
            matches!(&output, InputFlow::Checked { analysis, .. } if !analysis.conflicts.is_empty());
        self.type_info
            .protocols
            .explicit_input_flow
            .insert(identity.clone(), output);
        // A validated conflict already produced its operation-owned diagnostic. The executable-body gate must still
        // reject that graph if lowering asks for it, but repeating the rejection here exposes an internal proof
        // consequence as a second source error.
        if has_source_conflicts {
            return;
        }
        let admission = match callable {
            CallableSource::Function(function) => self.type_info.executable_function_input(&identity, function),
            CallableSource::Method(method) => self.type_info.executable_method_input(&identity, method),
        };
        // A refused returned view already names the offending return; admission would only repeat it.
        if let Err(reason) = admission
            && !view_refused
        {
            let mut error = CompileError::type_error(
                format!("Ownership contract cannot be proved: {reason}"), span,
            ).with_stable_code("INCAN-T0123")
                .with_note("The entire owning body must retain supported access, storage and cleanup facts; a valid annotation alone is insufficient");
            // Name the construct source analysis stopped at, or the graph fault it found, so the refusal points at
            // what to change.
            match self.type_info.protocols.explicit_input_flow.get(&identity) {
                Some(InputFlow::Unavailable(unavailable)) => {
                    error = error.with_note(format!("source analysis stops: {}", unavailable.reason));
                    if unavailable.span != span {
                        error = error.with_related_span(unavailable.span, "source analysis stops here");
                    }
                }
                Some(InputFlow::Invalid { fault, .. }) => {
                    error = error.with_note(format!("source analysis produced an invalid graph: {}", fault.0));
                }
                _ => {}
            }
            self.errors.push(error);
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

/// Return whether a body stores, returns or loops over a compiler-authenticated lazy result. Terminal iterator
/// expressions produce no retained dependency and must not widen ordinary bodies into the explicit ownership profile.
fn body_retains_lazy_result(body: &[Spanned<Statement>], facts: &TypeCheckInfo) -> bool {
    for (index, statement) in body.iter().enumerate() {
        let tail = &body[index + 1..];
        let retains = match &statement.node {
            Statement::Assignment(assignment) => {
                value_retains_lazy_result(&assignment.value, facts, false)
                    && lazy_value_conflicts_later(&assignment.value, tail, facts)
            }
            Statement::Return(Some(value)) => value_retains_lazy_result(value, facts, true),
            Statement::For(loop_) => {
                direct_loop_retains_lazy_source(&loop_.iter, &loop_.body, facts)
                    || body_retains_lazy_result(&loop_.body, facts)
            }
            Statement::If(branch) => {
                body_retains_lazy_result(&branch.then_body, facts)
                    || branch
                        .elif_branches
                        .iter()
                        .any(|(_, body)| body_retains_lazy_result(body, facts))
                    || branch
                        .else_body
                        .as_deref()
                        .is_some_and(|body| body_retains_lazy_result(body, facts))
            }
            Statement::While(loop_) => body_retains_lazy_result(&loop_.body, facts),
            Statement::Loop(loop_) => body_retains_lazy_result(&loop_.body, facts),
            Statement::Unsafe(block) => body_retains_lazy_result(&block.body, facts),
            Statement::VocabBlock(block) => body_retains_lazy_result(&block.body, facts),
            Statement::Expr(expression) => expression_block_retains_lazy_result(expression, facts),
            _ => false,
        };
        if retains {
            return true;
        }
    }
    false
}

/// Inspect expression-owned statement blocks without treating a terminal expression value as stored by itself.
fn expression_block_retains_lazy_result(expression: &Spanned<Expr>, facts: &TypeCheckInfo) -> bool {
    match &expression.node {
        Expr::Match(_, arms) => arms.iter().any(|arm| match &arm.node.body {
            crate::ast::MatchBody::Block(body) => body_retains_lazy_result(body, facts),
            crate::ast::MatchBody::Expr(_) => false,
        }),
        Expr::If(branch) => {
            body_retains_lazy_result(&branch.then_body, facts)
                || branch
                    .else_body
                    .as_deref()
                    .is_some_and(|body| body_retains_lazy_result(body, facts))
        }
        Expr::Loop(loop_) => body_retains_lazy_result(&loop_.body, facts),
        _ => false,
    }
}

/// Return whether a stored lazy value has a source that a later statement may mutate, replace, transfer or escape.
fn lazy_value_conflicts_later(value: &Spanned<Expr>, tail: &[Spanned<Statement>], facts: &TypeCheckInfo) -> bool {
    let mut sources = Vec::new();
    lazy_result_source_bindings(value, facts, &mut sources);
    sources
        .iter()
        .any(|source| body_conflicts_with_binding(tail, source, facts))
}

/// Activate direct-loop flow only when the body reaches the same stored source. A loop that merely consumes its
/// iterator and writes unrelated output retains no dependency beyond the loop statement and needs no body profile.
fn direct_loop_retains_lazy_source(iter: &Spanned<Expr>, body: &[Spanned<Statement>], facts: &TypeCheckInfo) -> bool {
    if !value_retains_lazy_result(iter, facts, false) {
        return false;
    }
    let mut sources = Vec::new();
    lazy_result_source_bindings(iter, facts, &mut sources);
    sources
        .iter()
        .any(|source| body_conflicts_with_binding(body, source, facts))
}

/// Collect checked storage bindings retained by a lazy expression, following authenticated adapters and carriers.
fn lazy_result_source_bindings(value: &Spanned<Expr>, facts: &TypeCheckInfo, bindings: &mut Vec<CanonicalSymbolId>) {
    match &value.node {
        Expr::Paren(inner) => lazy_result_source_bindings(inner, facts, bindings),
        Expr::Generator(generator) => generator.clauses.iter().for_each(|clause| {
            if let crate::ast::ComprehensionClause::For { iter, .. } = clause {
                lazy_result_source_bindings(iter, facts, bindings);
                push_place_binding(iter, facts, bindings);
            }
        }),
        Expr::MethodCall(receiver, _, _, _) => {
            if builtins::retained_iterator_method(facts, value.span).is_some() {
                lazy_result_source_bindings(receiver, facts, bindings);
                push_place_binding(receiver, facts, bindings);
            }
        }
        Expr::List(entries) => entries.iter().for_each(|entry| {
            if let crate::ast::ListEntry::Element(value) = entry {
                lazy_result_source_bindings(value, facts, bindings);
            }
        }),
        Expr::Call(..) => {
            if let Some((_variant, arguments)) = builtins::carrier_construction(facts, value) {
                arguments.iter().for_each(|argument| match argument {
                    crate::ast::CallArg::Positional(value) | crate::ast::CallArg::Named(_, value) => {
                        lazy_result_source_bindings(value, facts, bindings);
                    }
                    _ => {}
                });
            }
        }
        _ => push_place_binding(value, facts, bindings),
    }
}

/// Add one canonical place root once, preserving multiple independent sources carried by an adapter or aggregate.
fn push_place_binding(value: &Spanned<Expr>, facts: &TypeCheckInfo, bindings: &mut Vec<CanonicalSymbolId>) {
    if let Some(binding) = place_binding(value, facts)
        && !bindings.contains(&binding)
    {
        bindings.push(binding);
    }
}

/// Return the canonical root binding of a checked place without reconstructing it from source spelling.
fn place_binding(value: &Spanned<Expr>, facts: &TypeCheckInfo) -> Option<CanonicalSymbolId> {
    match &value.node {
        Expr::Ident(_) => facts.resolved_identity(value.span).cloned(),
        Expr::Paren(inner) => place_binding(inner, facts),
        Expr::Field(base, _) | Expr::Index(base, _) => place_binding(base, facts),
        _ => None,
    }
}

/// Recognize actions that can invalidate or outlive a retained source; ordinary reads do not activate full flow.
fn body_conflicts_with_binding(body: &[Spanned<Statement>], source: &CanonicalSymbolId, facts: &TypeCheckInfo) -> bool {
    if body_writes_binding(body, source, facts) {
        return true;
    }
    body.iter().any(|statement| match &statement.node {
        Statement::Assignment(assignment) => whole_binding(&assignment.value, facts).as_ref() == Some(source),
        Statement::Return(Some(value)) => whole_binding(value, facts).as_ref() == Some(source),
        Statement::FieldAssignment(assignment) => place_binding(&assignment.object, facts).as_ref() == Some(source),
        Statement::IndexAssignment(assignment) => place_binding(&assignment.object, facts).as_ref() == Some(source),
        Statement::Expr(expression) => expression_mutates_binding(expression, source, facts),
        Statement::If(branch) => {
            body_conflicts_with_binding(&branch.then_body, source, facts)
                || branch
                    .elif_branches
                    .iter()
                    .any(|(_, body)| body_conflicts_with_binding(body, source, facts))
                || branch
                    .else_body
                    .as_deref()
                    .is_some_and(|body| body_conflicts_with_binding(body, source, facts))
        }
        Statement::For(loop_) => body_conflicts_with_binding(&loop_.body, source, facts),
        Statement::While(loop_) => body_conflicts_with_binding(&loop_.body, source, facts),
        Statement::Loop(loop_) => body_conflicts_with_binding(&loop_.body, source, facts),
        Statement::Unsafe(block) => body_conflicts_with_binding(&block.body, source, facts),
        Statement::VocabBlock(block) => body_conflicts_with_binding(&block.body, source, facts),
        _ => false,
    })
}

/// Return the canonical identity of an unprojected binding value that may transfer its complete storage.
fn whole_binding(value: &Spanned<Expr>, facts: &TypeCheckInfo) -> Option<CanonicalSymbolId> {
    match &value.node {
        Expr::Ident(_) => facts.resolved_identity(value.span).cloned(),
        Expr::Paren(inner) => whole_binding(inner, facts),
        _ => None,
    }
}

/// Recognize a compiler-selected mutating list method on the retained source place.
fn expression_mutates_binding(value: &Spanned<Expr>, source: &CanonicalSymbolId, facts: &TypeCheckInfo) -> bool {
    let Expr::MethodCall(receiver, method, _, _) = &value.node else {
        return false;
    };
    place_binding(receiver, facts).as_ref() == Some(source)
        && builtins::list_method_writes(facts, receiver, method, value.span) == Some(true)
}

/// Find replacement of the retained root in statement-owned write positions that expression traversal does not see.
fn body_writes_binding(body: &[Spanned<Statement>], source: &CanonicalSymbolId, facts: &TypeCheckInfo) -> bool {
    body.iter().any(|statement| match &statement.node {
        Statement::Assignment(assignment) => {
            facts.resolved_write_identity(assignment.name_span, &assignment.name) == Some(source)
        }
        Statement::CompoundAssignment(assignment) => {
            facts.resolved_write_identity(assignment.name_span, &assignment.name) == Some(source)
        }
        Statement::ChainedAssignment(assignment) => assignment
            .targets
            .iter()
            .zip(&assignment.target_spans)
            .any(|(name, span)| facts.resolved_write_identity(*span, name) == Some(source)),
        Statement::TupleUnpack(assignment) => assignment
            .names
            .iter()
            .zip(&assignment.name_spans)
            .any(|(name, span)| facts.resolved_write_identity(*span, name) == Some(source)),
        Statement::If(branch) => {
            body_writes_binding(&branch.then_body, source, facts)
                || branch
                    .elif_branches
                    .iter()
                    .any(|(_, body)| body_writes_binding(body, source, facts))
                || branch
                    .else_body
                    .as_deref()
                    .is_some_and(|body| body_writes_binding(body, source, facts))
        }
        Statement::For(loop_) => body_writes_binding(&loop_.body, source, facts),
        Statement::While(loop_) => body_writes_binding(&loop_.body, source, facts),
        Statement::Loop(loop_) => body_writes_binding(&loop_.body, source, facts),
        Statement::Unsafe(block) => body_writes_binding(&block.body, source, facts),
        Statement::VocabBlock(block) => body_writes_binding(&block.body, source, facts),
        _ => false,
    })
}

/// Return whether one value physically retains a lazy dependency, including nested list aggregates. An escaping
/// iterator over a transferable local or parameter owns that list through the lowering route, while a projected
/// place must retain a borrow because moving the enclosing owner would change the program's value semantics.
fn value_retains_lazy_result(value: &Spanned<Expr>, facts: &TypeCheckInfo, escaping: bool) -> bool {
    match &value.node {
        Expr::Paren(inner) => value_retains_lazy_result(inner, facts, escaping),
        Expr::Generator(generator) => generator.clauses.iter().any(|clause| {
            let crate::ast::ComprehensionClause::For { iter, .. } = clause else {
                return false;
            };
            (lazy_source_is_place(iter)
                && (!escaping || escaping_lazy_source_is_borrowed(iter))
                && matches!(facts.expr_type(iter.span),
                Some(ResolvedType::Generic(name, _))
                    if incan_lang::lang::types::collections::from_str(name)
                        == Some(incan_lang::lang::types::collections::CollectionTypeId::List)))
                || value_retains_lazy_result(iter, facts, escaping)
        }),
        Expr::MethodCall(receiver, _, _, _) => {
            let Some(method) = builtins::retained_iterator_method(facts, value.span) else {
                return false;
            };
            let lazy_result = matches!(facts.expr_type(value.span),
                Some(ResolvedType::Generic(name, _))
                    if incan_lang::lang::traits::from_qualified_str(name)
                        == Some(incan_lang::lang::traits::TraitId::Iterator)
                        || incan_lang::lang::types::collections::from_str(name)
                            == Some(incan_lang::lang::types::collections::CollectionTypeId::Generator));
            lazy_result
                && ((method == incan_lang::lang::surface::iterator_methods::IteratorMethodId::Iter
                    && lazy_source_is_place(receiver)
                    && (!escaping || escaping_lazy_source_is_borrowed(receiver)))
                    || value_retains_lazy_result(receiver, facts, escaping))
        }
        Expr::List(entries) => entries.iter().any(|entry| match entry {
            crate::ast::ListEntry::Element(value) => value_retains_lazy_result(value, facts, escaping),
            crate::ast::ListEntry::Spread(_) => false,
        }),
        Expr::Call(..) => builtins::carrier_construction(facts, value).is_some_and(|(_, arguments)| {
            arguments.iter().any(|argument| match argument {
                crate::ast::CallArg::Positional(value) | crate::ast::CallArg::Named(_, value) => {
                    value_retains_lazy_result(value, facts, escaping)
                }
                _ => false,
            })
        }),
        _ => false,
    }
}

/// Return whether an escaping lazy source is a projection whose enclosing owner must stay in place. Plain local and
/// parameter bindings can transfer their complete list storage into the lazy result at last use.
fn escaping_lazy_source_is_borrowed(value: &Spanned<Expr>) -> bool {
    match &value.node {
        Expr::Paren(inner) => escaping_lazy_source_is_borrowed(inner),
        Expr::Field(..) | Expr::Index(..) => true,
        _ => false,
    }
}

/// Return whether a lazy source is existing storage rather than a fresh call or aggregate result.
fn lazy_source_is_place(value: &Spanned<Expr>) -> bool {
    match &value.node {
        Expr::Ident(_) => true,
        Expr::Paren(inner) => lazy_source_is_place(inner),
        Expr::Field(base, _) | Expr::Index(base, _) => lazy_source_is_place(base),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
