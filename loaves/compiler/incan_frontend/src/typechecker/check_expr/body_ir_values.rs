//! Preserve checked executable value contracts without changing legacy inference or lexical symbol types.

use crate::ast::{AssignmentStmt, CallArg, DictEntry, Expr, ListEntry, Literal, Pattern, Span, Spanned};
use crate::symbols::{CallableParam, ResolvedType, canonical_builtin_member_identity};
use incan_lang::lang::surface::result_methods::{self, ResultMethodId};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_semantics_core::{CanonicalSymbolId, body_ir::nominal_carrier_key};

use super::{TypeChecker, fill_open_result_parts};

impl TypeChecker {
    /// Retain nominal leaves of a selected function's declared result before consumer names can replace its scope.
    /// The original template protects caller-owned substitutions; only declaration-owned nominal positions receive
    /// the declaring scope's identities before remaining call-site arguments are applied.
    pub(in crate::typechecker::check_expr) fn retain_declared_nominal_result(
        &mut self,
        span: Span,
        declared: &ResolvedType,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        declaration: Option<&CanonicalSymbolId>,
    ) {
        let Some(qualified) = self.declared_nominal_value(declared, bindings, declaration) else {
            return;
        };
        self.type_info
            .calls
            .body_ir_call_result_types
            .entry((span.start, span.end))
            .or_insert(qualified);
    }

    /// Preserve a selected stored field's declaring nominal leaves before the consumer's type table is projected.
    pub(in crate::typechecker::check_expr) fn retain_declared_nominal_field(
        &mut self,
        span: Span,
        declared: &ResolvedType,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        declaration: Option<&CanonicalSymbolId>,
    ) {
        let Some(qualified) = self.declared_nominal_value(declared, bindings, declaration) else {
            return;
        };
        self.type_info
            .expressions
            .body_ir_expr_types
            .insert((span.start, span.end), qualified);
    }

    /// Apply only declaration-owned nominal positions, leaving caller binder substitutions under their own owners.
    fn declared_nominal_value(
        &self,
        declared: &ResolvedType,
        bindings: &std::collections::HashMap<String, ResolvedType>,
        declaration: Option<&CanonicalSymbolId>,
    ) -> Option<ResolvedType> {
        declaration?;
        let unbound = std::collections::BTreeMap::new();
        let scope = declaration
            .and_then(|identity| self.declaration_nominal_scopes.get(identity))
            .unwrap_or(&unbound);
        let mut qualified = declared.clone();
        let template = declaration
            .and_then(|identity| self.declaration_callables.get(identity))
            .map(|method| &method.return_type)
            .unwrap_or(declared);
        qualify_contextual_nominals(&mut qualified, template, scope);
        if qualified == *declared {
            return None;
        }
        let qualified = crate::resolved_type_subst::substitute_resolved_type(&qualified, bindings);
        Self::is_settled_type_argument(&qualified).then_some(qualified)
    }

    /// Keep the accepted parameter binding metadata while reading each type template from the canonical declaration.
    /// Original labels identify slots within that selected signature; missing template evidence grants no owner.
    pub(in crate::typechecker::check_expr) fn contextual_parameter_templates(
        &self,
        params: &[CallableParam],
        declaration: Option<&CanonicalSymbolId>,
        fallback: &[CallableParam],
    ) -> Vec<CallableParam> {
        let original = declaration
            .and_then(|identity| self.declaration_callables.get(identity))
            .map(|signature| signature.params.as_slice())
            .unwrap_or(fallback);
        params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let template = match &param.name {
                    Some(name) => original.iter().find(|candidate| candidate.name.as_ref() == Some(name)),
                    None => original.get(index),
                };
                CallableParam {
                    ty: template
                        .map(|template| template.ty.clone())
                        .unwrap_or(ResolvedType::Unknown),
                    ..param.clone()
                }
            })
            .collect()
    }

    /// Give a literal aggregate or `None` the selected declaration's expected owners while checking has its scope.
    /// Aggregate elements keep their independently checked owners; contextual storage never substitutes their values.
    pub(in crate::typechecker::check_expr) fn retain_contextual_literal_type(
        &mut self,
        expression: &Spanned<Expr>,
        expected: &ResolvedType,
        declaration: Option<&CanonicalSymbolId>,
        template: Option<&ResolvedType>,
    ) {
        if declaration.is_none() || !Self::is_settled_type_argument(expected) {
            return;
        }
        if let Expr::Paren(inner) = &expression.node {
            self.retain_contextual_literal_type(inner, expected, declaration, template);
            if let Some(ty) = self
                .type_info
                .expressions
                .body_ir_expr_types
                .get(&(inner.span.start, inner.span.end))
                .cloned()
            {
                self.type_info
                    .expressions
                    .body_ir_expr_types
                    .insert((expression.span.start, expression.span.end), ty);
            }
            return;
        }
        let observed = self
            .type_info
            .expr_type(expression.span)
            .cloned()
            .unwrap_or_else(|| expected.clone());
        let contextual = if matches!(expression.node, Expr::Literal(Literal::None)) {
            expected.clone()
        } else {
            self.literal_storage_template(&observed, expected)
                .unwrap_or(&observed)
                .clone()
        };
        let template = template.map(|template| {
            self.literal_storage_template(&contextual, template)
                .unwrap_or(template)
                .clone()
        });
        self.retain_contextual_literal_members(expression, &contextual, declaration, template.as_ref());
        if !matches!(
            &expression.node,
            Expr::List(_) | Expr::Dict(_) | Expr::Set(_) | Expr::Tuple(_)
        ) && !matches!(&expression.node, Expr::Literal(Literal::None) if expected.option_inner_type().is_some())
        {
            return;
        }
        let unbound = std::collections::BTreeMap::new();
        let scope = declaration
            .and_then(|identity| self.declaration_nominal_scopes.get(identity))
            .unwrap_or(&unbound);
        let mut ty = contextual.clone();
        qualify_contextual_nominals(&mut ty, template.as_ref().unwrap_or(&contextual), scope);
        self.type_info
            .expressions
            .body_ir_expr_types
            .insert((expression.span.start, expression.span.end), ty);
    }

    /// Apply a selected aggregate's context only to its written members. Nonempty members retain their own checked
    /// owners, while nested empty values receive the corresponding declared element or entry type.
    fn retain_contextual_literal_members(
        &mut self,
        expression: &Spanned<Expr>,
        expected: &ResolvedType,
        declaration: Option<&CanonicalSymbolId>,
        template: Option<&ResolvedType>,
    ) {
        match (&expression.node, expected) {
            (Expr::List(entries), ResolvedType::Generic(_, parts)) if !entries.is_empty() && parts.len() == 1 => {
                for entry in entries {
                    match entry {
                        ListEntry::Element(value) => self.retain_contextual_literal_type(
                            value,
                            &parts[0],
                            declaration,
                            template.and_then(|ty| contextual_type_part(ty, 0)).or(template),
                        ),
                        ListEntry::Spread(value) => {
                            self.retain_contextual_literal_type(value, expected, declaration, template)
                        }
                    }
                }
            }
            (Expr::Dict(entries), ResolvedType::Generic(_, parts)) if !entries.is_empty() && parts.len() == 2 => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.retain_contextual_literal_type(
                                key,
                                &parts[0],
                                declaration,
                                template.and_then(|ty| contextual_type_part(ty, 0)).or(template),
                            );
                            self.retain_contextual_literal_type(
                                value,
                                &parts[1],
                                declaration,
                                template.and_then(|ty| contextual_type_part(ty, 1)).or(template),
                            );
                        }
                        DictEntry::Spread(value) => {
                            self.retain_contextual_literal_type(value, expected, declaration, template)
                        }
                    }
                }
            }
            (Expr::Tuple(entries), ResolvedType::Tuple(parts)) if entries.len() == parts.len() => {
                for (index, (value, expected)) in entries.iter().zip(parts).enumerate() {
                    self.retain_contextual_literal_type(
                        value,
                        expected,
                        declaration,
                        template.and_then(|ty| contextual_type_part(ty, index)).or(template),
                    );
                }
            }
            _ => {}
        }
    }

    /// Read an additive executable contract before the legacy contextual snapshot.
    pub(in crate::typechecker) fn body_ir_checked_value_type(&self, span: Span) -> Option<&ResolvedType> {
        self.type_info
            .expressions
            .body_ir_expr_types
            .get(&(span.start, span.end))
            .or_else(|| {
                self.type_info
                    .calls
                    .body_ir_constructor_types
                    .get(&(span.start, span.end))
            })
            .or_else(|| {
                self.type_info
                    .calls
                    .body_ir_call_result_types
                    .get(&(span.start, span.end))
            })
            .or_else(|| self.type_info.expr_type(span))
    }

    /// Fill only holes in the checked callback return from its checked body; fixed channels are never replaced.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_closure_type(
        &mut self,
        span: Span,
        parameters: &[CallableParam],
        returned: &ResolvedType,
        actual: &ResolvedType,
    ) {
        let mut returned = returned.clone();
        if !self.types_compatible(&returned, actual) {
            return;
        }
        fill_open_result_parts(&mut returned, actual, false);
        self.type_info.expressions.body_ir_expr_types.insert(
            (span.start, span.end),
            ResolvedType::Function(parameters.to_vec(), Box::new(returned)),
        );
    }

    /// Preserve an initializer's executable carrier under its canonical lexical binding identity.
    pub(in crate::typechecker) fn retain_body_ir_assignment_value(&mut self, assignment: &AssignmentStmt) {
        self.bind_open_value(assignment.value.span, assignment.name_span, &assignment.name);
        self.retain_body_ir_empty_list_initializer(assignment);
        let Some(actual) = self.body_ir_checked_value_type(assignment.value.span).cloned() else {
            return;
        };
        let Some(identity) = self
            .type_info
            .resolved_write_identity(assignment.name_span, &assignment.name)
            .cloned()
        else {
            return;
        };
        self.type_info
            .expressions
            .body_ir_binding_types
            .insert(identity, actual);
    }

    /// Propagate exact lexical reads, checked nominal members, canonical Result contracts and comprehension elements.
    /// Incompatible constraints and members without a checked declaration publish no fact.
    /// Exact lexical reads and projections preserve the selected nominal leaves rather than source-facing names.
    pub(in crate::typechecker) fn retain_body_ir_result_value(
        &mut self,
        expression: &Spanned<Expr>,
        recorded: &ResolvedType,
    ) {
        if matches!(expression.node, Expr::MethodCall(..))
            && self.expr_is_async_call_realization(expression)
            && let Some(identity) = self.type_info.resolved_identity(expression.span).cloned()
            && identity.kind == incan_semantics_core::SemanticSourceTargetKind::Method
        {
            self.type_info
                .calls
                .body_ir_async_call_targets
                .insert((expression.span.start, expression.span.end), identity);
        }
        self.retain_open_value_expression(expression);
        if self
            .type_info
            .calls
            .body_ir_factory_result_conflicts
            .contains(&(expression.span.start, expression.span.end))
        {
            return;
        }
        if let Expr::MethodCall(receiver, method, _, arguments) = &expression.node
            && let Some(method_id) = result_methods::from_str(method)
            && self.type_info.resolved_identity(expression.span)
                == Some(&canonical_builtin_member_identity("Result", method))
        {
            self.retain_body_ir_result_callback(receiver, method_id, arguments);
        }
        if matches!(expression.node, Expr::Call(_, _, _) | Expr::MethodCall(_, _, _, _))
            && let Some(actual) = self
                .type_info
                .calls
                .body_ir_call_result_types
                .get(&(expression.span.start, expression.span.end))
                .cloned()
        {
            self.type_info
                .expressions
                .body_ir_expr_types
                .insert((expression.span.start, expression.span.end), actual);
            return;
        }
        let actual = match &expression.node {
            Expr::Ident(_) => self
                .type_info
                .resolved_identity(expression.span)
                .and_then(|identity| self.type_info.expressions.body_ir_binding_types.get(identity))
                .cloned(),
            Expr::Paren(inner) => self.body_ir_checked_value_type(inner.span).cloned(),
            Expr::List(entries) => self.body_ir_list_value_type(entries, recorded),
            Expr::Try(inner) => self
                .body_ir_checked_value_type(inner.span)
                .and_then(ResolvedType::result_ok_type)
                .cloned(),
            Expr::ListComp(comprehension) => self
                .body_ir_checked_value_type(comprehension.expr.span)
                .map(|element| ResolvedType::Generic("List".into(), vec![element.clone()])),
            Expr::MethodCall(receiver, method, _, arguments) => {
                if let Some(actual) = self.body_ir_nominal_method_type(receiver, method, arguments, expression.span) {
                    self.type_info
                        .expressions
                        .body_ir_expr_types
                        .insert((expression.span.start, expression.span.end), actual);
                    return;
                }
                let Some(method_id) = result_methods::from_str(method) else {
                    return;
                };
                if self.type_info.resolved_identity(expression.span)
                    != Some(&canonical_builtin_member_identity("Result", method))
                {
                    return;
                }
                self.body_ir_result_method_type(receiver, method_id, arguments)
            }
            Expr::Call(_, _, _) | Expr::Constructor(_, _) => self
                .type_info
                .calls
                .body_ir_call_result_types
                .get(&(expression.span.start, expression.span.end))
                .or_else(|| {
                    self.type_info
                        .calls
                        .body_ir_constructor_types
                        .get(&(expression.span.start, expression.span.end))
                })
                .cloned(),
            Expr::Field(receiver, field) => self.body_ir_nominal_field_type(receiver, field),
            Expr::Index(receiver, index) => self.body_ir_indexed_value_type(receiver, index),
            _ => None,
        };
        let Some(actual) = actual else {
            return;
        };
        // Canonical lexical reads and projections keep the selected owner's nominal leaves.
        if Self::is_settled_type_argument(&actual)
            && (matches!(expression.node, Expr::Field(_, _) | Expr::Index(_, _))
                || matches!(expression.node, Expr::Ident(_)) && same_retained_value_shape(self, recorded, &actual))
        {
            self.type_info
                .expressions
                .body_ir_expr_types
                .insert((expression.span.start, expression.span.end), actual);
            return;
        }
        let closed = self.body_ir_close_value_parts(recorded, &actual);
        if !self.types_compatible(&closed, &actual) {
            return;
        }
        self.type_info
            .expressions
            .body_ir_expr_types
            .insert((expression.span.start, expression.span.end), closed);
    }

    /// Keep a builtin Some payload's already checked owner without changing contextual or still-open storage.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_some_payload(
        &mut self,
        span: Span,
        args: &[CallArg],
        inner: &ResolvedType,
    ) {
        let [CallArg::Positional(value)] = args else { return };
        let Some(actual) = self.body_ir_checked_value_type(value.span) else {
            return;
        };
        let closed = self.body_ir_close_value_parts(inner, actual);
        if closed == *inner || !Self::is_settled_type_argument(&closed) || !self.types_compatible(&closed, actual) {
            return;
        }
        self.type_info.calls.body_ir_call_result_types.insert(
            (span.start, span.end),
            ResolvedType::Generic("Option".to_owned(), vec![closed]),
        );
    }

    /// Pass only a registry-selected Result channel's checked owner into its inline closure parameter facts.
    fn retain_body_ir_result_callback(&mut self, receiver: &Spanned<Expr>, method: ResultMethodId, args: &[CallArg]) {
        let Some(receiver) = self.body_ir_checked_value_type(receiver.span) else {
            return;
        };
        let input = match method {
            ResultMethodId::Map | ResultMethodId::AndThen | ResultMethodId::Inspect => receiver.result_ok_type(),
            ResultMethodId::MapErr | ResultMethodId::OrElse | ResultMethodId::InspectErr => receiver.result_err_type(),
            _ => None,
        }
        .cloned();
        if let (Some(input), [CallArg::Positional(callback)]) = (input, args) {
            self.retain_body_ir_callback_parameter(callback, &input);
        }
    }

    /// Fill compatible holes and retain proven nominal owners from a checked value contract. Concrete scalar leaves
    /// and caller-owned binders keep their contextual storage; nominal leaves keep their selected declaration.
    pub(in crate::typechecker::check_expr) fn body_ir_close_value_parts(
        &self,
        recorded: &ResolvedType,
        actual: &ResolvedType,
    ) -> ResolvedType {
        match (recorded, actual) {
            (ResolvedType::Named(name), ResolvedType::Named(other)) => self
                .body_ir_composed_nominal_name(name, other)
                .map(ResolvedType::Named)
                .unwrap_or_else(|| recorded.clone()),
            (ResolvedType::Unknown | ResolvedType::CallSiteInfer, _) => actual.clone(),
            (ResolvedType::TypeVar(_), _) if self.active_type_param_name(recorded).is_none() => actual.clone(),
            (ResolvedType::Generic(name, parts), ResolvedType::Generic(other, values))
                if parts.len() == values.len()
                    && (name == other
                        || self.public_library_type_identities_compatible(recorded, actual) == Some(true)
                        || self.body_ir_composed_nominal_name(name, other).is_some()) =>
            {
                ResolvedType::Generic(
                    self.body_ir_composed_nominal_name(name, other)
                        .unwrap_or_else(|| name.clone()),
                    parts
                        .iter()
                        .zip(values)
                        .map(|(part, value)| self.body_ir_close_value_parts(part, value))
                        .collect(),
                )
            }
            (ResolvedType::Tuple(parts), ResolvedType::Tuple(values)) if parts.len() == values.len() => {
                ResolvedType::Tuple(
                    parts
                        .iter()
                        .zip(values)
                        .map(|(part, value)| self.body_ir_close_value_parts(part, value))
                        .collect(),
                )
            }
            (ResolvedType::Function(params, result), ResolvedType::Function(values, output))
                if params.len() == values.len() =>
            {
                let params = params
                    .iter()
                    .zip(values)
                    .map(|(param, value)| CallableParam {
                        ty: self.body_ir_close_value_parts(&param.ty, &value.ty),
                        ..param.clone()
                    })
                    .collect();
                ResolvedType::Function(params, Box::new(self.body_ir_close_value_parts(result, output)))
            }
            _ => recorded.clone(),
        }
    }

    /// Keep an already proven carrier key when the contextual leaf names that same declaration. An active different
    /// binding is a conflict; an unbound source spelling grants no authority beyond the value's selected owner.
    fn body_ir_composed_nominal_name(&self, contextual: &str, actual: &str) -> Option<String> {
        if contextual == actual {
            return Some(actual.to_owned());
        }
        let identities = self.checked_nominal_type_identities();
        let owner = identities.get(actual)?;
        if nominal_carrier_key(owner).as_deref() != Some(actual) {
            return None;
        }
        let compatible = identities
            .get(contextual)
            .map_or(contextual == owner.declaration_name, |identity| identity == owner);
        compatible.then(|| actual.to_owned())
    }

    /// Read one builtin list's already checked element without inferring a new collection or nominal owner.
    pub(in crate::typechecker) fn body_ir_list_element_type(value: &ResolvedType) -> Option<&ResolvedType> {
        match value {
            ResolvedType::Generic(name, parts)
                if collections::from_str(name) == Some(CollectionTypeId::List) && parts.len() == 1 =>
            {
                parts.first()
            }
            _ => None,
        }
    }

    /// Merge already checked element owners into a literal's storage without selecting between conflicting owners.
    fn body_ir_list_value_type(&self, entries: &[ListEntry], recorded: &ResolvedType) -> Option<ResolvedType> {
        let ResolvedType::Generic(name, _) = recorded else {
            return None;
        };
        let mut element = Self::body_ir_list_element_type(recorded)?.clone();
        for entry in entries {
            let actual = match entry {
                ListEntry::Element(value) => self.body_ir_checked_value_type(value.span)?,
                ListEntry::Spread(value) => {
                    Self::body_ir_list_element_type(self.body_ir_checked_value_type(value.span)?)?
                }
            };
            element = self.body_ir_close_value_parts(&element, actual);
            if !self.types_compatible(&element, actual) {
                return None;
            }
        }
        Self::is_settled_type_argument(&element).then(|| ResolvedType::Generic(name.clone(), vec![element]))
    }

    /// Refine a loop or comprehension binding only from its iterable's already checked list element owner.
    /// Equal lexical types need no override, so open contextual storage remains available to later checking.
    pub(in crate::typechecker) fn retain_body_ir_iteration_binding(
        &mut self,
        pattern: &Spanned<Pattern>,
        iterable: &Spanned<Expr>,
    ) {
        let Some(retained) = self.body_ir_checked_value_type(iterable.span) else {
            return;
        };
        let actual = self.infer_iterator_element_type(retained);
        if Self::is_settled_type_argument(&actual) {
            self.retain_body_ir_for_value(pattern, &actual);
            return;
        }
        let Some(recorded) = self.type_info.expr_type(pattern.span) else {
            return;
        };
        let closed = self.body_ir_close_value_parts(recorded, &actual);
        if closed != *recorded && self.types_compatible(&closed, &actual) {
            self.retain_body_ir_for_value(pattern, &closed);
        }
    }

    /// Retain list iteration's checked element under each loop binding before its body reads those bindings.
    /// The lexical symbol types stay unchanged; Body IR receives the collection value's already selected owners.
    pub(in crate::typechecker) fn retain_body_ir_for_value(
        &mut self,
        pattern: &Spanned<Pattern>,
        value: &ResolvedType,
    ) {
        self.type_info
            .expressions
            .body_ir_expr_types
            .insert((pattern.span.start, pattern.span.end), value.clone());
        match &pattern.node {
            Pattern::Binding(name) => {
                if let Some(identity) = self.type_info.resolved_write_identity(pattern.span, name).cloned() {
                    self.type_info
                        .expressions
                        .body_ir_binding_types
                        .insert(identity, value.clone());
                }
            }
            Pattern::Tuple(items) => {
                if let ResolvedType::Tuple(values) = value {
                    for (item, value) in items.iter().zip(values) {
                        self.retain_body_ir_for_value(item, value);
                    }
                }
            }
            _ => {}
        }
    }

    /// Retain a checked nominal result's annotated specialization when legacy leaves its constructor arguments open.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_value_destination(
        &mut self,
        expression: &Spanned<Expr>,
        recorded: &ResolvedType,
        expected: Option<&ResolvedType>,
    ) {
        if self
            .type_info
            .calls
            .body_ir_factory_result_conflicts
            .contains(&(expression.span.start, expression.span.end))
        {
            return;
        }
        let Some(expected) = expected.filter(|ty| Self::is_settled_type_argument(ty)) else {
            return;
        };
        let (ResolvedType::Generic(owner, _), ResolvedType::Generic(destination, _)) = (recorded, expected) else {
            return;
        };
        if owner != destination
            || self.type_info.resolved_identity(expression.span).is_none()
            || !self.types_compatible(recorded, expected)
        {
            return;
        }
        if self
            .body_ir_checked_value_type(expression.span)
            .is_some_and(|actual| !self.types_compatible(actual, expected))
        {
            self.refuse_body_ir_factory_result(expression.span);
            return;
        }
        let actual = self.body_ir_close_value_parts(recorded, expected);
        self.type_info
            .expressions
            .body_ir_expr_types
            .insert((expression.span.start, expression.span.end), actual);
    }

    /// The registry-selected member fixes which checked callback output replaces which Result channel.
    fn body_ir_result_method_type(
        &self,
        receiver: &Spanned<Expr>,
        method: ResultMethodId,
        arguments: &[CallArg],
    ) -> Option<ResolvedType> {
        let receiver = self.body_ir_checked_value_type(receiver.span)?;
        let ok = receiver.result_ok_type()?;
        let error = receiver.result_err_type()?;
        if matches!(method, ResultMethodId::Unwrap | ResultMethodId::UnwrapOr) {
            return Some(ok.clone());
        }
        if matches!(method, ResultMethodId::Inspect | ResultMethodId::InspectErr) {
            return Some(receiver.clone());
        }
        let [CallArg::Positional(callback)] = arguments else {
            return None;
        };
        let ResolvedType::Function(_, output) = self.body_ir_checked_value_type(callback.span)? else {
            return None;
        };
        let (ok, error) = match method {
            ResultMethodId::Map => (output.as_ref(), error),
            ResultMethodId::MapErr => (ok, output.as_ref()),
            ResultMethodId::AndThen | ResultMethodId::OrElse => return Some(output.as_ref().clone()),
            _ => return None,
        };
        Some(ResolvedType::Generic(
            "Result".to_owned(),
            vec![ok.clone(), error.clone()],
        ))
    }
}

/// Preserve nominal owners only across the same value shape; contextual wrapping, numeric conversions and narrowing
/// keep their checked result.
fn same_retained_value_shape(checker: &TypeChecker, recorded: &ResolvedType, actual: &ResolvedType) -> bool {
    match (recorded, actual) {
        (ResolvedType::Named(name), ResolvedType::Named(other)) => same_retained_nominal_head(checker, name, other),
        (ResolvedType::Generic(name, parts), ResolvedType::Generic(other, values)) => {
            same_retained_nominal_head(checker, name, other) && same_retained_value_parts(checker, parts, values)
        }
        (ResolvedType::Tuple(parts), ResolvedType::Tuple(values)) => same_retained_value_parts(checker, parts, values),
        (ResolvedType::Ref(inner), ResolvedType::Ref(other))
        | (ResolvedType::RefMut(inner), ResolvedType::RefMut(other)) => {
            same_retained_value_shape(checker, inner, other)
        }
        _ => recorded == actual,
    }
}

/// Require every structural position to retain its own shape before copying a binding's exact nominal leaves.
fn same_retained_value_parts(checker: &TypeChecker, recorded: &[ResolvedType], actual: &[ResolvedType]) -> bool {
    recorded.len() == actual.len()
        && recorded
            .iter()
            .zip(actual)
            .all(|(left, right)| same_retained_value_shape(checker, left, right))
}

/// Match a retained canonical carrier to its source declaration name without treating a newtype coercion as an
/// unchanged value.
fn same_retained_nominal_head(checker: &TypeChecker, recorded: &str, actual: &str) -> bool {
    if recorded == actual {
        return true;
    }
    let source_name = checker
        .symbols
        .module_type_identity(recorded)
        .map_or(recorded, |identity| identity.declaration_name.as_str());
    checker.declaration_nominal_scopes.keys().any(|identity| {
        identity.declaration_name == source_name && nominal_carrier_key(identity).as_deref() == Some(actual)
    })
}

/// Preserve expected nominal leaves through their declaration's retained bindings. This only projects a
/// checker-selected destination, never a value's independently owned type or an unproven caller binding.
fn qualify_contextual_nominals(
    ty: &mut ResolvedType,
    template: &ResolvedType,
    scope: &std::collections::BTreeMap<String, CanonicalSymbolId>,
) {
    match ty {
        ResolvedType::Named(name) => {
            let ResolvedType::Named(original) = template else {
                return;
            };
            if let Some(key) = scope.get(original).and_then(nominal_carrier_key) {
                *name = key;
            }
        }
        ResolvedType::Generic(_, parts) | ResolvedType::Tuple(parts) => {
            for (index, part) in parts.iter_mut().enumerate() {
                if let Some(original) = contextual_type_part(template, index) {
                    qualify_contextual_nominals(part, original, scope);
                }
            }
        }
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner)
        | ResolvedType::TypeToken(inner) => {
            if let Some(original) = contextual_type_part(template, 0) {
                qualify_contextual_nominals(inner, original, scope);
            }
        }
        ResolvedType::FrozenDict(key, value) => {
            if let Some(original) = contextual_type_part(template, 0) {
                qualify_contextual_nominals(key, original, scope);
            }
            if let Some(original) = contextual_type_part(template, 1) {
                qualify_contextual_nominals(value, original, scope);
            }
        }
        ResolvedType::Function(parameters, returned) => {
            let ResolvedType::Function(original_params, original_return) = template else {
                return;
            };
            for (parameter, original) in parameters.iter_mut().zip(original_params) {
                qualify_contextual_nominals(&mut parameter.ty, &original.ty, scope);
            }
            qualify_contextual_nominals(returned, original_return, scope);
        }
        _ => {}
    }
}

/// Read a structural template position without turning a declaration-owned type parameter into a nominal leaf.
fn contextual_type_part(ty: &ResolvedType, index: usize) -> Option<&ResolvedType> {
    match ty {
        ResolvedType::Generic(_, parts) | ResolvedType::Tuple(parts) => parts.get(index),
        ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner)
        | ResolvedType::TypeToken(inner)
            if index == 0 =>
        {
            Some(inner)
        }
        ResolvedType::FrozenDict(key, _) if index == 0 => Some(key),
        ResolvedType::FrozenDict(_, value) if index == 1 => Some(value),
        _ => None,
    }
}
