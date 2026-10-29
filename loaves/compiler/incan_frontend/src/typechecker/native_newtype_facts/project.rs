//! Atomic accepted-body projection and exact owner-signature closure.

use std::collections::{BTreeSet, HashMap};

use crate::ast::{self, Declaration, Program, Span, Spanned};
use crate::symbols::{CallableParam, ResolvedType};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

use super::selection::{MethodFrame, OwnerFrame, TargetDomain, collect_owners, selected_method};
use super::{FunctionBindingInfo, NativeBodyFacts, NativeFactGuard, NativeNewtypeFacts, TypeCheckInfo};

/// A derived occurrence carries its canonical owner and the specific template holes it is permitted to close.
#[derive(Clone)]
struct NativeValue {
    ty: ResolvedType,
    owner: Option<CanonicalSymbolId>,
    parameter: Option<BinderOrigin>,
    holes: BTreeSet<Vec<usize>>,
    native: bool,
}

/// Untouched calls can preserve their checked result; a damaged selected constructor must abort the transaction.
enum ConstructorSelection {
    Ordinary,
    Proved(Box<NativeValue>),
}

/// Same-spelled parameters from distinct owners have distinct native refinement authority.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BinderOrigin {
    pub(super) owner: CanonicalSymbolId,
    pub(super) declaration_span: Span,
    pub(super) name: String,
    pub(super) argument: ResolvedType,
}

/// One eligible body's isolated environment; failure discards every accumulated refinement.
struct BodyFactsBuilder<'source> {
    info: &'source TypeCheckInfo,
    owners: &'source [OwnerFrame<'source>],
    domain: &'source TargetDomain<'source>,
    method: Option<MethodFrame<'source>>,
    root: Option<(Span, &'source FunctionBindingInfo)>,
    bindings: HashMap<CanonicalSymbolId, NativeValue>,
    facts: NativeBodyFacts,
}

/// Derive native facts only from an accepted program checkpoint, without feeding them back into checking.
pub(super) fn project(program: &Program, info: &TypeCheckInfo) -> NativeNewtypeFacts {
    // ---- Context: exact local target domain and admitted owner frames ----
    let owners = collect_owners(program, info);
    let mut result = NativeNewtypeFacts {
        owners: owners.iter().map(|owner| owner.identity.clone()).collect(),
        ..NativeNewtypeFacts::default()
    };
    let Some(domain) = TargetDomain::collect(program, info) else {
        return result;
    };
    if owners.is_empty() {
        return result;
    }

    // ---- Context: parameterless source functions with one atomic native environment ----
    for declaration in &program.declarations {
        if let Declaration::Function(function) = &declaration.node
            && function.type_params.is_empty()
            && function.params.is_empty()
            && let Some(binding) = info.declarations.function_bindings_by_span.get(&declaration.span)
            && !binding.is_async
            && let Some(facts) = body_transaction(
                &function.body,
                &owners,
                &domain,
                info,
                None,
                Some((declaration.span, binding)),
            )
        {
            result.bodies.push(facts);
        }
    }

    // ---- Context: method bodies retain their declaring owner and separate binder frame ----
    for owner in &owners {
        for method in &owner.source.methods {
            let span = method.span;
            let Some(binding) = info.declarations.method_bindings_by_span.get(&span) else {
                continue;
            };
            let Some(identity) = &binding.identity else {
                continue;
            };
            let Some(frame) = selected_method(identity, &owners, info) else {
                continue;
            };
            if frame.owner.identity != owner.identity || frame.source.span != method.span {
                continue;
            }
            let Some(body) = &method.node.body else {
                continue;
            };
            if let Some(facts) = body_transaction(body, &owners, &domain, info, Some(frame), None) {
                result.bodies.push(facts);
            }
        }
    }
    result
}

/// Reject uncertain flow before deriving any facts; a later conflict drops this entire transaction.
fn body_transaction(
    statements: &[Spanned<ast::Statement>],
    owners: &[OwnerFrame<'_>],
    domain: &TargetDomain<'_>,
    info: &TypeCheckInfo,
    method: Option<MethodFrame<'_>>,
    root: Option<(Span, &FunctionBindingInfo)>,
) -> Option<NativeBodyFacts> {
    if !statements.iter().all(|statement| {
        matches!(
            statement.node,
            ast::Statement::Assignment(_) | ast::Statement::Return(_) | ast::Statement::Expr(_) | ast::Statement::Pass
        )
    }) {
        return None;
    }
    let mut builder = BodyFactsBuilder {
        info,
        owners,
        domain,
        method,
        root,
        bindings: HashMap::new(),
        facts: NativeBodyFacts::default(),
    };
    builder.seed_method_parameters()?;
    builder
        .facts
        .guards
        .push(NativeFactGuard::TargetDeclarations(domain.declarations.clone()));
    if let Some((span, binding)) = root {
        builder
            .facts
            .guards
            .push(NativeFactGuard::Function(span, binding.clone()));
    }
    for statement in statements {
        match &statement.node {
            ast::Statement::Assignment(assignment) => builder.assignment(assignment, statement.span)?,
            ast::Statement::Return(Some(value)) => {
                let returned = builder.expression(value)?;
                builder.validate_return(&returned)?;
            }
            ast::Statement::Expr(value) => {
                builder.expression(value)?;
            }
            ast::Statement::Return(None) | ast::Statement::Pass => {}
            _ => return None,
        }
    }
    (!builder.facts.expressions.is_empty()).then_some(builder.facts)
}

impl BodyFactsBuilder<'_> {
    /// Close one exact selected local method using a retained receiver or its accepted static parameter snapshot.
    fn method_value(
        &mut self,
        receiver: &Spanned<ast::Expr>,
        span: Span,
        values: &[NativeValue],
    ) -> Option<NativeValue> {
        // ---- Context: preserve canonical dispatch before ordinary or selected classification ----
        let identity = self.info.resolved_identity(span)?;
        self.facts
            .guards
            .push(NativeFactGuard::Reference(span, identity.clone()));
        let Some(frame) = selected_method(identity, self.owners, self.info) else {
            if self.domain.method(identity) {
                return None;
            }
            self.expression(receiver)?;
            return Some(NativeValue {
                ty: self.info.expr_type(span)?.clone(),
                owner: None,
                parameter: None,
                holes: BTreeSet::new(),
                native: false,
            });
        };

        // ---- Context: close only the original owner signature at accepted argument types ----
        let params = self.info.call_site_callable_params(span)?.to_vec();
        if params.len() != frame.binding.params.len() || values.len() != params.len() {
            return None;
        }
        let argument = if frame.source.node.receiver.is_none() {
            self.static_owner_argument(receiver, frame, &params, values)?
        } else {
            let value = self.expression(receiver)?;
            if value.owner.as_ref() != Some(frame.owner.identity) {
                return None;
            }
            let ResolvedType::Generic(name, arguments) = value.ty else {
                return None;
            };
            let [argument] = arguments.as_slice() else {
                return None;
            };
            if name != frame.owner.source.name {
                return None;
            }
            argument.clone()
        };
        if !self.admitted_payload(frame.owner, &argument) {
            return None;
        }
        let closed_params = frame
            .binding
            .params
            .iter()
            .map(|parameter| {
                let mut closed = parameter.clone();
                closed.ty = close_method_type(&parameter.ty, frame.owner, &argument, self.info)?;
                Some(closed)
            })
            .collect::<Option<Vec<_>>>()?;
        for ((original, accepted), value) in closed_params.iter().zip(&params).zip(values) {
            if original != accepted || original.ty != value.ty {
                return None;
            }
        }
        let ty = close_method_type(&frame.binding.return_type, frame.owner, &argument, self.info)?;
        let holes = signature_holes(&frame.binding.return_type, frame.owner, self.info)?;
        let owner = match &ty {
            ResolvedType::Generic(name, _) if *name == frame.owner.source.name => Some(frame.owner.identity.clone()),
            _ => None,
        };

        // ---- Context: capture the complete supporting snapshot before publishing any call fact ----
        self.facts.guards.push(frame.owner.guard());
        self.facts
            .guards
            .push(NativeFactGuard::Method(frame.source.span, frame.binding.clone()));
        self.facts
            .guards
            .push(NativeFactGuard::MethodSelection(identity.clone(), frame.source.span));
        self.facts
            .guards
            .push(NativeFactGuard::Reference(span, identity.clone()));
        self.facts.guards.push(NativeFactGuard::Parameters(span, params));
        self.facts.parameters.insert(span, closed_params);
        if frame.source.node.receiver.is_none() {
            self.facts
                .static_receivers
                .insert(span, frame.owner.instantiated_type(argument.clone()));
        }
        let parameter = (!holes.is_empty()).then(|| frame.owner.binder_origin(argument));
        Some(NativeValue {
            ty,
            owner,
            parameter,
            holes,
            native: true,
        })
    }

    /// A static identity-template method uses its accepted concrete parameter type, never contextual reinference.
    fn static_owner_argument(
        &mut self,
        receiver: &Spanned<ast::Expr>,
        frame: MethodFrame<'_>,
        params: &[CallableParam],
        values: &[NativeValue],
    ) -> Option<ResolvedType> {
        let identity = self.info.resolved_identity(receiver.span)?;
        if identity != frame.owner.identity {
            return None;
        }
        let [original] = frame.binding.params.as_slice() else {
            return None;
        };
        let [accepted] = params else {
            return None;
        };
        let [value] = values else {
            return None;
        };
        if !is_owner_parameter(&original.ty, frame.owner, self.info)
            || accepted.ty != value.ty
            || !self.admitted_payload(frame.owner, &accepted.ty)
        {
            return None;
        }
        self.facts
            .guards
            .push(NativeFactGuard::Reference(receiver.span, identity.clone()));
        Some(accepted.ty.clone())
    }

    /// Declared returns retain the current method's binder, never a foreign result's same-spelled permission.
    fn validate_return(&self, value: &NativeValue) -> Option<()> {
        if !value.native {
            return Some(());
        }
        let Some(method) = self.method else {
            return self
                .root
                .and_then(|(_, binding)| (binding.return_type == value.ty).then_some(()));
        };
        let declared = close_method_type(
            &method.binding.return_type,
            method.owner,
            &method.owner.parameter(),
            self.info,
        )?;
        same_signature_type(&declared, &value.ty).then_some(())
    }
    /// Traverse only the bounded expression shapes whose checked decisions this transaction can preserve.
    fn expression(&mut self, expression: &Spanned<ast::Expr>) -> Option<NativeValue> {
        // ---- Context: every traversed expression retains its checked type and canonical leaves ----
        let span = expression.span;
        let checked = self.info.expr_type(span)?.clone();
        self.facts.guards.push(NativeFactGuard::Expression(
            span,
            checked.clone(),
            self.info.expressions.expression_type_identities.get(&span).cloned(),
        ));
        let fallback = NativeValue {
            ty: checked.clone(),
            owner: None,
            parameter: None,
            holes: BTreeSet::new(),
            native: false,
        };

        // ---- Context: bounded source shapes preserve checking rather than re-entering inference ----
        let value = match &expression.node {
            ast::Expr::Literal(_) => fallback,
            ast::Expr::Ident(_) => {
                let identity = self.info.resolved_identity(span)?;
                self.facts
                    .guards
                    .push(NativeFactGuard::Reference(span, identity.clone()));
                self.bindings.get(identity).cloned().unwrap_or(fallback)
            }
            ast::Expr::SelfExpr => self.self_value(span)?,
            ast::Expr::Paren(inner) => self.expression(inner)?,
            ast::Expr::Tuple(elements) => self.tuple_value(elements)?,
            ast::Expr::Index(base, index) => self.index_value(base, index)?,
            ast::Expr::Field(base, name) if name == "0" => self.payload_value(base, span)?,
            ast::Expr::Call(callee, type_args, args) => {
                let values = self.argument_values(args)?;
                match self.constructor_value(callee.span, span, args, &values)? {
                    ConstructorSelection::Ordinary => fallback,
                    ConstructorSelection::Proved(value) => {
                        if !type_args.is_empty() {
                            return None;
                        }
                        *value
                    }
                }
            }
            ast::Expr::MethodCall(receiver, _, type_args, args) if type_args.is_empty() => {
                let values = self.argument_values(args)?;
                self.method_value(receiver, span, &values)?
            }
            ast::Expr::FString(parts) => {
                for part in parts {
                    if let ast::FStringPart::Expr { expr, .. } = part {
                        self.expression(expr)?;
                    }
                }
                fallback
            }
            _ => return None,
        };
        self.retain_expression(span, &checked, value)
    }

    /// Preserve written fixed argument order without introducing spread or named-binding inference.
    fn argument_values(&mut self, args: &[ast::CallArg]) -> Option<Vec<NativeValue>> {
        args.iter()
            .map(|argument| {
                let ast::CallArg::Positional(value) = argument else {
                    return None;
                };
                self.expression(value)
            })
            .collect()
    }

    /// An actual source receiver belongs to this exact checked declaration frame, not a same-spelled local.
    fn self_value(&mut self, span: Span) -> Option<NativeValue> {
        let method = self.method?;
        if method.source.node.receiver != Some(ast::Receiver::Immutable) {
            return None;
        }
        let binding = method.source.node.receiver_binding.as_ref()?;
        let identity = self.info.resolved_write_identity(binding.span, &binding.node)?;
        if identity.kind != SemanticSourceTargetKind::Receiver || self.info.resolved_identity(span) != Some(identity) {
            return None;
        }
        self.facts
            .guards
            .push(NativeFactGuard::Reference(span, identity.clone()));
        self.facts.guards.push(NativeFactGuard::Write(
            binding.span,
            binding.node.clone(),
            identity.clone(),
            self.info.resolved_write_type(binding.span, &binding.node)?.clone(),
        ));
        Some(NativeValue {
            ty: method.owner.instantiated_type(method.owner.parameter()),
            owner: Some(method.owner.identity.clone()),
            parameter: Some(method.owner.binder_origin(method.owner.parameter())),
            holes: BTreeSet::from([vec![0]]),
            native: true,
        })
    }

    /// Tuple closure follows the checked arity and independently proved children; it invents no element types.
    fn tuple_value(&mut self, elements: &[Spanned<ast::Expr>]) -> Option<NativeValue> {
        let values = elements
            .iter()
            .map(|element| self.expression(element))
            .collect::<Option<Vec<_>>>()?;
        let parameter = values
            .iter()
            .filter_map(|value| value.parameter.as_ref())
            .next()
            .cloned();
        if values
            .iter()
            .filter_map(|value| value.parameter.as_ref())
            .any(|name| Some(name) != parameter.as_ref())
        {
            return None;
        }
        let native = values.iter().any(|value| value.native);
        let holes = values
            .iter()
            .enumerate()
            .flat_map(|(index, value)| {
                value.holes.iter().map(move |path| {
                    let mut prefixed = vec![index];
                    prefixed.extend(path);
                    prefixed
                })
            })
            .collect();
        Some(NativeValue {
            ty: ResolvedType::Tuple(values.into_iter().map(|value| value.ty).collect()),
            owner: None,
            parameter,
            holes,
            native,
        })
    }

    /// Slot zero is authorized by the propagated canonical raw owner, with the same payload substitution.
    fn payload_value(&mut self, base: &Spanned<ast::Expr>, span: Span) -> Option<NativeValue> {
        let receiver = self.expression(base)?;
        let identity = receiver.owner.as_ref()?;
        let owner = self.owners.iter().find(|owner| owner.identity == identity)?;
        let ResolvedType::Generic(name, arguments) = &receiver.ty else {
            return None;
        };
        let [payload] = arguments.as_slice() else {
            return None;
        };
        if *name != owner.source.name {
            return None;
        }
        self.facts.field_owners.insert(span, identity.clone());
        Some(NativeValue {
            ty: payload.clone(),
            owner: None,
            parameter: receiver.parameter,
            holes: BTreeSet::from([Vec::new()]),
            native: true,
        })
    }

    /// Retain only a checked in-bounds constant tuple projection, including the existing negative-index rule.
    fn index_value(&mut self, base: &Spanned<ast::Expr>, index: &Spanned<ast::Expr>) -> Option<NativeValue> {
        let receiver = self.expression(base)?;
        let elements = tuple_elements(&receiver.ty)?;
        if self.info.expr_type(index.span) != Some(&ResolvedType::Int) {
            return None;
        }
        self.facts.guards.push(NativeFactGuard::Expression(
            index.span,
            ResolvedType::Int,
            self.info
                .expressions
                .expression_type_identities
                .get(&index.span)
                .cloned(),
        ));
        let index = crate::typechecker::TypeChecker::constant_tuple_index(index)?;
        let count = i64::try_from(elements.len()).ok()?;
        let normalized = if index < 0 { count.checked_add(index)? } else { index };
        let element = elements.get(usize::try_from(normalized).ok()?)?;
        Some(NativeValue {
            ty: element.clone(),
            owner: None,
            parameter: receiver.parameter,
            holes: receiver
                .holes
                .iter()
                .filter_map(|path| {
                    let (head, tail) = path.split_first()?;
                    (*head == usize::try_from(normalized).ok()?).then(|| tail.to_vec())
                })
                .collect(),
            native: receiver.native,
        })
    }

    /// The sole identity-template binder closes uniquely from its already checked slot-zero input type.
    fn constructor_value(
        &mut self,
        callee: Span,
        span: Span,
        args: &[ast::CallArg],
        values: &[NativeValue],
    ) -> Option<ConstructorSelection> {
        let identity = self.info.resolved_identity(callee)?;
        self.facts
            .guards
            .push(NativeFactGuard::Reference(callee, identity.clone()));
        let mut owners = self.owners.iter().filter(|owner| owner.identity == identity);
        let Some(owner) = owners.next().copied() else {
            if self.domain.constructor(identity) {
                return None;
            }
            return Some(ConstructorSelection::Ordinary);
        };
        if owners.next().is_some() {
            return None;
        }
        let [value] = values else {
            return None;
        };
        if args.len() != 1 || !self.admitted_payload(owner, &value.ty) {
            return None;
        }
        let binding = self.info.constructor_field_binding(span)?;
        if binding.field_count != 1 || binding.argument_slots != [0] || !binding.defaulted_slots.is_empty() {
            return None;
        }
        self.facts.guards.push(owner.guard());
        self.facts
            .guards
            .push(NativeFactGuard::Reference(callee, identity.clone()));
        self.facts
            .guards
            .push(NativeFactGuard::Constructor(span, binding.clone()));
        Some(ConstructorSelection::Proved(Box::new(NativeValue {
            ty: owner.instantiated_type(value.ty.clone()),
            owner: Some(identity.clone()),
            parameter: Some(owner.binder_origin(value.ty.clone())),
            holes: BTreeSet::from([vec![0]]),
            native: true,
        })))
    }

    /// External instances are closed int/str; symbolic arguments are confined to the same selected owner frame.
    fn admitted_payload(&self, owner: OwnerFrame<'_>, ty: &ResolvedType) -> bool {
        matches!(ty, ResolvedType::Int | ResolvedType::Str)
            || self
                .method
                .is_some_and(|method| method.owner.identity == owner.identity && *ty == owner.parameter())
    }

    /// Retain source parameter identities before a template body reads its own declared owner binder.
    fn seed_method_parameters(&mut self) -> Option<()> {
        let Some(method) = self.method else {
            return Some(());
        };
        if method.source.node.receiver == Some(ast::Receiver::Mutable)
            || method.source.node.params.len() != method.binding.params.len()
        {
            return None;
        }
        self.facts.guards.push(method.owner.guard());
        self.facts
            .guards
            .push(NativeFactGuard::Method(method.source.span, method.binding.clone()));
        self.facts.guards.push(NativeFactGuard::MethodSelection(
            method.binding.identity.as_ref()?.clone(),
            method.source.span,
        ));
        for (parameter, binding) in method.source.node.params.iter().zip(&method.binding.params) {
            let identity = self
                .info
                .resolved_write_identity(parameter.span, &parameter.node.name)?
                .clone();
            self.facts.guards.push(NativeFactGuard::Write(
                parameter.span,
                parameter.node.name.clone(),
                identity.clone(),
                self.info
                    .resolved_write_type(parameter.span, &parameter.node.name)?
                    .clone(),
            ));
            let owns_binder = is_owner_parameter(&binding.ty, method.owner, self.info);
            let ty = if owns_binder {
                method.owner.parameter()
            } else {
                binding.ty.clone()
            };
            self.bindings.insert(
                identity,
                NativeValue {
                    ty,
                    owner: None,
                    parameter: owns_binder.then(|| method.owner.binder_origin(method.owner.parameter())),
                    holes: if owns_binder {
                        BTreeSet::from([Vec::new()])
                    } else {
                        BTreeSet::new()
                    },
                    native: owns_binder,
                },
            );
        }
        Some(())
    }

    /// Canonical storage closure respects annotations and rejects conflicting writes atomically.
    fn assignment(&mut self, assignment: &ast::AssignmentStmt, span: Span) -> Option<()> {
        let value = self.expression(&assignment.value)?;
        let identity = self
            .info
            .resolved_write_identity(assignment.name_span, &assignment.name)?
            .clone();
        let checked = self.info.assignment_binding_type(span)?;
        self.facts.guards.push(NativeFactGuard::Write(
            assignment.name_span,
            assignment.name.clone(),
            identity.clone(),
            self.info
                .resolved_write_type(assignment.name_span, &assignment.name)?
                .clone(),
        ));
        self.facts
            .guards
            .push(NativeFactGuard::Assignment(span, checked.clone()));
        if let Some(previous) = self.bindings.get(&identity)
            && (previous.native || value.native)
            && (previous.ty != value.ty || previous.owner != value.owner)
        {
            return None;
        }
        if value.native {
            if assignment.ty.is_some() && *checked != value.ty {
                return None;
            }
            if !refines_checked_type(checked, &value, self.info) {
                return None;
            }
            self.facts.assignments.insert(span, value.ty.clone());
        }
        self.bindings.insert(identity, value);
        Some(())
    }

    /// Record a derived occurrence only after concrete checked positions agree with its guarded closure.
    fn retain_expression(&mut self, span: Span, checked: &ResolvedType, value: NativeValue) -> Option<NativeValue> {
        if value.native {
            if self
                .info
                .expressions
                .expression_type_identities
                .get(&span)
                .is_some_and(|identities| value.holes.iter().any(|path| identities.contains_key(path)))
            {
                return None;
            }
            if !refines_checked_type(checked, &value, self.info) {
                return None;
            }
            self.facts.expressions.insert(span, value.ty.clone());
        }
        Some(value)
    }
}

/// Both checked tuple encodings denote the same structural slots; nominal generic owners remain distinct.
fn tuple_elements(ty: &ResolvedType) -> Option<&[ResolvedType]> {
    match ty {
        ResolvedType::Tuple(elements) => Some(elements),
        ResolvedType::Generic(name, elements) if collections::from_str(name) == Some(CollectionTypeId::Tuple) => {
            Some(elements)
        }
        _ => None,
    }
}

/// Compare declared returns without changing the original checked representation retained by guards.
fn same_signature_type(left: &ResolvedType, right: &ResolvedType) -> bool {
    if left == right {
        return true;
    }
    let (Some(left), Some(right)) = (tuple_elements(left), tuple_elements(right)) else {
        return false;
    };
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| same_signature_type(left, right))
}

/// The selected declaration's binder can retain either checked encoding; a fixed nominal of that name cannot.
fn is_owner_parameter(ty: &ResolvedType, owner: OwnerFrame<'_>, info: &TypeCheckInfo) -> bool {
    close_method_type(ty, owner, &owner.parameter(), info) == Some(owner.parameter())
}

/// Locate only the original selected signature's owner-binder and Self argument positions.
fn signature_holes(ty: &ResolvedType, owner: OwnerFrame<'_>, info: &TypeCheckInfo) -> Option<BTreeSet<Vec<usize>>> {
    /// Prefix child positions without pooling unrelated declaration permissions.
    fn children(types: &[ResolvedType], owner: OwnerFrame<'_>, info: &TypeCheckInfo) -> Option<BTreeSet<Vec<usize>>> {
        let mut result = BTreeSet::new();
        for (index, ty) in types.iter().enumerate() {
            for child in signature_holes(ty, owner, info)? {
                let mut path = vec![index];
                path.extend(child);
                result.insert(path);
            }
        }
        Some(result)
    }
    let binder = &owner.construction.type_params[0];
    match ty {
        ResolvedType::TypeVar(name) if name == binder => Some(BTreeSet::from([Vec::new()])),
        ResolvedType::Named(name) if name == binder => {
            if info.declarations.named_type_identities.contains_key(name) {
                return None;
            }
            Some(BTreeSet::from([Vec::new()]))
        }
        ResolvedType::SelfType => Some(BTreeSet::from([vec![0]])),
        ResolvedType::Generic(_, arguments) | ResolvedType::Tuple(arguments) => children(arguments, owner, info),
        ResolvedType::Function(_, _)
        | ResolvedType::Unknown
        | ResolvedType::CallSiteInfer
        | ResolvedType::Ref(_)
        | ResolvedType::RefMut(_) => None,
        _ => Some(BTreeSet::new()),
    }
}

/// Close only the selected declaration's owner binder and Self, preserving every other signature position.
fn close_method_type(
    ty: &ResolvedType,
    owner: OwnerFrame<'_>,
    argument: &ResolvedType,
    info: &TypeCheckInfo,
) -> Option<ResolvedType> {
    let binder = &owner.construction.type_params[0];
    match ty {
        ResolvedType::TypeVar(name) if name == binder => Some(argument.clone()),
        ResolvedType::Named(name) if name == binder => {
            if info.declarations.named_type_identities.contains_key(name) {
                return None;
            }
            Some(argument.clone())
        }
        ResolvedType::SelfType => Some(owner.instantiated_type(argument.clone())),
        ResolvedType::Generic(name, arguments) => Some(ResolvedType::Generic(
            name.clone(),
            arguments
                .iter()
                .map(|ty| close_method_type(ty, owner, argument, info))
                .collect::<Option<Vec<_>>>()?,
        )),
        ResolvedType::Tuple(elements) => Some(ResolvedType::Tuple(
            elements
                .iter()
                .map(|ty| close_method_type(ty, owner, argument, info))
                .collect::<Option<Vec<_>>>()?,
        )),
        ResolvedType::Ref(inner) => Some(ResolvedType::Ref(Box::new(close_method_type(
            inner, owner, argument, info,
        )?))),
        ResolvedType::RefMut(inner) => Some(ResolvedType::RefMut(Box::new(close_method_type(
            inner, owner, argument, info,
        )?))),
        ResolvedType::Function(_, _) | ResolvedType::Unknown | ResolvedType::CallSiteInfer => None,
        ty => Some(ty.clone()),
    }
}

/// Preserve all concrete checked positions while closing only the guarded owner or its original binder holes.
///
/// A checked tuple and a closed `Tuple[...]` signature are the same slots in either encoding: a method declared to
/// return `Tuple[T, T]` closes to the written encoding, while its checked call result can retain the structural one.
fn refines_checked_type(checked: &ResolvedType, native: &NativeValue, info: &TypeCheckInfo) -> bool {
    /// Preserve concrete positions and close only the exact retained owner template holes.
    fn agrees(
        old: &ResolvedType,
        new: &ResolvedType,
        value: &NativeValue,
        info: &TypeCheckInfo,
        path: &[usize],
    ) -> bool {
        if old == new {
            return true;
        }
        if let (Some(old), Some(new)) = (tuple_elements(old), tuple_elements(new)) {
            return slots_agree(old, new, value, info, path);
        }
        match (old, new) {
            (ResolvedType::Generic(old_name, old_args), ResolvedType::Generic(new_name, new_args)) => {
                old_name == new_name && slots_agree(old_args, new_args, value, info, path)
            }
            (ResolvedType::TypeVar(name), _) => {
                value.holes.contains(path) && value.parameter.as_ref().is_some_and(|origin| origin.name == *name)
            }
            (ResolvedType::Named(name), _) => {
                value.holes.contains(path)
                    && value.parameter.as_ref().is_some_and(|origin| origin.name == *name)
                    && !info.declarations.named_type_identities.contains_key(name)
            }
            (ResolvedType::Unknown, _) => value.holes.contains(path) && value.parameter.is_some(),
            _ => false,
        }
    }

    /// Compare positional children of one owner or tuple, each under its own hole path.
    fn slots_agree(
        old: &[ResolvedType],
        new: &[ResolvedType],
        value: &NativeValue,
        info: &TypeCheckInfo,
        path: &[usize],
    ) -> bool {
        old.len() == new.len()
            && old.iter().zip(new).enumerate().all(|(index, (old, new))| {
                let mut child = path.to_vec();
                child.push(index);
                agrees(old, new, value, info, &child)
            })
    }

    agrees(checked, &native.ty, native, info, &[])
}
