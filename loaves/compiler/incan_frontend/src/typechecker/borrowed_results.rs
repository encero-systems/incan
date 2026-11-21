//! Borrowed results of borrow-only signatures (RFC 164). A returned view is exact: every return is its declared source
//! place, or a chained call whose relation is that place (slice 06); a native call takes that relation from its Rust
//! signature's lifetime elision (slice 09B). A signature whose inputs are borrows or plain values and whose borrowed
//! result is `from` one of its borrowed inputs (the input itself, an element selected by `Index.index`, the target of
//! `as_ref()`) keeps native passing, and its result is the native reference: no whole-body proof for it or for its
//! callers. Its `from` clause names the input elision ties the result to, and callers hold the result as a loan of that
//! argument. The explicit view route stays for an `Owned[T]` or `Reference[T]` input, an explicit or carried result
//! mode, a receiver or field-path source, and a trait slot.

use super::*;
use incan_lang::lang::types::ownership::OwnershipMode;
use std::borrow::Cow;
use std::collections::HashMap;

/// How one callable's result relates to its inputs, as its signature declares it.
pub(super) struct ResultRoute<'a> {
    /// The `from` clause an ownership contract records; a native borrowed result records none.
    pub(super) source: Option<&'a Spanned<crate::ast::ResultSource>>,
    /// The result annotation the signature resolves; a native borrowed result's root is the native reference.
    pub(super) result: Cow<'a, Spanned<Type>>,
    /// The `from` clause of a native borrowed result, whose input lifetime elision must select.
    pub(super) native_source: Option<&'a Spanned<crate::ast::ResultSource>>,
}

impl TypeChecker {
    /// The route of one callable's result.
    pub(super) fn result_route<'a>(
        &self,
        parameters: &[Spanned<Param>],
        result: &'a Spanned<Type>,
        source: Option<&'a Spanned<crate::ast::ResultSource>>,
    ) -> ResultRoute<'a> {
        match source.filter(|source| self.reaches_result_from_input(parameters, result, source)) {
            Some(native) => ResultRoute {
                source: None,
                result: self
                    .canonical_borrow_root(result, false)
                    .map_or(Cow::Borrowed(result), Cow::Owned),
                native_source: Some(native),
            },
            None => ResultRoute {
                source,
                result: Cow::Borrowed(result),
                native_source: None,
            },
        }
    }

    /// Whether a top-level borrowed result is `from` a borrowed parameter on a signature with no owned or reference
    /// input (which keeps the proof). A `from` naming the receiver or a field path keeps the explicit view route.
    fn reaches_result_from_input(
        &self,
        parameters: &[Spanned<Param>],
        result: &Spanned<Type>,
        source: &Spanned<crate::ast::ResultSource>,
    ) -> bool {
        if self.borrow_referent(result).is_none()
            || !source.node.fields.is_empty()
            || self.has_owned_or_reference_parameter(parameters)
        {
            return false;
        }
        let root = source.node.binding.node.as_str();
        parameters
            .iter()
            .find(|parameter| parameter.node.name == root)
            .is_some_and(|parameter| self.borrow_referent(&parameter.node.ty).is_some())
    }

    /// The referent of a top-level borrow annotation, in either spelling.
    fn borrow_referent<'a>(&self, annotation: &'a Spanned<Type>) -> Option<&'a Spanned<Type>> {
        match &annotation.node {
            Type::Ref(inner) | Type::RefMut(inner) => Some(inner),
            Type::Generic(name, arguments)
                if matches!(
                    self.explicit_ownership_mode(name),
                    Some(OwnershipMode::Borrowed | OwnershipMode::BorrowedMut)
                ) =>
            {
                match arguments.as_slice() {
                    [argument] => Some(argument),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Whether an annotation holds a borrow at any depth, in either spelling: one input lifetime for Rust's elision.
    fn annotation_borrows(&self, annotation: &Type) -> bool {
        match annotation {
            Type::Ref(_) | Type::RefMut(_) => true,
            Type::Generic(name, arguments) => {
                (arguments.len() == 1
                    && matches!(
                        self.explicit_ownership_mode(name),
                        Some(OwnershipMode::Borrowed | OwnershipMode::BorrowedMut)
                    ))
                    || arguments.iter().any(|argument| self.annotation_borrows(&argument.node))
            }
            Type::DottedGeneric(_, arguments) | Type::Tuple(arguments) => {
                arguments.iter().any(|argument| self.annotation_borrows(&argument.node))
            }
            Type::MutParam(inner) => self.annotation_borrows(&inner.node),
            _ => false,
        }
    }

    /// Refuse a native borrowed result whose `from` clause names an input Rust's lifetime elision does not tie it to:
    /// the receiver of a method, or else the only borrowed input (INCAN-T0124, at the clause).
    pub(super) fn check_native_result_elision(
        &mut self,
        name: &str,
        parameters: &[Spanned<Param>],
        source: &Spanned<crate::ast::ResultSource>,
        receiver: Option<&str>,
    ) {
        let named = source.node.binding.node.as_str();
        let borrowed: Vec<&str> = parameters
            .iter()
            .filter(|parameter| self.annotation_borrows(&parameter.node.ty.node))
            .map(|parameter| parameter.node.name.as_str())
            .collect();
        let elided = receiver.or(match borrowed.as_slice() {
            [only] => Some(*only),
            _ => None,
        });
        let error = match elided {
            Some(input) if input == named => return,
            Some(input) => {
                let chosen = if receiver.is_some() {
                    "the receiver"
                } else {
                    "the only borrowed input"
                };
                CompileError::type_error(
                    format!("This borrowed result is tied to '{input}', not to '{named}'"),
                    source.node.binding.span,
                )
                .with_primary_label(format!(
                    "Rust ties a result reached through a call to {chosen}, '{input}'"
                ))
                .with_hint(format!("Write `from {input}`"))
            }
            None => CompileError::type_error(
                format!(
                    "'{name}' has {} borrowed inputs, so its borrowed result cannot be tied to one of them",
                    borrowed.len()
                ),
                source.node.binding.span,
            )
            .with_primary_label("a result reached through a call borrows from one input")
            .with_hint("Take one borrowed input, or return exactly the source place"),
        };
        self.errors.push(error.with_stable_code("INCAN-T0124"));
    }
}

/// While a native borrowed result's body is checked: its source and whether each local's value is reached from it.
#[derive(Debug, Clone, Default)]
pub(super) struct NativeResultReach {
    source: String,
    locals: HashMap<String, bool>,
}

impl NativeResultReach {
    /// Whether a value is reached from the source: the source itself, a place or call result rooted at it (a call
    /// receiving it borrows from it, by elision), or a local only ever bound to such a value.
    fn reaches(&self, value: &Spanned<Expr>) -> bool {
        match &value.node {
            Expr::Ident(name) => *name == self.source || self.locals.get(name).copied().unwrap_or(false),
            Expr::Field(object, _) | Expr::Index(object, _) | Expr::Paren(object) | Expr::Try(object) => {
                self.reaches(object)
            }
            Expr::MethodCall(receiver, _, _, arguments) => self.reaches(receiver) || self.arguments_reach(arguments),
            Expr::Call(_, _, arguments) => self.arguments_reach(arguments),
            _ => false,
        }
    }

    /// Whether any argument of a call is reached from the source.
    fn arguments_reach(&self, arguments: &[CallArg]) -> bool {
        arguments.iter().any(|argument| match argument {
            CallArg::Positional(value)
            | CallArg::Named(_, value)
            | CallArg::PositionalUnpack(value)
            | CallArg::KeywordUnpack(value) => self.reaches(value),
        })
    }
}

impl TypeChecker {
    /// Enter one callable body: a native borrowed result tracks what its returns are reached from. Returns the state
    /// to restore when the body is done.
    pub(super) fn enter_native_result_body(
        &mut self,
        callable: super::ownership_contracts::CallableSource<'_>,
    ) -> Option<NativeResultReach> {
        use super::ownership_contracts::CallableSource;
        let (parameters, result, source) = match callable {
            CallableSource::Function(function) => {
                (&function.params, &function.return_type, function.result_source.as_ref())
            }
            CallableSource::Method(method) => (&method.params, &method.return_type, method.result_source.as_ref()),
        };
        let next = self
            .result_route(parameters, result, source)
            .native_source
            .map(|source| NativeResultReach {
                source: source.node.binding.node.clone(),
                locals: HashMap::new(),
            });
        std::mem::replace(&mut self.native_result_reach, next)
    }

    /// A local bound to a call of a function whose result is the native reference holds that result as a loan of the
    /// input it borrows from, so its body's loan conflicts are reported outside the whole-body proof.
    pub(super) fn note_held_borrowed_result(&mut self, value: &Spanned<Expr>) {
        let Expr::Call(callee, _, _) = &value.node else {
            return;
        };
        let Expr::Ident(name) = &callee.node else {
            return;
        };
        let borrowed = self.lookup_symbol(name).is_some_and(|symbol| {
            matches!(&symbol.kind, SymbolKind::Function(function)
                if matches!(function.return_type, ResolvedType::Ref(_) | ResolvedType::RefMut(_)))
        });
        if borrowed {
            self.note_native_borrow_body();
        }
    }

    /// Record whether a local bound in a native borrowed result's body is reached from its source.
    pub(super) fn note_native_result_local(&mut self, assignment: &AssignmentStmt) {
        if let Some(reach) = self.native_result_reach.as_mut() {
            let reached = reach.reaches(&assignment.value);
            reach
                .locals
                .entry(assignment.name.clone())
                .and_modify(|known| *known &= reached)
                .or_insert(reached);
        }
    }

    /// Refuse a native borrowed result's returned value that is not reached from its source, with one INCAN-T0124 in
    /// place of the type mismatch it would also be. Returns whether the value was refused.
    pub(super) fn refuse_unreached_native_result(&mut self, value: &Spanned<Expr>) -> bool {
        let Some(source) = self
            .native_result_reach
            .as_ref()
            .filter(|reach| !reach.reaches(value))
            .map(|reach| reach.source.clone())
        else {
            return false;
        };
        self.check_expr(value);
        self.errors.push(
            CompileError::type_error(
                format!("A borrowed result cannot borrow a value created in this body; it borrows from '{source}'"),
                value.span,
            )
            .with_primary_label(format!("not reached from '{source}'"))
            .with_hint(format!(
                "Return '{source}', a local borrow of a place in it, or a call's borrowed result taken from it"
            ))
            .with_stable_code("INCAN-T0124"),
        );
        true
    }
}

impl TypeChecker {
    /// RFC 164: in Incan source a borrow is spelled `Borrowed[T]` or `BorrowedMut[T]`; native references exist only in
    /// imported Rust signatures, which never reach a written annotation. Refuse each Rust reference spelling of one
    /// written annotation once, at the reference, with the Incan spelling as the fix. Only the checking pass reports,
    /// and it examines each annotation once, before any canonical respelling, by its root span.
    pub(super) fn refuse_rust_borrow_spellings(&mut self, written: &Spanned<Type>) {
        if self.validate_source_type_names && self.borrow_spellings_examined.insert(written.span) {
            self.refuse_rust_borrow_spelling_nodes(written);
        }
    }

    /// One node of [`Self::refuse_rust_borrow_spellings`].
    fn refuse_rust_borrow_spelling_nodes(&mut self, annotation: &Spanned<Type>) {
        match &annotation.node {
            Type::Ref(inner) | Type::RefMut(inner) => {
                let (kind, mode, rust) = if matches!(annotation.node, Type::RefMut(_)) {
                    ("An exclusive borrow", "BorrowedMut", "&mut ")
                } else {
                    ("A borrow", "Borrowed", "&")
                };
                let fix = format!("{mode}[{}]", inner.node);
                self.errors.push(
                    CompileError::type_error(
                        format!("{kind} is spelled `{fix}` in Incan, not `{rust}{}`", inner.node),
                        annotation.span,
                    )
                    .with_primary_label("Rust reference spelling")
                    .with_hint(format!("Write `{fix}`")),
                );
                self.refuse_rust_borrow_spelling_nodes(inner);
            }
            Type::Generic(_, arguments) | Type::DottedGeneric(_, arguments) | Type::Tuple(arguments) => {
                for argument in arguments {
                    self.refuse_rust_borrow_spelling_nodes(argument);
                }
            }
            Type::Function(parameters, result) => {
                for parameter in parameters {
                    self.refuse_rust_borrow_spelling_nodes(parameter);
                }
                self.refuse_rust_borrow_spelling_nodes(result);
            }
            Type::MutParam(inner) => self.refuse_rust_borrow_spelling_nodes(inner),
            _ => {}
        }
    }
}

impl TypeChecker {
    /// The parameter and result annotations a free function's checked signature resolves (RFC 164): each borrow as
    /// the signature rule respells it and the result as its route does. Metadata that resolves written annotations
    /// reads these, so it publishes a borrow the same way in both spellings.
    pub(crate) fn canonical_function_annotations<'a>(
        &self,
        function: &'a FunctionDecl,
    ) -> (Vec<Cow<'a, Spanned<Type>>>, Cow<'a, Spanned<Type>>) {
        let route = self.result_route(&function.params, &function.return_type, function.result_source.as_ref());
        let explicit = self.signature_keeps_explicit_borrows(&function.params, &route.result, route.source);
        let parameters = function
            .params
            .iter()
            .map(|parameter| self.parameter_annotation(parameter, explicit))
            .collect();
        (parameters, route.result)
    }
}
