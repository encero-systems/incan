//! Refusal of a generic call that leaves one of its callee's type parameters undetermined (#1561, #2762).

use crate::typechecker::undetermined_type_params::{written_arguments, written_expression};

use super::*;

/// A checked generic call whose callee's type parameters are bound as far as the call's sources allow.
pub(super) struct OpenTypeParamCall<'call> {
    /// The function or method name as the diagnostic shows it.
    pub(super) callee: &'call str,
    /// Whether the callee is a method, whose written form follows its receiver.
    pub(super) method: bool,
    /// The callee's declaration, whose type parameter the diagnostic points at.
    pub(super) declaration: Option<&'call CanonicalSymbolId>,
    /// The callee's own type parameters, in declaration order.
    pub(super) type_params: &'call [String],
    /// The declared bounds of the callee's type parameters.
    pub(super) bound_details: &'call HashMap<String, Vec<crate::symbols::TypeBoundInfo>>,
    /// The declared parameters, with written type arguments substituted.
    pub(super) params: &'call [CallableParam],
    /// The declared result type, naming the callee's type parameters.
    pub(super) return_type: &'call ResolvedType,
    /// The call's written arguments.
    pub(super) args: &'call [CallArg],
    /// The call's instantiation once arguments, the expected type and `Ok`/`Err` sides are settled.
    pub(super) bindings: &'call HashMap<String, ResolvedType>,
    /// The call expression.
    pub(super) span: Span,
}

impl TypeChecker {
    /// Refuse a generic call that leaves one of its callee's type parameters undetermined (#1561, #2762).
    ///
    /// See [`Self::type_param_left_open`] for when a parameter is open. One error is reported per call, for its
    /// first open parameter.
    pub(super) fn refuse_type_params_left_open(&mut self, call: OpenTypeParamCall<'_>) {
        let arguments = Self::arguments_with_parameters(call.params, call.args);
        let open = call
            .type_params
            .iter()
            .filter(|type_param| self.type_param_left_open(&call, &arguments, type_param))
            .cloned()
            .collect::<Vec<_>>();
        let Some(first) = open.first() else {
            return;
        };
        let slots = call
            .type_params
            .iter()
            .map(|type_param| if open.contains(type_param) { "..." } else { "_" })
            .collect::<Vec<_>>()
            .join(", ");
        let receiver = if call.method { "." } else { "" };
        let written_args = written_arguments(call.args);
        let mut hint = format!(
            "Write the type argument, as in `{receiver}{}[{slots}]({written_args})`",
            call.callee
        );
        let named_by_value = arguments.iter().find_map(|(expression, param)| {
            let param = (*param)?;
            (first_open_type_param(&param.ty, std::slice::from_ref(first)).is_some()
                && !is_built_from_literals(&expression.node))
            .then_some((*expression, param))
        });
        if let Some((expression, param)) = named_by_value {
            hint.push_str(&format!(
                ", or bind the argument to an annotated name first, as in `{}: {} = {}`",
                param.name.as_deref().unwrap_or("value"),
                written_type(&param.ty, &open, call.bindings),
                written_expression(&expression.node).unwrap_or_else(|| "...".to_owned()),
            ));
        } else if first_open_type_param(call.return_type, std::slice::from_ref(first)).is_some() {
            hint.push_str(&format!(
                ", or bind the result to an annotated name, as in `{}: {} = {receiver}{}({written_args})`",
                self.binding_name_of(call.span).unwrap_or("result"),
                written_type(call.return_type, &open, call.bindings),
                call.callee,
            ));
        }
        let error = errors::type_param_not_determined(call.callee, first, "argument", call.span).with_hint(hint);
        let error = self.with_declared_type_param(error, call.declaration, first);
        self.errors.push(error);
    }

    /// Return whether the call leaves `type_param` undetermined.
    ///
    /// The call's instantiation must leave the parameter open: no binding, or only one naming a type parameter still
    /// to be inferred. Every argument passed for a parameter that names it must then leave it so: the `None` literal,
    /// or a value of a generic enum that carries no type arguments, such as the payload-free `Slot.Vacant` for a
    /// `Slot[T]` parameter. Any other argument may fix the parameter through facts the checker settles after this
    /// point, such as a nested constructor's exact type or a future's output, and is not judged here.
    ///
    /// A parameter with a bound on a trait the checker declares is also open when every argument for it is built from
    /// literals, or when no argument names it: `show(None)` for `def show[T with Display](o: Option[T])`, and `make()`
    /// for `def make[T with Display]() -> list[T]`. A parameter no argument names and that carries no such bound is
    /// left to later uses of the call's result, such as a decorator factory applied to its function.
    fn type_param_left_open(
        &self,
        call: &OpenTypeParamCall<'_>,
        arguments: &[(&Spanned<Expr>, Option<&CallableParam>)],
        type_param: &String,
    ) -> bool {
        if call
            .bindings
            .get(type_param)
            .is_some_and(|bound| !is_open_binding(bound, call.type_params))
        {
            return false;
        }
        let bounded = call.bound_details.get(type_param).is_some_and(|bounds| {
            bounds.iter().any(|bound| {
                !bound.inferred
                    && !is_rust_capability_bound(Self::type_bound_source_name(bound))
                    && self.lookup_trait_adoption_info(bound).is_some()
            })
        });
        let naming = arguments
            .iter()
            .filter(|(_, param)| {
                param.is_some_and(|param| first_open_type_param(&param.ty, std::slice::from_ref(type_param)).is_some())
            })
            .map(|(expression, _)| *expression)
            .collect::<Vec<_>>();
        if naming.is_empty() {
            return bounded;
        }
        naming.iter().all(|expression| {
            is_none_literal(&expression.node)
                || (bounded && is_built_from_literals(&expression.node))
                || self.is_unparameterized_generic_enum_value(expression)
        })
    }

    /// Return whether a value's checked type is a generic source enum named without type arguments: a payload-free
    /// variant such as `Slot.Vacant`, or a construction whose payloads and expected type fixed none of the enum's
    /// parameters.
    fn is_unparameterized_generic_enum_value(&self, expression: &Spanned<Expr>) -> bool {
        let Some(ResolvedType::Named(name)) = self.type_info.expr_type(expression.span) else {
            return false;
        };
        matches!(
            self.lookup_semantic_type_info(name),
            Some(TypeInfo::Enum(info)) if !info.type_params.is_empty()
        )
    }
}

/// Spell a declared type for a hint: `...` for each open type parameter, and the call's binding for the others.
fn written_type(declared: &ResolvedType, open: &[String], bindings: &HashMap<String, ResolvedType>) -> String {
    let mut written = bindings.clone();
    for type_param in open {
        written.insert(type_param.clone(), ResolvedType::Named("...".to_owned()));
    }
    substitute_resolved_type(declared, &written).to_string()
}
