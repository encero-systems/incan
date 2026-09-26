//! Method-decorator receivers (#1790): the checks on a decorated method's receiver, and the plan that lets lowering
//! pass a `self` method's receiver through its decorator chain.
//!
//! Source spells a method decorator's receiver the way the method does: `(Box, int) -> str` for a `self` method,
//! `(mut Box, int) -> int` for a `mut self` method, and `def parse(box: Box, ...)` for a function a decorator returns
//! in the method's place. For a `self` method the generated wrapper passes the receiver without giving it up, and
//! lowering rewrites the chain's declarations to take it the same way, so their generated signatures differ from their
//! checked types. That is sound only for a chain the compiler sees whole, so this module records the planned
//! declarations for lowering and refuses (`INCAN-T0116`) every chain, and every use of a planned declaration, it could
//! not plan.

use std::collections::{HashMap, HashSet};

use incan_semantics_core::SemanticSourceTargetKind;

use crate::ast::Visibility;
use crate::ast::{Declaration, Decorator, Expr, FunctionDecl, ParamKind, Program, Receiver, Span, Spanned, Type};
use crate::diagnostics::errors;
use crate::symbols::{CallableParam, ResolvedType};

use super::{MethodDecoratorReceiverRole, MethodDecoratorReceiverSlot, TypeChecker};

/// Return the receiver parameter of a callable shape: its first parameter, when the shape is a callable type.
fn callable_receiver_slot(shape: &ResolvedType) -> Option<&CallableParam> {
    match shape {
        ResolvedType::Function(params, _) => params.first(),
        _ => None,
    }
}

/// The checker's inputs to receiver planning, gathered while bodies are checked.
#[derive(Debug, Default)]
pub(super) struct ReceiverPlanInputs {
    /// Declaration span of the module-level function whose body is being checked; `None` inside methods and outside
    /// bodies.
    pub(super) current_function: Option<(usize, usize)>,
    /// Every value a module-level function returns, keyed by the function's declaration span, so the plan learns which
    /// functions a decorator returns in a method's place.
    returned_values: HashMap<(usize, usize), Vec<ReturnedValue>>,
    /// Spans of `mut` markers on copied-scalar parameters already refused, so an annotation resolved twice reports
    /// once.
    pub(super) reported_marked_scalars: HashSet<(usize, usize)>,
}

/// One value a module-level function returns.
#[derive(Debug, Clone)]
struct ReturnedValue {
    /// Span of the returned expression, or of the name inside any parentheses around it.
    span: Span,
    /// The returned name, when the value is a bare name.
    name: Option<String>,
}

/// How a method decorator declaration's shapes hold the decorated method's receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverShapeSpelling {
    /// No shape holds the receiver in a position of its own, as with a generic `(F) -> F` decorator.
    Absent,
    /// Every shape that holds the receiver is written as a callable type, `(Box, int) -> str`.
    Written,
    /// A shape that holds the receiver is written through a type alias.
    ThroughAlias,
}

/// The declarations of `self`-method decorator chains that take the receiver the way the method's wrapper passes it,
/// keyed by declaration span.
#[derive(Debug, Default)]
struct ReceiverPlan {
    declarations: HashMap<(usize, usize), PlannedReceiverDeclaration>,
    /// Decorator declarations whose returns and uses of the accepted callable were already checked.
    checked_decorators: HashSet<(usize, usize)>,
}

/// One planned declaration: the position of its signature that holds the receiver and the references the chain makes.
#[derive(Debug)]
struct PlannedReceiverDeclaration {
    /// Which positions of the declaration's signature hold the receiver.
    role: MethodDecoratorReceiverRole,
    /// The declaration's source name, for diagnostics.
    name: String,
    /// A decorated method the declaration was planned for, for diagnostics.
    method: String,
    /// The decorator as written on that method, without `@`, for diagnostics.
    display: String,
    /// Spans of the references that name the declaration inside decorator chains: a decorator on a method, or a
    /// `return` of the declaration in a planned decorator or factory.
    chain_references: HashSet<(usize, usize)>,
}

/// The decorated method and the decorator on it through which one chain link is planned.
struct ChainSite<'a> {
    /// The decorated method.
    method: &'a str,
    /// The decorator on that method, without `@`.
    display: &'a str,
    /// The method's callable shape as source spells it, for hints.
    shape: &'a str,
}

/// How a decorated method takes its receiver, and the two forms that receiver has in the method's callable shape.
///
/// Source spells the receiver the way the method does: the owner type for `self`, the owner type marked `mut` for
/// `mut self`. That surface form is what the decorator chain is checked against. The recorded binding keeps the form
/// the method's generated wrapper passes, `&Owner` or `&mut Owner`, which is the type lowering builds the decorated
/// static, adapter and wrapper from.
#[derive(Debug, Clone)]
pub(super) struct DecoratedMethodReceiver {
    /// The owner type the receiver belongs to.
    owner_ty: ResolvedType,
    /// Whether the method takes `mut self`.
    mutable: bool,
}

impl DecoratedMethodReceiver {
    /// Describe the receiver of `owner`'s method with the given receiver; a static method keeps the shared form.
    pub(super) fn new(owner: &str, receiver: Option<Receiver>) -> Self {
        Self {
            owner_ty: ResolvedType::Named(owner.to_string()),
            mutable: receiver == Some(Receiver::Mutable),
        }
    }

    /// Return the receiver parameter of the method's callable shape as source spells it.
    pub(super) fn surface_param(&self) -> CallableParam {
        CallableParam::named("self", self.owner_ty.clone(), ParamKind::Normal).with_mut(self.mutable)
    }

    /// Return `shape` with its receiver written the way this method spells it: without `&`, marked `mut` for `mut
    /// self`.
    ///
    /// Diagnostics use it to spell the shape the source should have written, keeping the rest of the shape and a type
    /// parameter in the receiver position as they are.
    fn surface_shape(&self, shape: &ResolvedType) -> ResolvedType {
        let ResolvedType::Function(params, ret) = shape else {
            return shape.clone();
        };
        let mut params = params.clone();
        if let Some(receiver) = params.first_mut() {
            if let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = &receiver.ty {
                receiver.ty = (**inner).clone();
            }
            receiver.is_mut = self.mutable;
        }
        ResolvedType::Function(params, ret.clone())
    }

    /// Return a checked shape with its receiver in the form the method's generated wrapper passes it.
    ///
    /// This is the one place the checker turns the source spelling into `&Owner` / `&mut Owner`: the recorded binding
    /// is lowering's input for the decorated static and wrapper, and is never shown to the user.
    pub(super) fn with_passing_receiver(&self, shape: ResolvedType) -> ResolvedType {
        let ResolvedType::Function(mut params, ret) = shape else {
            return shape;
        };
        if let Some(receiver) = params.first_mut() {
            let ty = std::mem::replace(&mut receiver.ty, ResolvedType::Unknown);
            receiver.ty = if self.mutable {
                ResolvedType::RefMut(Box::new(ty))
            } else {
                ResolvedType::Ref(Box::new(ty))
            };
            receiver.is_mut = false;
        }
        ResolvedType::Function(params, ret)
    }
}

impl TypeChecker {
    /// Record the value a `return` in a module-level function's body gives back, for receiver planning.
    ///
    /// A bare name, also inside parentheses, is recorded with its own span so the plan can match it to the declaration
    /// or parameter it resolved to; any other expression is recorded as unnamed.
    pub(super) fn record_returned_value(&mut self, expr: Option<&Spanned<Expr>>) {
        let (Some(function), Some(returned)) = (self.receiver_plan_inputs.current_function, expr) else {
            return;
        };
        let mut value = returned;
        while let Expr::Paren(inner) = &value.node {
            value = &**inner;
        }
        let returned = match &value.node {
            Expr::Ident(name) => ReturnedValue {
                span: value.span,
                name: Some(name.clone()),
            },
            _ => ReturnedValue {
                span: returned.span,
                name: None,
            },
        };
        self.receiver_plan_inputs
            .returned_values
            .entry(function)
            .or_default()
            .push(returned);
    }

    /// Type-check one decorator on a method against the method's current callable shape.
    ///
    /// A method decorator gets every check a function decorator gets, and must also spell the receiver the way the
    /// method does: a receiver written `&Owner` or `&mut Owner` in the shape the decorator accepts or returns is
    /// refused with `INCAN-T0110`, and the receiver's `mut` marker must match the method's receiver in both shapes. A
    /// decorator of a `self` method whose shapes name the receiver must be a function of this module applied by name,
    /// since lowering can pass the receiver only to a declaration it can rewrite.
    pub(super) fn apply_user_defined_method_decorator(
        &mut self,
        decorator: &Spanned<Decorator>,
        binding_ty: ResolvedType,
        method_name: &str,
        receiver: &DecoratedMethodReceiver,
    ) -> ResolvedType {
        let display = Self::decorator_display(&decorator.node);
        let callable_ty = self.decorator_callable_type(decorator, &display);

        // ---- Context: the receiver as the decorator's declared shapes spell it, through any type alias ----
        let (accepted, returned) = match &callable_ty {
            ResolvedType::Function(params, ret) => (
                params.first().map(|param| self.expand_type_aliases(param.ty.clone())),
                Some(self.expand_type_aliases((**ret).clone())),
            ),
            _ => (None, None),
        };
        for shape in [accepted.as_ref(), returned.as_ref()].into_iter().flatten() {
            if let Some(slot) = callable_receiver_slot(shape)
                && matches!(slot.ty, ResolvedType::Ref(_) | ResolvedType::RefMut(_))
            {
                self.errors.push(errors::method_decorator_receiver_spelling(
                    &format!("Method decorator '@{display}'"),
                    method_name,
                    &slot.ty.to_string(),
                    &receiver.surface_shape(shape).to_string(),
                    receiver.mutable,
                    decorator.span,
                ));
                return ResolvedType::Unknown;
            }
        }
        if let Some(shape) = accepted.as_ref()
            && let Some(slot) = callable_receiver_slot(shape)
            && slot.is_mut != receiver.mutable
        {
            self.errors.push(errors::method_decorator_receiver_mut_mismatch(
                &format!("Method decorator '@{display}'"),
                method_name,
                receiver.mutable,
                &format!("Write `{}`", receiver.surface_shape(shape)),
                decorator.span,
            ));
            return ResolvedType::Unknown;
        }
        let names_receiver = [accepted.as_ref(), returned.as_ref()]
            .into_iter()
            .flatten()
            .any(|shape| callable_receiver_slot(shape).is_some());
        if names_receiver
            && !receiver.mutable
            && let Some((reason, hint)) = self.method_decorator_not_plannable_reason(decorator)
        {
            self.errors.push(errors::method_decorator_receiver_not_planned(
                &format!("Method decorator '@{display}' cannot decorate `self` method '{method_name}'"),
                reason,
                hint,
                decorator.span,
            ));
            return ResolvedType::Unknown;
        }

        // ---- Context: the ordinary decorator application, then the receiver of the shape it produced ----
        let result = self.apply_decorator_callable(&display, callable_ty, binding_ty, method_name, decorator.span);
        let result = self.expand_type_aliases(result);
        if let Some(slot) = callable_receiver_slot(&result)
            && slot.is_mut != receiver.mutable
        {
            self.errors.push(errors::method_decorator_receiver_mut_mismatch(
                &format!("Method decorator '@{display}' returns a shape that"),
                method_name,
                receiver.mutable,
                &format!("Return `{}`", receiver.surface_shape(&result)),
                decorator.span,
            ));
            return ResolvedType::Unknown;
        }
        result
    }

    /// Say why lowering cannot pass a shared receiver to the declaration a method decorator resolved to, and what to
    /// write instead, if it cannot.
    ///
    /// Lowering passes a `self` method's receiver to the decorator's shapes and to the function the decorator returns
    /// the way the method's wrapper passes it, by rewriting those declarations' signatures. It can rewrite a function
    /// declared in this module only: a decorator imported from another module keeps the signature its own module gave
    /// it, and a decorator reached through a value or a method has no declaration to rewrite.
    fn method_decorator_not_plannable_reason(
        &self,
        decorator: &Spanned<Decorator>,
    ) -> Option<(&'static str, &'static str)> {
        if self.local_function_declaration_span(decorator.span).is_some() {
            return None;
        }
        let declared_elsewhere = self
            .type_info
            .resolved_identity(decorator.span)
            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Function);
        Some(if declared_elsewhere {
            (
                "it is declared in another module, and its shapes name the receiver",
                "Declare the decorator in the module that declares the method's type, or make it generic over the \
                 whole callable, `(F) -> F`",
            )
        } else {
            (
                "it is reached through a value or a method, and its shapes name the receiver",
                "Apply a function declared in this module by name, or make the decorator generic over the whole \
                 callable, `(F) -> F`",
            )
        })
    }

    /// Return the declaration span of the function declared in this module that the reference at `span` resolved to.
    fn local_function_declaration_span(&self, span: Span) -> Option<(usize, usize)> {
        let identity = self.type_info.resolved_identity(span)?;
        if identity.kind != SemanticSourceTargetKind::Function {
            return None;
        }
        let declaration_span = (identity.declaration_span.start, identity.declaration_span.end);
        self.type_info
            .declarations
            .function_bindings_by_span
            .get(&declaration_span)
            .is_some_and(|binding| binding.identity.as_ref() == Some(identity))
            .then_some(declaration_span)
    }

    /// Plan the local declarations that take a decorated `self` method's receiver, and refuse every chain or use the
    /// compiler could not pass that receiver through.
    ///
    /// This pass records each planned declaration (keyed by declaration span) with the references the chain makes,
    /// then refuses any other reference, except a direct call of a returned function in its module, which lowering
    /// passes the same way. Inside a planned decorator, the callable it accepts is only returned: its checked type
    /// spells the receiver as the owner type while lowering passes it as the wrapper does. A `mut self` method's
    /// chain needs no plan: `mut Box` already says how the receiver is passed. The pass runs once every body is
    /// checked, because a decorator may be declared after the type it decorates and the functions it returns are known
    /// only from its checked body.
    pub(super) fn record_method_decorator_receiver_slots(&mut self, program: &Program) {
        // ---- Context: this module's functions, the names it calls directly, and its function decorators ----
        let functions: HashMap<(usize, usize), &FunctionDecl> = program
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Function(function) => Some(((decl.span.start, decl.span.end), function)),
                _ => None,
            })
            .collect();
        let mut direct_callees = HashSet::new();
        crate::ast_walk::any_expr_in_program(program, |expr| {
            if let Expr::Call(callee, _, _) = expr
                && matches!(callee.node, Expr::Ident(_))
            {
                direct_callees.insert((callee.span.start, callee.span.end));
            }
            false
        });
        let function_decorators: HashMap<(usize, usize), &str> = functions
            .values()
            .flat_map(|function| {
                function
                    .decorators
                    .iter()
                    .map(move |decorator| ((decorator.span.start, decorator.span.end), function.name.as_str()))
            })
            .collect();

        // ---- Context: the decorator chains of `self` methods ----
        let mut plan = ReceiverPlan::default();
        for decl in &program.declarations {
            let (owner, methods) = match &decl.node {
                Declaration::Model(model) => (&model.name, &model.methods),
                Declaration::Class(class) => (&class.name, &class.methods),
                Declaration::Newtype(newtype) => (&newtype.name, &newtype.methods),
                Declaration::Enum(enum_decl) => (&enum_decl.name, &enum_decl.methods),
                Declaration::Trait(trait_decl) => (&trait_decl.name, &trait_decl.methods),
                _ => continue,
            };
            for method in methods {
                if method.node.receiver == Some(Receiver::Mutable) {
                    continue;
                }
                let key = (owner.clone(), method.node.name.clone());
                let Some(binding) = self.type_info.declarations.decorated_method_bindings.get(&key) else {
                    continue;
                };
                let shape = DecoratedMethodReceiver::new(owner, method.node.receiver)
                    .surface_shape(&binding.original_unbound_ty)
                    .to_string();
                for decorator in &method.node.decorators {
                    if self.is_user_defined_decorator_candidate(&decorator.node) {
                        let display = Self::decorator_display(&decorator.node);
                        let site = ChainSite {
                            method: &method.node.name,
                            display: &display,
                            shape: &shape,
                        };
                        self.plan_method_decorator(decorator, &site, &functions, &mut plan);
                    }
                }
            }
        }

        // ---- Context: every other use of a planned declaration, then the facts lowering reads ----
        self.refuse_receiver_uses_outside_the_chain(&plan, &direct_callees, &function_decorators);
        for (span, planned) in plan.declarations {
            self.type_info.declarations.method_decorator_receiver_slots.insert(
                span,
                MethodDecoratorReceiverSlot {
                    mutable: false,
                    role: planned.role,
                },
            );
        }
    }

    /// Plan one decorator of a `self` method: the decorator or factory itself, the decorators a factory returns, and
    /// the functions each decorator returns in the method's place.
    fn plan_method_decorator(
        &mut self,
        decorator: &Spanned<Decorator>,
        site: &ChainSite<'_>,
        functions: &HashMap<(usize, usize), &FunctionDecl>,
        plan: &mut ReceiverPlan,
    ) {
        let Some(declaration_span) = self.local_function_declaration_span(decorator.span) else {
            return;
        };
        let Some(function) = functions.get(&declaration_span).copied() else {
            return;
        };
        let decorator_reference = (decorator.span.start, decorator.span.end);

        // ---- Context: the decorator declarations in this chain link, with the reference that names each ----
        let mut decorating = Vec::new();
        if decorator.node.is_call {
            let role = MethodDecoratorReceiverRole::Factory;
            match self.receiver_shape_spelling(function, declaration_span, role) {
                ReceiverShapeSpelling::Absent => return,
                ReceiverShapeSpelling::ThroughAlias => {
                    self.push_receiver_shape_through_alias(site, decorator.span);
                    return;
                }
                ReceiverShapeSpelling::Written => {}
            }
            if !self.plan_receiver_declaration(plan, declaration_span, function, role, site, decorator_reference) {
                return;
            }
            let returned = self.returned_values_of(declaration_span);
            for value in returned {
                let returned_decorator = value
                    .name
                    .as_ref()
                    .and_then(|_| self.local_function_declaration_span(value.span))
                    .and_then(|span| functions.get(&span).map(|returned| (span, *returned)));
                match returned_decorator {
                    Some((span, returned)) => decorating.push((span, returned, (value.span.start, value.span.end))),
                    None => self.errors.push(errors::method_decorator_receiver_not_planned(
                        &format!(
                            "Decorator factory '@{}' cannot decorate `self` method '{}'",
                            site.display, site.method
                        ),
                        "it returns something other than a decorator declared in this module, named directly",
                        "Return a decorator declared in this module, by name",
                        value.span,
                    )),
                }
            }
        } else {
            decorating.push((declaration_span, function, decorator_reference));
        }

        // ---- Context: each decorator's shapes and the functions it returns in the method's place ----
        for (span, decorating_function, reference) in decorating {
            let role = MethodDecoratorReceiverRole::Decorator;
            match self.receiver_shape_spelling(decorating_function, span, role) {
                ReceiverShapeSpelling::Absent => continue,
                ReceiverShapeSpelling::ThroughAlias => {
                    self.push_receiver_shape_through_alias(site, decorator.span);
                    continue;
                }
                ReceiverShapeSpelling::Written => {}
            }
            if self.plan_receiver_declaration(plan, span, decorating_function, role, site, reference)
                && plan.checked_decorators.insert(span)
            {
                self.plan_decorator_returns(span, decorating_function, site, functions, plan);
                self.refuse_uses_of_the_decorated_callable(span, decorating_function, site);
            }
        }
    }

    /// Return the values a module-level function's body returns, in source order.
    fn returned_values_of(&self, declaration_span: (usize, usize)) -> Vec<ReturnedValue> {
        self.receiver_plan_inputs
            .returned_values
            .get(&declaration_span)
            .cloned()
            .unwrap_or_default()
    }

    /// Plan what one decorator of a `self` method returns: the decorated callable passes through, a private function of
    /// this module is planned as the method's replacement, and anything else is refused.
    fn plan_decorator_returns(
        &mut self,
        declaration_span: (usize, usize),
        decorator: &FunctionDecl,
        site: &ChainSite<'_>,
        functions: &HashMap<(usize, usize), &FunctionDecl>,
        plan: &mut ReceiverPlan,
    ) {
        let accepted = decorator.params.first().map(|param| param.node.name.as_str());
        let receiver = DecoratedMethodReceiver::new("", None);
        let refused = format!(
            "Method decorator '@{}' cannot decorate `self` method '{}'",
            site.display, site.method
        );
        let hint = format!(
            "Return {} or a function declared in this module, by name",
            accepted.map_or_else(|| "the callable it accepts".to_string(), |name| format!("'{name}'"))
        );
        for value in self.returned_values_of(declaration_span) {
            let Some(name) = &value.name else {
                self.errors.push(errors::method_decorator_receiver_not_planned(
                    &refused,
                    "it returns an expression, and a decorator whose shapes name the receiver returns the callable it \
                     accepts or a function declared in this module, named directly",
                    &hint,
                    value.span,
                ));
                continue;
            };
            let replacement = self
                .local_function_declaration_span(value.span)
                .and_then(|span| functions.get(&span).map(|replacement| (span, *replacement)));
            if let Some((span, replacement)) = replacement {
                if self.check_method_decorator_replacement(replacement, site.display, site.method, &receiver) {
                    self.plan_receiver_declaration(
                        plan,
                        span,
                        replacement,
                        MethodDecoratorReceiverRole::Replacement,
                        site,
                        (value.span.start, value.span.end),
                    );
                }
                continue;
            }
            let names_parameter = self
                .type_info
                .resolved_identity(value.span)
                .is_none_or(|identity| identity.kind == SemanticSourceTargetKind::Parameter);
            if accepted == Some(name.as_str()) && names_parameter {
                continue;
            }
            self.errors.push(errors::method_decorator_receiver_not_planned(
                &refused,
                &format!(
                    "it returns '{name}', which is neither the callable it accepts nor a function declared in this \
                     module"
                ),
                &hint,
                value.span,
            ));
        }
    }

    /// Refuse every use of the callable a planned decorator accepts other than returning it.
    ///
    /// Lowering rewrites the accepted callable's receiver to the form the method's wrapper passes, while the checked
    /// body sees the owner type, so a call of it, or passing it on, would disagree with the generated signature.
    fn refuse_uses_of_the_decorated_callable(
        &mut self,
        declaration_span: (usize, usize),
        decorator: &FunctionDecl,
        site: &ChainSite<'_>,
    ) {
        let Some(accepted) = decorator.params.first() else {
            return;
        };
        let parameter = (accepted.span.start, accepted.span.end);
        let returned: HashSet<(usize, usize)> = self
            .returned_values_of(declaration_span)
            .iter()
            .filter(|value| value.name.as_deref() == Some(accepted.node.name.as_str()))
            .map(|value| (value.span.start, value.span.end))
            .collect();
        let mut uses: Vec<(usize, usize)> = self
            .type_info
            .references
            .resolved_identities
            .iter()
            .filter(|(span, identity)| {
                identity.kind == SemanticSourceTargetKind::Parameter
                    && (identity.declaration_span.start, identity.declaration_span.end) == parameter
                    && **span != parameter
            })
            .map(|(span, _)| *span)
            .filter(|span| !returned.contains(span))
            .collect();
        uses.sort_unstable();
        let name = &accepted.node.name;
        for (start, end) in uses {
            self.errors.push(errors::method_decorator_receiver_not_planned(
                &format!("'{name}' cannot be used here"),
                &format!(
                    "it is the callable '@{}' decorates, and a decorator of `self` method '{}' only returns it",
                    decorator.name, site.method
                ),
                &format!(
                    "Return '{name}' unchanged, or make '{}' generic over the whole callable, `(F) -> F`",
                    decorator.name
                ),
                Span::new(start, end),
            ));
        }
    }

    /// Record one declaration of a `self`-method decorator chain in `plan`; return whether it can take the receiver.
    ///
    /// A planned declaration is private: another module calls a function through its checked signature, which does not
    /// say how the receiver is passed. A declaration takes the receiver in one role only.
    fn plan_receiver_declaration(
        &mut self,
        plan: &mut ReceiverPlan,
        span: (usize, usize),
        function: &FunctionDecl,
        role: MethodDecoratorReceiverRole,
        site: &ChainSite<'_>,
        reference: (usize, usize),
    ) -> bool {
        let refused = match role {
            MethodDecoratorReceiverRole::Replacement => format!(
                "Function '{}' cannot take the place of `self` method '{}'",
                function.name, site.method
            ),
            MethodDecoratorReceiverRole::Decorator | MethodDecoratorReceiverRole::Factory => format!(
                "Method decorator '@{}' cannot decorate `self` method '{}'",
                site.display, site.method
            ),
        };
        if let Some(planned) = plan.declarations.get_mut(&span) {
            if planned.role != role {
                self.errors.push(errors::method_decorator_receiver_not_planned(
                    &refused,
                    &format!(
                        "'{}' already takes the receiver in another position of a decorator chain",
                        function.name
                    ),
                    "Use a separate function for each position in the chain",
                    Span::new(reference.0, reference.1),
                ));
                return false;
            }
            planned.chain_references.insert(reference);
            return true;
        }
        if function.visibility != Visibility::Private {
            self.errors.push(errors::method_decorator_receiver_not_planned(
                &refused,
                &format!(
                    "'{}' is declared `pub`, and another module would call it through a type that does not say how \
                     the receiver is passed",
                    function.name
                ),
                &format!(
                    "Remove `pub` from '{}', and give other modules a function of their own",
                    function.name
                ),
                Span::new(span.0, span.1),
            ));
            return false;
        }
        plan.declarations.insert(
            span,
            PlannedReceiverDeclaration {
                role,
                name: function.name.clone(),
                method: site.method.to_string(),
                display: site.display.to_string(),
                chain_references: HashSet::from([reference]),
            },
        );
        true
    }

    /// Refuse every reference to a planned declaration outside its decorator chain.
    ///
    /// A planned declaration's generated signature takes the receiver the way the method's wrapper passes it, which
    /// its checked type does not say, so any use the chain did not make would disagree with it. A direct call of a
    /// returned function in this module is the one exception: lowering passes its first argument the same way.
    fn refuse_receiver_uses_outside_the_chain(
        &mut self,
        plan: &ReceiverPlan,
        direct_callees: &HashSet<(usize, usize)>,
        function_decorators: &HashMap<(usize, usize), &str>,
    ) {
        let mut uses: Vec<((usize, usize), (usize, usize))> = self
            .type_info
            .references
            .resolved_identities
            .iter()
            .filter(|(_, identity)| identity.kind == SemanticSourceTargetKind::Function)
            .filter_map(|(span, identity)| {
                let declaration = (identity.declaration_span.start, identity.declaration_span.end);
                let local = self
                    .type_info
                    .declarations
                    .function_bindings_by_span
                    .get(&declaration)
                    .is_some_and(|binding| binding.identity.as_ref() == Some(identity));
                (local && plan.declarations.contains_key(&declaration)).then_some((*span, declaration))
            })
            .collect();
        uses.sort_unstable();
        for (span, declaration) in uses {
            let Some(planned) = plan.declarations.get(&declaration) else {
                continue;
            };
            let direct_call =
                planned.role == MethodDecoratorReceiverRole::Replacement && direct_callees.contains(&span);
            if planned.chain_references.contains(&span) || direct_call {
                continue;
            }
            let name = &planned.name;
            let (refused, reason, hint) = match (planned.role, function_decorators.get(&span)) {
                (MethodDecoratorReceiverRole::Replacement, _) => (
                    format!("'{name}' cannot be used here"),
                    format!(
                        "it takes the place of `self` method '{}' through '@{}', so it is only returned there or \
                         called directly",
                        planned.method, planned.display
                    ),
                    format!("Call '{name}' directly, or give this use a function of its own"),
                ),
                (_, Some(function)) => (
                    format!("Decorator '@{name}' cannot decorate function '{function}'"),
                    "its shapes name the receiver of a `self` method, so it only decorates `self` methods".to_string(),
                    format!("Give '{function}' a decorator of its own"),
                ),
                _ => (
                    format!("'{name}' cannot be used here"),
                    format!(
                        "its shapes name the receiver of `self` method '{}', so it is only applied as a decorator of \
                         `self` methods",
                        planned.method
                    ),
                    format!("Apply '@{name}' only to `self` methods, and give this use a function of its own"),
                ),
            };
            self.errors.push(errors::method_decorator_receiver_not_planned(
                &refused,
                &reason,
                &hint,
                Span::new(span.0, span.1),
            ));
        }
    }

    /// Refuse a method decorator whose receiver shape is written through a type alias.
    fn push_receiver_shape_through_alias(&mut self, site: &ChainSite<'_>, span: Span) {
        self.errors.push(errors::method_decorator_receiver_not_planned(
            &format!(
                "Method decorator '@{}' cannot decorate `self` method '{}'",
                site.display, site.method
            ),
            "a shape that names the receiver is written through a type alias",
            &format!(
                "Write each shape that names the receiver as a callable type, such as `{}`",
                site.shape
            ),
            span,
        ));
    }

    /// Say whether a decorator declaration's shapes hold the receiver, and whether they are written as callable types.
    ///
    /// The positions are the first parameter and the result of a decorator, and the same two inside the decorator
    /// shape a factory returns. A position holds the receiver when the checker resolved it to a callable type; lowering
    /// can rewrite its receiver only when the source wrote it as one.
    fn receiver_shape_spelling(
        &self,
        function: &FunctionDecl,
        span: (usize, usize),
        role: MethodDecoratorReceiverRole,
    ) -> ReceiverShapeSpelling {
        let Some(binding) = self.type_info.declarations.function_bindings_by_span.get(&span) else {
            return ReceiverShapeSpelling::Absent;
        };
        // Each pair is an annotation as written and the type the checker resolved for it.
        let positions: Vec<(&Type, &ResolvedType)> = match role {
            MethodDecoratorReceiverRole::Decorator => {
                let mut positions = vec![(&function.return_type.node, &binding.return_type)];
                if let (Some(param), Some(resolved)) = (function.params.first(), binding.params.first()) {
                    positions.push((&param.node.ty.node, &resolved.ty));
                }
                positions
            }
            MethodDecoratorReceiverRole::Factory => match (&function.return_type.node, &binding.return_type) {
                (Type::Function(params, ret), ResolvedType::Function(resolved_params, resolved_ret)) => {
                    let mut positions = vec![(&ret.node, &**resolved_ret)];
                    if let (Some(param), Some(resolved)) = (params.first(), resolved_params.first()) {
                        positions.push((&param.node, &resolved.ty));
                    }
                    positions
                }
                (annotation, resolved) => vec![(annotation, resolved)],
            },
            MethodDecoratorReceiverRole::Replacement => Vec::new(),
        };
        let mut holding = positions
            .into_iter()
            .filter(|(_, resolved)| callable_receiver_slot(&self.expand_type_aliases((*resolved).clone())).is_some())
            .map(|(annotation, _)| annotation)
            .peekable();
        if holding.peek().is_none() {
            ReceiverShapeSpelling::Absent
        } else if holding.all(|annotation| matches!(annotation, Type::Function(..))) {
            ReceiverShapeSpelling::Written
        } else {
            ReceiverShapeSpelling::ThroughAlias
        }
    }

    /// Check the receiver of a function a method decorator returns in the method's place; return whether it takes the
    /// receiver the way the method does.
    ///
    /// The replacement declares the receiver as its first parameter, spelled the way the method spells it: `box: Box`
    /// for a `self` method, `mut box: Box` for a `mut self` method, whose changes the caller sees.
    fn check_method_decorator_replacement(
        &mut self,
        replacement: &FunctionDecl,
        display: &str,
        method_name: &str,
        receiver: &DecoratedMethodReceiver,
    ) -> bool {
        let Some(first) = replacement.params.first() else {
            return false;
        };
        let param = &first.node;
        let owner = match &param.ty.node {
            Type::Ref(inner) | Type::RefMut(inner) => inner.node.to_string(),
            other => other.to_string(),
        };
        let spelled = if receiver.mutable {
            format!("mut {}: {owner}", param.name)
        } else {
            format!("{}: {owner}", param.name)
        };
        let subject = format!("Function '{}', returned by '@{display}',", replacement.name);
        if matches!(param.ty.node, Type::Ref(_) | Type::RefMut(_)) {
            self.errors.push(errors::method_decorator_receiver_spelling(
                &subject,
                method_name,
                &param.ty.node.to_string(),
                &spelled,
                receiver.mutable,
                param.ty.span,
            ));
            return false;
        }
        if param.is_mut != receiver.mutable {
            self.errors.push(errors::method_decorator_receiver_mut_mismatch(
                &subject,
                method_name,
                receiver.mutable,
                &format!("Declare its first parameter as `{spelled}`"),
                first.span,
            ));
            return false;
        }
        true
    }
}
