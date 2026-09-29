//! Closures that capture local values in function-typed slots (#1561).
//!
//! A function type, `(A) -> R`, holds a named function or a closure. The backend spells a function type as a Rust
//! function pointer, which holds a named function or a closure that captures nothing. A closure that reads a local
//! of an enclosing callable (`self` included) captures it, since it reads each outer local as the value the local
//! holds when the closure is built, and a local partial holds its presets: each is a *capturing callable*, whose
//! Rust type is its own. A capturing callable builds where that type can be kept:
//!
//! - as the value of a new local binding, which takes the callable's own type and is not reassigned afterwards;
//! - as the callee of a call;
//! - as an argument for a function-typed parameter of a function or method of this module whose body only calls the
//!   parameter, which is spelled `impl Fn(A) -> R` ([`TypeCheckInfo::closure_holding_params`]);
//! - as the value of the one `return` of a function or method of this module whose returned closure captures, whose
//!   return type is spelled `impl Fn(A) -> R` ([`TypeCheckInfo::closure_returning_callables`]).
//!
//! A function of this module qualifies only when it is called by name and never read as a value, since such a
//! function has no function-pointer type of its own; a free function qualifies only when it is not `pub`, since
//! another module may read it as a value. A function that is `async`, decorated, generic (a call may name its type
//! arguments, which Rust does not allow beside an `impl` parameter), a generator, or a method that implements a trait
//! method or belongs to a class hierarchy does not qualify either.
//!
//! Anywhere else a function type holds the callable (a parameter the body stores, returns or passes on, a function
//! of another project module or library, a field, an element, a branch of an `if` or a `match`, a reassignment, a
//! preset) it is refused with `INCAN-T0001`. A named function and a closure that captures nothing are accepted in
//! every function-typed slot, as before.
//!
//! [`TypeCheckInfo::closure_holding_params`]: crate::typechecker::TypeCheckInfo
//! [`TypeCheckInfo::closure_returning_callables`]: crate::typechecker::TypeCheckInfo

use std::collections::{HashMap, HashSet};

use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use crate::ast::*;
use crate::ast_walk::{any_expr_in_body, any_expr_in_expr, any_expr_in_program};
use crate::body_ir::free_vars::free_vars_in_closure_body;
use crate::diagnostics::errors;
use crate::symbols::{CallableParam, MethodInfo, ResolvedType, SymbolKind, TypeInfo};

use super::TypeChecker;

/// The capturing callables of the module being checked, and the callables of this module that hold them.
#[derive(Debug, Default, Clone)]
pub(super) struct CapturingCallables {
    /// The function-typed parameters, by index, of each function or method of this module that only calls them.
    admitting_params: HashMap<CanonicalSymbolId, HashSet<usize>>,
    /// Functions and methods of this module whose one returned value is a capturing closure.
    closure_returning: HashSet<CanonicalSymbolId>,
    /// Closure expressions that capture a local of an enclosing callable, by span.
    capturing_closures: HashSet<(usize, usize)>,
    /// Declaration spans of the locals bound to a capturing callable.
    capturing_bindings: HashSet<(usize, usize)>,
}

/// One function or method whose parameters and return this module may spell as `impl Fn(...) -> R`.
struct CallableCandidate<'a> {
    identity: CanonicalSymbolId,
    params: &'a [Spanned<Param>],
    resolved_params: &'a [CallableParam],
    return_type: &'a Spanned<Type>,
    resolved_return: &'a ResolvedType,
    body: &'a [Spanned<Statement>],
}

/// Names this module reads as values rather than calling them: identifiers read other than as a callee, and member
/// names read other than as a method call's method.
#[derive(Debug, Default)]
struct ValueReads {
    names: HashSet<String>,
    members: HashSet<String>,
}

impl TypeChecker {
    /// Decide, before any body is checked, which function-typed parameters and returns of this module's functions and
    /// methods hold a capturing callable, and record them for lowering ([`CapturingCallables`]).
    pub(super) fn plan_closure_holding_callables(&mut self, program: &Program) {
        self.capturing_callables = CapturingCallables::default();
        if self.checks_standard_library_source() {
            return;
        }
        let reads = value_reads(program);
        let extended_classes = program
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Class(class) => class.extends.clone(),
                _ => None,
            })
            .collect::<HashSet<_>>();
        for decl in &program.declarations {
            match &decl.node {
                Declaration::Function(function) => {
                    if function.visibility != Visibility::Private
                        || !function.decorators.is_empty()
                        || function.is_async()
                        || !function.type_params.is_empty()
                        || reads.names.contains(&function.name)
                    {
                        continue;
                    }
                    let Some(symbol_id) = self.symbols.lookup(&function.name) else {
                        continue;
                    };
                    let Some(identity) = self.symbols.identity_of(symbol_id).cloned() else {
                        continue;
                    };
                    let Some(SymbolKind::Function(info)) =
                        self.symbols.get(symbol_id).map(|symbol| symbol.kind.clone())
                    else {
                        continue;
                    };
                    self.plan_closure_holding_callable(&CallableCandidate {
                        identity,
                        params: &function.params,
                        resolved_params: &info.params,
                        return_type: &function.return_type,
                        resolved_return: &info.return_type,
                        body: &function.body,
                    });
                }
                Declaration::Model(model) => {
                    self.plan_closure_holding_methods(&model.name, &model.traits, &model.methods, &reads);
                }
                Declaration::Class(class) => {
                    if class.extends.is_some() || extended_classes.contains(&class.name) {
                        continue;
                    }
                    self.plan_closure_holding_methods(&class.name, &class.traits, &class.methods, &reads);
                }
                Declaration::Newtype(newtype) => {
                    self.plan_closure_holding_methods(&newtype.name, &newtype.traits, &newtype.methods, &reads);
                }
                Declaration::Enum(enum_decl) => {
                    self.plan_closure_holding_methods(&enum_decl.name, &enum_decl.traits, &enum_decl.methods, &reads);
                }
                _ => {}
            }
        }
    }

    /// Plan the methods of one type: each method that takes `self` or `mut self`, is neither `async`, decorated,
    /// generic nor overloaded, is not read as a member value anywhere in the module, and implements no method of a
    /// trait the type adopts. A type that adopts a trait whose methods are unknown plans none of its methods.
    fn plan_closure_holding_methods(
        &mut self,
        type_name: &str,
        traits: &[Spanned<TraitBound>],
        methods: &[Spanned<MethodDecl>],
        reads: &ValueReads,
    ) {
        let trait_methods = traits
            .iter()
            .map(|bound| {
                self.lookup_trait_info(&bound.node.name)
                    .map(|info| info.methods.keys().cloned().collect::<Vec<_>>())
            })
            .collect::<Option<Vec<_>>>();
        // A trait whose methods are unknown may declare any of this type's methods.
        let Some(trait_methods) = trait_methods.map(|names| names.into_iter().flatten().collect::<HashSet<_>>()) else {
            return;
        };
        for method in methods {
            let method = &method.node;
            let Some(body) = &method.body else {
                continue;
            };
            if method.receiver.is_none()
                || method.trait_target.is_some()
                || !method.decorators.is_empty()
                || method.is_async()
                || !method.type_params.is_empty()
                || trait_methods.contains(&method.name)
                || reads.members.contains(&method.name)
            {
                continue;
            }
            let Some(info) = self.own_method_info(type_name, &method.name) else {
                continue;
            };
            let Some(identity) = info.identity.clone() else {
                continue;
            };
            self.plan_closure_holding_callable(&CallableCandidate {
                identity,
                params: &method.params,
                resolved_params: &info.params,
                return_type: &method.return_type,
                resolved_return: &info.return_type,
                body,
            });
        }
    }

    /// Return the collected declaration of `type_name`'s own method `method`, unless it is overloaded.
    fn own_method_info(&self, type_name: &str, method: &str) -> Option<MethodInfo> {
        let (methods, overloads) = match self.lookup_semantic_type_info(type_name)? {
            TypeInfo::Model(model) => (&model.methods, &model.method_overloads),
            TypeInfo::Class(class) => (&class.methods, &class.method_overloads),
            TypeInfo::Newtype(newtype) => (&newtype.methods, &newtype.method_overloads),
            TypeInfo::Enum(enum_info) => (&enum_info.methods, &enum_info.method_overloads),
            TypeInfo::Builtin | TypeInfo::TypeAlias => return None,
        };
        if overloads.get(method).is_some_and(|overloads| overloads.len() > 1) {
            return None;
        }
        methods.get(method).cloned()
    }

    /// Record which function-typed parameters of one candidate its body only calls, and whether its one returned
    /// value is a capturing closure.
    ///
    /// A parameter qualifies when every read of it in the body is the callee of a call outside any closure or
    /// generator expression: a closure or a generator would hold the parameter itself, which a closure-holding
    /// parameter cannot give, and any other read passes it on as a value. A function whose return type is a function
    /// type returns a capturing closure when its body has exactly one `return`, whose value is a closure that reads a
    /// parameter or a local of the function and not `self`, or a local partial.
    fn plan_closure_holding_callable(&mut self, candidate: &CallableCandidate<'_>) {
        if body_yields(candidate.body) {
            return;
        }
        let mut admitted = HashSet::new();
        for (index, param) in candidate.params.iter().enumerate() {
            let Some(resolved) = candidate.resolved_params.get(index) else {
                continue;
            };
            if param.node.is_mut
                || param.node.kind != ParamKind::Normal
                || !matches!(resolved.ty, ResolvedType::Function(..))
                || !body_only_calls(candidate.body, &param.node.name)
            {
                continue;
            }
            admitted.insert(index);
            self.type_info
                .calls
                .closure_holding_params
                .insert((param.span.start, param.span.end));
        }
        if !admitted.is_empty() {
            self.capturing_callables
                .admitting_params
                .insert(candidate.identity.clone(), admitted);
        }
        if matches!(candidate.resolved_return, ResolvedType::Function(..))
            && let [value] = returned_values(candidate.body).as_slice()
            && returns_capturing_closure(value, candidate)
        {
            self.capturing_callables
                .closure_returning
                .insert(candidate.identity.clone());
            self.type_info
                .calls
                .closure_returning_callables
                .insert((candidate.return_type.span.start, candidate.return_type.span.end));
        }
    }

    // ========================================================================
    // Capturing callables
    // ========================================================================

    /// Record that the closure at `span` captures when a name its body reads, other than its own parameters and the
    /// names its body binds, resolves to a local of an enclosing callable, or when it reads `self`.
    ///
    /// Called before the closure's parameters are defined, so each name resolves in the scope the closure is built in.
    pub(super) fn note_closure_captures(&mut self, params: &[Spanned<Param>], body: &Spanned<Expr>, span: Span) {
        let reads_self = any_expr_in_expr(&body.node, |expr| matches!(expr, Expr::SelfExpr));
        let captures_local = free_vars_in_closure_body(params, body).iter().any(|name| {
            self.lookup_symbol(name)
                .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::Variable(_)))
        });
        if reads_self || captures_local {
            self.capturing_callables
                .capturing_closures
                .insert((span.start, span.end));
        }
    }

    /// Return whether the module being checked is a module of the standard library, whose function-typed slots,
    /// closures and trait defaults are spelled by the standard library's own lowering rules and are left as they are.
    ///
    /// A module is one when a harness says so ([`TypeChecker::set_standard_library_source`]), when its path is under
    /// `std`, and when the checker's provider plan carries an SDK bootstrap grant: the toolchain then compiles one SDK
    /// component from its own source, whose modules are checked under their physical paths (`derives.collection`)
    /// rather than their `std.*` paths.
    pub(super) fn checks_standard_library_source(&self) -> bool {
        self.standard_library_source
            || self.provider_plan.bootstrap_sdk_namespace_roots().next().is_some()
            || self
                .current_module_path
                .as_ref()
                .and_then(|path| path.first())
                .is_some_and(|root| root == incan_lang::lang::stdlib::STDLIB_ROOT)
    }

    /// Describe `expr`, already checked, when it is a capturing callable: a closure that captures, a local partial, a
    /// local bound to a capturing callable, or a call of a function of this module that returns a capturing closure.
    /// Nothing is described in a module of the standard library.
    fn capturing_callable_description(&self, expr: &Spanned<Expr>) -> Option<String> {
        if self.checks_standard_library_source() {
            return None;
        }
        match &expr.node {
            Expr::Paren(inner) => self.capturing_callable_description(inner),
            Expr::Closure(..)
                if self
                    .capturing_callables
                    .capturing_closures
                    .contains(&(expr.span.start, expr.span.end)) =>
            {
                Some("A closure that captures local values".to_string())
            }
            Expr::Partial(_) => Some("A local partial, which holds its presets,".to_string()),
            Expr::Ident(name) => {
                let symbol = self.lookup_symbol(name)?;
                (matches!(symbol.kind, SymbolKind::Variable(_))
                    && self
                        .capturing_callables
                        .capturing_bindings
                        .contains(&(symbol.span.start, symbol.span.end)))
                .then(|| format!("'{name}', which holds a closure that captures local values,"))
            }
            Expr::Call(..) | Expr::MethodCall(..) => {
                let identity = self.type_info.resolved_identity(call_callee_span(expr))?;
                self.capturing_callables
                    .closure_returning
                    .contains(identity)
                    .then(|| format!("The closure '{}' returns", identity.declaration_name))
            }
            _ => None,
        }
    }

    /// Refuse `expr` when it is a capturing callable held where a function type is a function pointer, described by
    /// `slot` ("stored in a collection").
    pub(super) fn refuse_capturing_callable(&mut self, expr: &Spanned<Expr>, slot: &str) {
        let Some(what) = self.capturing_callable_description(expr) else {
            return;
        };
        let error = errors::capturing_callable_in_function_slot(&what, slot, expr.span);
        let already_reported = self
            .errors
            .iter()
            .any(|existing| existing.span == error.span && existing.message == error.message);
        if !already_reported {
            self.errors.push(error);
        }
    }

    /// Refuse each capturing callable `expr`, already checked, holds as a value of its own: an element of a tuple,
    /// list, set or dict literal or of a comprehension, a yielded value, an argument of a construction, a preset of a
    /// local partial, and the value of an `if` branch or a `match` arm.
    pub(super) fn refuse_capturing_callables_held_in(&mut self, expr: &Spanned<Expr>) {
        match &expr.node {
            Expr::Tuple(items) | Expr::Set(items) => {
                for item in items {
                    self.refuse_capturing_callable(item, "held in a tuple or a set");
                }
            }
            Expr::List(entries) => {
                for entry in entries {
                    if let ListEntry::Element(item) = entry {
                        self.refuse_capturing_callable(item, "stored in a list");
                    }
                }
            }
            Expr::Dict(entries) => {
                for entry in entries {
                    if let DictEntry::Pair(_, value) = entry {
                        self.refuse_capturing_callable(value, "stored in a dict");
                    }
                }
            }
            Expr::ListComp(comp) => self.refuse_capturing_callable(&comp.expr, "stored in a list"),
            Expr::DictComp(comp) => self.refuse_capturing_callable(&comp.value, "stored in a dict"),
            Expr::Generator(generator) => self.refuse_capturing_callable(&generator.expr, "yielded by a generator"),
            Expr::Yield(Some(value)) => self.refuse_capturing_callable(value, "yielded by a generator"),
            Expr::Partial(partial) => {
                for arg in &partial.args {
                    self.refuse_capturing_callable(&arg.value, "a preset of a local partial");
                }
            }
            Expr::Constructor(_, args) => self.refuse_capturing_arguments(args, "an argument of a construction"),
            Expr::Call(callee, _, args) if self.callee_builds_a_value(callee) => {
                self.refuse_capturing_arguments(args, "an argument of a construction");
            }
            Expr::MethodCall(base, variant, _, args) if self.names_enum_variant(base, variant) => {
                self.refuse_capturing_arguments(args, "an argument of a construction");
            }
            Expr::Match(_, arms) => {
                for arm in arms {
                    let value = match &arm.node.body {
                        MatchBody::Expr(value) => Some(value),
                        MatchBody::Block(body) => trailing_expression(body),
                    };
                    if let Some(value) = value {
                        self.refuse_capturing_callable(value, "the value of a 'match' arm");
                    }
                }
            }
            Expr::If(if_expr) => {
                for body in std::iter::once(&if_expr.then_body).chain(if_expr.else_body.as_ref()) {
                    if let Some(value) = trailing_expression(body) {
                        self.refuse_capturing_callable(value, "the value of an 'if' branch");
                    }
                }
            }
            _ => {}
        }
    }

    /// Refuse each capturing callable passed in `args` to a construction.
    fn refuse_capturing_arguments(&mut self, args: &[CallArg], slot: &str) {
        for arg in args {
            if let CallArg::Positional(value) | CallArg::Named(_, value) = arg {
                self.refuse_capturing_callable(value, slot);
            }
        }
    }

    /// Return whether a call's callee builds a value that holds its arguments: a model, class or newtype, an enum
    /// variant, or a built-in constructor such as `Some`.
    fn callee_builds_a_value(&self, callee: &Spanned<Expr>) -> bool {
        match &callee.node {
            Expr::Ident(name) => {
                incan_lang::lang::surface::constructors::from_str(name).is_some()
                    || self.lookup_symbol(name).is_some_and(|symbol| {
                        matches!(
                            symbol.kind,
                            SymbolKind::Type(TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Newtype(_))
                                | SymbolKind::Variant(_)
                        )
                    })
            }
            Expr::Field(base, _) => self
                .type_info
                .ident_kind(base.span)
                .is_some_and(|kind| kind == super::IdentKind::TypeName),
            _ => false,
        }
    }

    /// Return whether `base.variant` names a variant of an enum, which `Wrap.Held(...)` constructs.
    fn names_enum_variant(&self, base: &Spanned<Expr>, variant: &str) -> bool {
        match &base.node {
            Expr::Ident(name) => self.lookup_symbol(name).is_some_and(|symbol| match &symbol.kind {
                SymbolKind::Type(TypeInfo::Enum(info)) => info.variants.iter().any(|known| known == variant),
                _ => false,
            }),
            _ => false,
        }
    }

    /// Check the arguments of a call of a function or method, `identity`, declared in the project or in a library,
    /// against its declared parameters `params`: an argument for a function-typed parameter that is a capturing
    /// callable is accepted when the parameter holds any callable of its type, and refused otherwise. A local bound
    /// to a capturing callable is passed by reference, so the local stays usable after the call.
    ///
    /// Only a function or method of this module holds one; a function-typed parameter of the standard library or of
    /// another package refuses it (#1561). A Rust callee, whose parameters its Rust signature types, and one no
    /// identity names are left as they are.
    pub(super) fn check_capturing_call_arguments(
        &mut self,
        identity: Option<&CanonicalSymbolId>,
        callee: &str,
        params: &[CallableParam],
        args: &[CallArg],
    ) {
        let Some(identity) = identity else {
            return;
        };
        if matches!(identity.origin, SymbolOrigin::RustCrate(_)) {
            return;
        }
        let admitted = self.capturing_callables.admitting_params.get(identity).cloned();
        self.check_capturing_arguments(admitted.as_ref(), callee, params, args);
    }

    /// Check the arguments of a call through a callable value, `callee`, such as a local or a parameter of function
    /// type: each of its function-typed parameters is a function pointer, so a capturing callable passed to one is
    /// refused.
    pub(super) fn check_capturing_value_call_arguments(
        &mut self,
        callee: &Spanned<Expr>,
        params: &[CallableParam],
        args: &[CallArg],
    ) {
        let callee = match &strip_parens(callee).node {
            Expr::Ident(name) => name.clone(),
            _ => "the called value".to_string(),
        };
        self.check_capturing_arguments(None, &callee, params, args);
    }

    /// Check each argument bound to a function-typed parameter of `params`: a capturing callable is accepted for a
    /// parameter whose index `admitted` holds, borrowed when it is a local, and refused for any other.
    fn check_capturing_arguments(
        &mut self,
        admitted: Option<&HashSet<usize>>,
        callee: &str,
        params: &[CallableParam],
        args: &[CallArg],
    ) {
        let normal_params = params
            .iter()
            .enumerate()
            .filter(|(_, param)| param.kind == ParamKind::Normal)
            .collect::<Vec<_>>();
        let mut position = 0;
        for arg in args {
            let (index, param, value) = match arg {
                CallArg::Positional(value) => {
                    let Some((index, param)) = normal_params.get(position).copied() else {
                        continue;
                    };
                    position += 1;
                    (index, param, value)
                }
                CallArg::Named(name, value) => {
                    let Some((index, param)) = params
                        .iter()
                        .enumerate()
                        .find(|(_, param)| param.name.as_deref() == Some(name.node.as_str()))
                    else {
                        continue;
                    };
                    (index, param, value)
                }
                _ => continue,
            };
            if !matches!(param.ty, ResolvedType::Function(..)) {
                continue;
            }
            if admitted.is_some_and(|admitted| admitted.contains(&index)) {
                if self.capturing_callable_description(value).is_some()
                    && matches!(strip_parens(value).node, Expr::Ident(_))
                {
                    self.type_info
                        .calls
                        .borrowed_callable_arguments
                        .insert((value.span.start, value.span.end));
                }
                continue;
            }
            let slot = match &param.name {
                Some(name) => format!("passed to the parameter '{name}' of '{callee}'"),
                None => format!("passed to the parameter at position {} of '{callee}'", index + 1),
            };
            self.refuse_capturing_callable(value, &slot);
        }
    }

    /// Check the value of a `return` in the current body: a capturing callable is accepted when the body's callable
    /// returns it as its one returned closure, and refused otherwise.
    pub(super) fn check_capturing_return(&mut self, value: &Spanned<Expr>) {
        let returns_closure = self
            .current_body_identity()
            .is_some_and(|identity| self.capturing_callables.closure_returning.contains(identity));
        if !returns_closure {
            self.refuse_capturing_callable(value, "returned as a function type");
            return;
        }
        let value = strip_parens(value);
        if matches!(value.node, Expr::Closure(..)) {
            self.type_info
                .calls
                .returned_closures
                .insert((value.span.start, value.span.end));
        }
    }

    /// Record a new local, declared at `binding_span` by the assignment at `statement_span`, bound to `value`: a
    /// capturing callable makes the local hold one, and the binding takes the value's own type.
    pub(super) fn note_capturing_binding(&mut self, value: &Spanned<Expr>, binding_span: Span, statement_span: Span) {
        if self.capturing_callable_description(value).is_none() {
            return;
        }
        self.capturing_callables
            .capturing_bindings
            .insert((binding_span.start, binding_span.end));
        self.type_info
            .calls
            .capturing_callable_bindings
            .insert((statement_span.start, statement_span.end));
    }

    /// Refuse an assignment to the existing local `name`, a field or an element of `value` when it is a capturing
    /// callable, and any assignment to a local that holds one: either would give the place another type.
    pub(super) fn refuse_capturing_reassignment(&mut self, name: Option<&str>, value: &Spanned<Expr>, span: Span) {
        if let Some(name) = name
            && let Some(symbol) = self.lookup_symbol(name)
            && self
                .capturing_callables
                .capturing_bindings
                .contains(&(symbol.span.start, symbol.span.end))
        {
            let error = errors::capturing_callable_in_function_slot(
                &format!("'{name}', which holds a closure that captures local values,"),
                "assigned another value",
                span,
            );
            if !self
                .errors
                .iter()
                .any(|existing| existing.span == error.span && existing.message == error.message)
            {
                self.errors.push(error);
            }
            return;
        }
        self.refuse_capturing_callable(value, "assigned to an existing place");
    }
}

/// Collect the names `program` reads as values: an identifier that is not the callee of a call, the name of a
/// decorator, which is applied to the function it decorates as a value, and a member that is not the method of a method
/// call.
fn value_reads(program: &Program) -> ValueReads {
    let mut callees = HashMap::<String, usize>::new();
    let mut idents = HashMap::<String, usize>::new();
    let mut members = HashSet::new();
    any_expr_in_program(program, |expr| {
        match expr {
            Expr::Call(callee, _, _) => {
                if let Expr::Ident(name) = &callee.node {
                    *callees.entry(name.clone()).or_default() += 1;
                }
            }
            Expr::Ident(name) => *idents.entry(name.clone()).or_default() += 1,
            Expr::Field(_, member) => {
                members.insert(member.clone());
            }
            _ => {}
        }
        false
    });
    let mut names = idents
        .into_iter()
        .filter(|(name, count)| callees.get(name).copied().unwrap_or(0) < *count)
        .map(|(name, _)| name)
        .collect::<HashSet<_>>();
    for decl in &program.declarations {
        let (decorators, methods): (&[Spanned<Decorator>], &[Spanned<MethodDecl>]) = match &decl.node {
            Declaration::Function(function) => (&function.decorators, &[]),
            Declaration::Model(model) => (&model.decorators, &model.methods),
            Declaration::Class(class) => (&class.decorators, &class.methods),
            Declaration::Newtype(newtype) => (&newtype.decorators, &newtype.methods),
            Declaration::Enum(enum_decl) => (&enum_decl.decorators, &enum_decl.methods),
            Declaration::Trait(trait_decl) => (&trait_decl.decorators, &trait_decl.methods),
            _ => continue,
        };
        let method_decorators = methods.iter().flat_map(|method| method.node.decorators.iter());
        names.extend(
            decorators
                .iter()
                .chain(method_decorators)
                .map(|decorator| decorator.node.name.clone()),
        );
    }
    ValueReads { names, members }
}

/// Return whether every read of `name` in `body` is the callee of a call made outside any closure or generator
/// expression.
fn body_only_calls(body: &[Spanned<Statement>], name: &str) -> bool {
    let is_name = |expr: &Expr| matches!(expr, Expr::Ident(read) if read == name);
    let mut reads = 0usize;
    let mut calls = 0usize;
    let mut held_by_a_deferred_body = false;
    any_expr_in_body(body, |expr| {
        match expr {
            Expr::Ident(read) if read == name => reads += 1,
            Expr::Call(callee, _, _) if is_name(&callee.node) => calls += 1,
            Expr::Closure(_, closure_body) => {
                held_by_a_deferred_body |= any_expr_in_expr(&closure_body.node, is_name);
            }
            Expr::Generator(_) => held_by_a_deferred_body |= any_expr_in_expr(expr, is_name),
            _ => {}
        }
        false
    });
    reads == calls && !held_by_a_deferred_body
}

/// Return whether `body` yields, which makes its callable a generator.
fn body_yields(body: &[Spanned<Statement>]) -> bool {
    any_expr_in_body(body, |expr| matches!(expr, Expr::Yield(_)))
}

/// Collect the value of every `return` in `body`, in nested blocks too.
fn returned_values(body: &[Spanned<Statement>]) -> Vec<&Spanned<Expr>> {
    let mut values = Vec::new();
    collect_returned_values(body, &mut values);
    values
}

/// Push the value of each `return` in `statements`, and in the blocks they hold, onto `values`.
fn collect_returned_values<'a>(statements: &'a [Spanned<Statement>], values: &mut Vec<&'a Spanned<Expr>>) {
    for statement in statements {
        match &statement.node {
            Statement::Return(Some(value)) => values.push(value),
            Statement::If(if_stmt) => {
                collect_returned_values(&if_stmt.then_body, values);
                for (_, body) in &if_stmt.elif_branches {
                    collect_returned_values(body, values);
                }
                if let Some(body) = &if_stmt.else_body {
                    collect_returned_values(body, values);
                }
            }
            Statement::While(while_stmt) => collect_returned_values(&while_stmt.body, values),
            Statement::For(for_stmt) => collect_returned_values(&for_stmt.body, values),
            Statement::Expr(expr) => collect_expression_returns(expr, values),
            _ => {}
        }
    }
}

/// Push the value of each `return` in the blocks of a `match`, `if` or `loop` expression statement onto `values`.
fn collect_expression_returns<'a>(expr: &'a Spanned<Expr>, values: &mut Vec<&'a Spanned<Expr>>) {
    match &expr.node {
        Expr::Match(_, arms) => {
            for arm in arms {
                if let MatchBody::Block(body) = &arm.node.body {
                    collect_returned_values(body, values);
                }
            }
        }
        Expr::If(if_expr) => {
            collect_returned_values(&if_expr.then_body, values);
            if let Some(body) = &if_expr.else_body {
                collect_returned_values(body, values);
            }
        }
        Expr::Loop(loop_expr) => collect_returned_values(&loop_expr.body, values),
        _ => {}
    }
}

/// Return whether `value`, the one returned value of `candidate`, is a capturing closure it can return: a local
/// partial, or a closure that reads a parameter or a local of the candidate.
///
/// A closure that reads `self` borrows the receiver, which does not outlive the call, so it is not one: its `return` is
/// refused.
fn returns_capturing_closure(value: &Spanned<Expr>, candidate: &CallableCandidate<'_>) -> bool {
    match &strip_parens(value).node {
        Expr::Partial(_) => true,
        Expr::Closure(params, body) => {
            if any_expr_in_expr(&body.node, |expr| matches!(expr, Expr::SelfExpr)) {
                return false;
            }
            let mut locals = candidate
                .params
                .iter()
                .map(|param| param.node.name.clone())
                .collect::<HashSet<_>>();
            collect_local_names(candidate.body, &mut locals);
            free_vars_in_closure_body(params, body)
                .iter()
                .any(|name| locals.contains(name))
        }
        _ => false,
    }
}

/// Collect the names the statements of a body bind, in nested blocks too: assigned and unpacked names and the names a
/// `for` pattern binds.
fn collect_local_names(statements: &[Spanned<Statement>], names: &mut HashSet<String>) {
    for statement in statements {
        match &statement.node {
            Statement::Assignment(assign) => {
                names.insert(assign.name.clone());
            }
            Statement::TupleUnpack(unpack) => names.extend(unpack.names.iter().cloned()),
            Statement::ChainedAssignment(chain) => names.extend(chain.targets.iter().cloned()),
            Statement::If(if_stmt) => {
                collect_local_names(&if_stmt.then_body, names);
                for (_, body) in &if_stmt.elif_branches {
                    collect_local_names(body, names);
                }
                if let Some(body) = &if_stmt.else_body {
                    collect_local_names(body, names);
                }
            }
            Statement::While(while_stmt) => collect_local_names(&while_stmt.body, names),
            Statement::Loop(loop_stmt) => collect_local_names(&loop_stmt.body, names),
            Statement::Unsafe(unsafe_stmt) => collect_local_names(&unsafe_stmt.body, names),
            Statement::For(for_stmt) => {
                let mut bound = HashSet::new();
                crate::body_ir::free_vars::bind_pattern_names(&for_stmt.pattern.node, &mut bound);
                names.extend(bound);
                collect_local_names(&for_stmt.body, names);
            }
            _ => {}
        }
    }
}

/// Return the trailing expression statement of a block, the value a block-bodied branch produces.
fn trailing_expression(body: &[Spanned<Statement>]) -> Option<&Spanned<Expr>> {
    match &body.last()?.node {
        Statement::Expr(value) => Some(value),
        _ => None,
    }
}

/// Return `expr` without the parentheses around it.
fn strip_parens(expr: &Spanned<Expr>) -> &Spanned<Expr> {
    match &expr.node {
        Expr::Paren(inner) => strip_parens(inner),
        _ => expr,
    }
}

/// Return the span a call's callee identity is recorded at: the callee of a function call, the whole call of a
/// method call.
fn call_callee_span(call: &Spanned<Expr>) -> Span {
    match &call.node {
        Expr::Call(callee, _, _) => callee.span,
        _ => call.span,
    }
}
