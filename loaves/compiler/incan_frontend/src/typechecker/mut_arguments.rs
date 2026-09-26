//! `mut` parameters whose changes reach the caller, and the arguments a call passes to them (#1773).
//!
//! A parameter declared `mut` is a mutable binding inside its function. When its type is not `int`, `float`, `bool` or
//! a Rust type, and it is not a `*args` or `**kwargs` parameter, the function's changes to it are also visible to the
//! caller: that is what the `mut` marker on a parameter means. The caller therefore passes a place it owns and may
//! change -- a binding or parameter declared `mut`, a static, `self` in a `mut self` method, or a field or element of
//! one of those. An immutable binding, a literal or any other temporary is refused with `INCAN-T0117` instead of
//! reaching the generated program, where the change would have nowhere to go.
//!
//! The facts are keyed by the callee's declaration identity, recorded when the declaration is collected (for the
//! module being checked and for every source module it imports), and consulted once per call expression after the call
//! has resolved its callee.

use crate::ast::{CallArg, Expr, Param, ParamKind, Span, Spanned, Type};
use crate::diagnostics::errors;
use crate::symbols::{CallableParam, ResolvedType, SymbolKind};
use incan_lang::lang::keywords::{self, KeywordId};
use incan_semantics_core::CanonicalSymbolId;

use super::TypeChecker;

/// One declared parameter of a callable that has at least one `mut` parameter whose changes reach the caller.
#[derive(Debug, Clone)]
pub(crate) struct DeclaredParamSlot {
    /// The parameter's declared name, which a named argument binds to.
    name: String,
    /// Whether the parameter is ordinary or a rest parameter; positional arguments bind only ordinary ones in order.
    kind: ParamKind,
    /// Whether the callable's changes to this parameter are visible to the caller.
    shows_changes_to_caller: bool,
}

impl TypeChecker {
    /// Return whether a declared parameter shows the callee's changes to the caller.
    ///
    /// Only a `mut` parameter can, and not every one: a parameter of type `int`, `float` or `bool` receives its own
    /// copy of the argument, a Rust-typed parameter receives the value itself, and a rest parameter gathers its
    /// arguments into a fresh collection. For those `mut` only makes the parameter reassignable inside the body.
    fn mut_param_shows_changes_to_caller(&self, param: &Param, resolved: &ResolvedType) -> bool {
        if !param.is_mut || param.kind != ParamKind::Normal {
            return false;
        }
        if matches!(
            resolved,
            ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool | ResolvedType::RustPath(_)
        ) {
            return false;
        }
        let head = match &param.ty.node {
            Type::Simple(name) | Type::ConstrainedPrimitive(name, _) | Type::Generic(name, _) => Some(name.as_str()),
            Type::Qualified(segments) => segments.first().map(String::as_str),
            _ => None,
        };
        !head.is_some_and(|name| {
            self.lookup_symbol(name)
                .is_some_and(|symbol| matches!(symbol.kind, SymbolKind::RustItem(_)))
        })
    }

    /// Make a parameter declared `mut` a mutable binding of the body it belongs to.
    ///
    /// `mut` on a parameter always lets the body reassign and change it (`n += 1`, `items.append(x)`), whatever the
    /// parameter's type; whether the caller also sees those changes is a separate fact of the declaration. The name
    /// joins the checker's mutable bindings, as a `mut` local does, so a Rust or C boundary that needs a mutable
    /// argument accepts the parameter too.
    pub(in crate::typechecker) fn bind_mut_param_as_mutable(&mut self, param: &Param) {
        if param.is_mut {
            self.mutable_bindings.insert(param.name.clone());
        }
    }

    /// Record one source callable's declared parameters when any of them shows its changes to the caller.
    ///
    /// `declared` and `resolved` are the declaration's parameters without the receiver, in declaration order, and
    /// `identity` is the declaration identity calls to it resolve to. A callable without such a parameter records
    /// nothing, so calls to it are never inspected.
    pub(in crate::typechecker) fn record_caller_visible_mut_params(
        &mut self,
        identity: Option<&CanonicalSymbolId>,
        declared: &[Spanned<Param>],
        resolved: &[CallableParam],
    ) {
        let Some(identity) = identity else {
            return;
        };
        let slots = declared
            .iter()
            .zip(resolved)
            .map(|(param, resolved)| DeclaredParamSlot {
                name: param.node.name.clone(),
                kind: param.node.kind,
                shows_changes_to_caller: self.mut_param_shows_changes_to_caller(&param.node, &resolved.ty),
            })
            .collect::<Vec<_>>();
        if slots.iter().any(|slot| slot.shows_changes_to_caller) {
            self.caller_visible_mut_params.insert(identity.clone(), slots);
        }
    }

    /// Refuse each argument that is not a mutable place when it binds a `mut` parameter whose changes reach the caller.
    ///
    /// `callee_span` is the span the call recorded its resolved declaration at: the callee expression of a function
    /// call, the whole expression of a method call. Positional arguments bind ordinary parameters in order and named
    /// arguments bind by name; an unpacked argument ends positional binding, since which parameters it fills is not
    /// known here. A call that resolved to no recorded declaration is left alone.
    pub(in crate::typechecker) fn refuse_immutable_arguments_to_mut_params(
        &mut self,
        callee_span: Span,
        args: &[CallArg],
    ) {
        let Some(identity) = self.type_info.resolved_identity(callee_span) else {
            return;
        };
        let Some(slots) = self.caller_visible_mut_params.get(identity).cloned() else {
            return;
        };
        let callee = identity.declaration_name.clone();
        let mut next_positional = Some(0usize);
        for arg in args {
            let (slot, value) = match arg {
                CallArg::Positional(value) => {
                    let slot = next_positional
                        .and_then(|index| slots.get(index))
                        .filter(|slot| slot.kind == ParamKind::Normal);
                    next_positional = slot.and(next_positional.map(|index| index + 1));
                    (slot, value)
                }
                CallArg::Named(name, value) => (slots.iter().find(|slot| slot.name == name.node), value),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => {
                    next_positional = None;
                    continue;
                }
            };
            let Some(slot) = slot else {
                continue;
            };
            if !slot.shows_changes_to_caller || self.is_mutable_place(value) {
                continue;
            }
            let binding = match &value.node {
                Expr::Ident(name) => Some(name.as_str()),
                _ => None,
            };
            let error = errors::immutable_argument_to_mut_parameter(&slot.name, &callee, binding, value.span);
            let already_reported = self
                .errors
                .iter()
                .any(|existing| existing.span == error.span && existing.message == error.message);
            if !already_reported {
                self.errors.push(error);
            }
        }
    }

    /// Return whether an argument names a place the caller owns and may change.
    ///
    /// A binding or parameter declared `mut`, a static, and `self` in a `mut self` method are such places, and so is a
    /// field or element reached from one of them. Every other expression, a literal or a call among them, produces a
    /// temporary the caller could not observe a change to.
    fn is_mutable_place(&self, expr: &Spanned<Expr>) -> bool {
        match &expr.node {
            Expr::Ident(name) => self.binding_is_mutable(name),
            Expr::SelfExpr => self.binding_is_mutable(keywords::as_str(KeywordId::SelfKw)),
            Expr::Field(base, _) | Expr::Index(base, _) | Expr::Paren(base) => self.is_mutable_place(base),
            _ => false,
        }
    }

    /// Return whether the name resolves to a mutable variable binding or to a static.
    fn binding_is_mutable(&self, name: &str) -> bool {
        self.lookup_symbol(name).is_some_and(|symbol| match &symbol.kind {
            SymbolKind::Variable(info) => info.is_mutable,
            SymbolKind::Static(_) => true,
            _ => false,
        })
    }
}
