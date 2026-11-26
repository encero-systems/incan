//! Refusal of a call or construction that leaves one of its type parameters undetermined (#1561, #2762).
//!
//! A type parameter is determined by a written type argument, by the value arguments, fields or payloads, by the
//! expected type, or, for the open side of an `Ok(...)` or `Err(...)` argument, by the enclosing function's `Result`
//! side. The checker never chooses a type for one that none of these fixes: the call or construction is refused
//! (`INCAN-T0001`) at its own span, with the declared type parameter as a related location and a hint that spells the
//! written form. The call refusal lives with generic call checking; this module holds the construction refusal and the
//! location and spelling both share.

use std::collections::HashMap;

use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_semantics_core::CanonicalSymbolId;

use super::TypeChecker;
use crate::ast::{CallArg, Declaration, Expr, Literal, MethodDecl, Program, Span, Spanned, TypeParam};
use crate::diagnostics::{CompileError, errors};
use crate::symbols::ResolvedType;

/// The longest argument spelling a hint repeats; a longer one is written `...`.
const WRITTEN_EXPRESSION_LIMIT: usize = 60;

/// A generic model, class or enum construction that its fields or payloads, written type arguments and expected
/// type leave partly open.
pub(in crate::typechecker) struct OpenConstruction<'construction> {
    /// The type as the source spells it at the construction (`RecordId`, `RecordState`).
    pub(in crate::typechecker) owner: &'construction str,
    /// The type's declaration, whose type parameter the diagnostic points at.
    pub(in crate::typechecker) declaration: Option<CanonicalSymbolId>,
    /// The type's parameters, in declaration order.
    pub(in crate::typechecker) type_params: &'construction [String],
    /// The parameters the construction fixed; every other parameter is open.
    pub(in crate::typechecker) fixed: HashMap<String, ResolvedType>,
    /// The written construction after the type: `(serial=1)`, or `.Deleted(Tombstone(serial=1))`.
    pub(in crate::typechecker) written_rest: String,
    /// What fixes the type's parameters at such a construction, for the message: `field` or `payload`.
    pub(in crate::typechecker) sources: &'construction str,
    /// The construction expression.
    pub(in crate::typechecker) span: Span,
}

impl TypeChecker {
    /// Record where each generic declaration of the program declares its type parameters, keyed by the declaration
    /// span its canonical identity carries, so a refusal can point at the parameter it names.
    pub(super) fn record_type_param_declaration_spans(&mut self, program: &Program) {
        self.type_param_declaration_spans.clear();
        for declaration in &program.declarations {
            let (type_params, methods): (&[TypeParam], &[Spanned<MethodDecl>]) = match &declaration.node {
                Declaration::Function(function) => (&function.type_params, &[]),
                Declaration::Model(owner) => (&owner.type_params, &owner.methods),
                Declaration::Class(owner) => (&owner.type_params, &owner.methods),
                Declaration::Enum(owner) => (&owner.type_params, &owner.methods),
                Declaration::Newtype(owner) => (&owner.type_params, &owner.methods),
                Declaration::Trait(owner) => (&owner.type_params, &owner.methods),
                _ => continue,
            };
            let owners = std::iter::once((declaration.span, type_params)).chain(
                methods
                    .iter()
                    .map(|method| (method.span, method.node.type_params.as_slice())),
            );
            for (span, type_params) in owners {
                if !type_params.is_empty() {
                    let binders = type_params
                        .iter()
                        .map(|param| (param.name.clone(), param.span))
                        .collect();
                    self.type_param_declaration_spans.insert(span, binders);
                }
            }
        }
    }

    /// Refuse a generic model, class or enum construction that leaves one of the type's parameters undetermined,
    /// naming its first open parameter.
    pub(in crate::typechecker) fn refuse_undetermined_construction(&mut self, construction: OpenConstruction<'_>) {
        let Some(first) = construction
            .type_params
            .iter()
            .find(|type_param| !construction.fixed.contains_key(*type_param))
        else {
            return;
        };
        let arguments = construction
            .type_params
            .iter()
            .map(|type_param| match construction.fixed.get(type_param) {
                Some(fixed) => fixed.to_string(),
                None => "...".to_owned(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let written_type = format!("{}[{arguments}]", construction.owner);
        let mut hint = format!(
            "Write the type argument, as in `{written_type}{}`",
            construction.written_rest
        );
        if let Some(name) = self.binding_name_of(construction.span) {
            hint.push_str(&format!(
                ", or annotate the name it is bound to, as in `{name}: {written_type} = {}{}`",
                construction.owner, construction.written_rest
            ));
        }
        let error =
            errors::type_param_not_determined(construction.owner, first, construction.sources, construction.span)
                .with_hint(hint);
        let error = self.with_declared_type_param(error, construction.declaration.as_ref(), first);
        self.errors.push(error);
    }

    /// Return whether a construction's expected type names the constructed declaration anywhere, so that a generic
    /// caller or an enclosing construction can still fix its parameters through it.
    pub(in crate::typechecker) fn expected_type_names_declaration(
        &self,
        expected: Option<&ResolvedType>,
        name: &str,
    ) -> bool {
        expected.is_some_and(|expected| self.type_names_declaration(expected, name))
    }

    /// Return whether `ty` names the declaration `name` spells, at any depth.
    fn type_names_declaration(&self, ty: &ResolvedType, name: &str) -> bool {
        match ty {
            ResolvedType::Named(named) => self.source_type_names_same_declaration(named, name),
            ResolvedType::Generic(named, arguments) => {
                self.source_type_names_same_declaration(named, name)
                    || arguments
                        .iter()
                        .any(|argument| self.type_names_declaration(argument, name))
            }
            ResolvedType::Tuple(items) => items.iter().any(|item| self.type_names_declaration(item, name)),
            ResolvedType::FrozenList(inner)
            | ResolvedType::FrozenSet(inner)
            | ResolvedType::TypeToken(inner)
            | ResolvedType::Ref(inner)
            | ResolvedType::RefMut(inner) => self.type_names_declaration(inner, name),
            ResolvedType::FrozenDict(key, value) => {
                self.type_names_declaration(key, name) || self.type_names_declaration(value, name)
            }
            ResolvedType::Function(params, returned) => {
                params.iter().any(|param| self.type_names_declaration(&param.ty, name))
                    || self.type_names_declaration(returned, name)
            }
            _ => false,
        }
    }

    /// Point a refusal at the declared type parameter: a related location in this module's source, or the declaration
    /// itself when another module declares it.
    pub(in crate::typechecker) fn with_declared_type_param(
        &self,
        error: CompileError,
        declaration: Option<&CanonicalSymbolId>,
        type_param: &str,
    ) -> CompileError {
        let Some(declaration) = declaration else {
            return error;
        };
        let declared_here = (declaration.origin == self.symbols.declaration_origin())
            .then(|| self.type_param_declaration_spans.get(&declaration.declaration_span))
            .flatten()
            .and_then(|binders| binders.iter().find(|(name, _)| name == type_param))
            .map(|(_, span)| *span);
        match declared_here {
            Some(span) => error.with_related_span(span, format!("'{type_param}' is declared here")),
            None => error.with_related_declaration(
                declaration.clone(),
                format!("declaration of `{}`", declaration.declaration_name),
            ),
        }
    }

    /// The name an unannotated assignment binds the value at `span` to, when the value is that whole expression.
    pub(in crate::typechecker) fn binding_name_of(&self, span: Span) -> Option<&str> {
        self.unannotated_binding_value
            .as_ref()
            .filter(|(value, _)| *value == span)
            .map(|(_, name)| name.as_str())
    }
}

/// Spell a call's arguments for a hint, each as the source wrote it when it is short and plain, else `...`.
pub(in crate::typechecker) fn written_arguments(args: &[CallArg]) -> String {
    args.iter()
        .map(|argument| match argument {
            CallArg::Positional(value) => written_expression(&value.node).unwrap_or_else(|| "...".to_owned()),
            CallArg::Named(name, value) => format!(
                "{}={}",
                name.node,
                written_expression(&value.node).unwrap_or_else(|| "...".to_owned())
            ),
            CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => "...".to_owned(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Spell an expression for a hint when it is a name, a path, a simple literal or a call of those, within
/// [`WRITTEN_EXPRESSION_LIMIT`] characters.
pub(in crate::typechecker) fn written_expression(expression: &Expr) -> Option<String> {
    let written = match expression {
        Expr::Ident(name) => name.clone(),
        Expr::SelfExpr => keywords::as_str(KeywordId::SelfKw).to_owned(),
        Expr::Literal(Literal::None) => constructors::as_str(ConstructorId::None).to_owned(),
        Expr::Literal(Literal::Bool(value)) => value.to_string(),
        Expr::Literal(Literal::Int(value)) => value.repr.clone(),
        Expr::Literal(Literal::String(text))
            if text
                .chars()
                .all(|character| character.is_alphanumeric() || " _-./:".contains(character)) =>
        {
            format!("\"{text}\"")
        }
        Expr::Paren(inner) => format!("({})", written_expression(&inner.node)?),
        Expr::Field(base, field) => format!("{}.{field}", written_expression(&base.node)?),
        Expr::Call(callee, type_args, args) if type_args.is_empty() => {
            format!("{}({})", written_expression(&callee.node)?, written_arguments(args))
        }
        Expr::Constructor(name, args) => format!("{name}({})", written_arguments(args)),
        Expr::MethodCall(base, method, type_args, args) if type_args.is_empty() => {
            format!(
                "{}.{method}({})",
                written_expression(&base.node)?,
                written_arguments(args)
            )
        }
        Expr::List(entries) if entries.is_empty() => "[]".to_owned(),
        _ => return None,
    };
    (written.len() <= WRITTEN_EXPRESSION_LIMIT).then_some(written)
}
