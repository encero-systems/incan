//! Call-argument planning: the declared slot surface, spread expansion, and binding a written argument list to it.

use super::*;

/// One resolved direct-call declaration narrowed to the executor-relevant facts.
///
/// `direct_call_id` is present only for a declaration physically represented by this module. Keeping the target
/// separate from its parameter slots prevents a future consumer from treating a successfully planned argument list
/// as proof that an imported callable is executable here.
///
/// `canonical` answers a deliberately different question: *which declaration* this call selected, in a form that
/// survives an import or a rename. An imported callable therefore has no `direct_call_id` but may still have a
/// canonical identity, and the two must not be read as substitutes for one another.
pub(super) struct DirectCallDeclaration {
    pub(super) slots: Option<Vec<DeclaredSlot>>,
    pub(super) direct_call_id: Option<CompilerNodeId>,
    pub(super) builtin: Option<incan_core::lang::builtins::BuiltinFnId>,
    pub(super) canonical: Option<CanonicalSymbolId>,
}
/// One declared callable parameter or nominal field, reduced to the facts call-site binding actually needs.
///
/// Direct functions, methods, local callables, and nominal constructors each carry their declared surface in a
/// different type (`IncanCallableParam`, `symbols::CallableParam`, a field layout). Binding them through one planner
/// is what keeps #1158's "one mechanism" contract honest, so each caller narrows its own declaration surface to this
/// shape first rather than getting its own copy of the binding rules.
pub(super) struct DeclaredSlot {
    /// Declared name, when the slot can be supplied by name. Positional-only slots carry `None`.
    pub(super) name: Option<String>,
    /// Whether omitting this slot is legal because the declaration supplies a default.
    pub(super) has_default: bool,
    /// Whether this slot holds a partial's construction-time preset, which positional binding skips.
    pub(super) is_partial_preset: bool,
    /// Whether this slot is a `*args`/`**kwargs` rest parameter, which this planner refuses.
    pub(super) is_rest: bool,
}
impl DeclaredSlot {
    /// Narrow a semantic callable parameter (a local callable value's signature) to its binding-relevant facts.
    pub(super) fn from_semantic_param(param: &IncanCallableParam) -> Self {
        Self {
            name: param.name.clone(),
            has_default: param.has_default,
            is_partial_preset: param.is_partial_preset,
            is_rest: param.kind != IncanCallableParamKind::Normal,
        }
    }

    /// Narrow a typechecker-resolved source callable parameter to its binding-relevant facts.
    pub(super) fn from_checked_param(param: &CallableParam) -> Self {
        Self {
            name: param.name.clone(),
            has_default: param.has_default,
            is_partial_preset: param.is_partial_preset,
            is_rest: param.kind != ast::ParamKind::Normal,
        }
    }
}
/// Expand a statically shaped spread argument into the ordinary arguments it stands for.
///
/// The typechecker proves a spread's shape when its operand is written as a literal whose arity is visible before
/// lowering -- `f(*(1, 2))`, `f(**{"a": 1})` -- and records the result as a
/// [`FixedUnpackPlan`](crate::frontend::typechecker::FixedUnpackPlan). Those calls have a perfectly ordinary fixed
/// arity, so they bind through the same declaration-slot planner as any other call rather than being pushed onto
/// the runtime-arity path; a `*(1, 2)` against `def add(a, b)` really is `add(1, 2)`.
///
/// Returns `None` when the spread has no proven shape, which is the ordinary case (`f(*xs)` for a list variable):
/// its arity is a runtime fact and it belongs on the unresolved-arity path. Also returns `None` when a plan exists
/// but the operand is not a destructurable literal -- the plan is recorded for tuple-*typed* operands too, and
/// those have no written elements to expand.
///
/// Parentheses are transparent here exactly as they are for the typechecker's own shape check, so the two stages
/// agree on which spellings count as shaped.
pub(super) fn expand_shaped_spread(type_info: &TypeCheckInfo, arg: &ast::CallArg) -> Option<Vec<ast::CallArg>> {
    /// Look through any number of parenthesis layers to the expression they wrap.
    ///
    /// The typechecker's own shape check treats parentheses as transparent, so this must too, or the two stages
    /// would disagree about which spellings count as statically shaped.
    pub(super) fn unparenthesized(expr: &ast::Spanned<ast::Expr>) -> &ast::Spanned<ast::Expr> {
        match &expr.node {
            ast::Expr::Paren(inner) => unparenthesized(inner),
            _ => expr,
        }
    }

    match arg {
        ast::CallArg::PositionalUnpack(source) => {
            if !matches!(
                type_info.fixed_unpack_plan(source.span),
                Some(FixedUnpackPlan::Positional(_))
            ) {
                return None;
            }
            match &unparenthesized(source).node {
                ast::Expr::Tuple(items) => Some(items.iter().cloned().map(ast::CallArg::Positional).collect()),
                ast::Expr::List(entries) => entries
                    .iter()
                    .map(|entry| match entry {
                        ast::ListEntry::Element(value) => Some(ast::CallArg::Positional(value.clone())),
                        ast::ListEntry::Spread(_) => None,
                    })
                    .collect(),
                _ => None,
            }
        }
        ast::CallArg::KeywordUnpack(source) => {
            if !matches!(
                type_info.fixed_unpack_plan(source.span),
                Some(FixedUnpackPlan::Keyword(_))
            ) {
                return None;
            }
            let ast::Expr::Dict(entries) = &unparenthesized(source).node else {
                return None;
            };
            entries
                .iter()
                .map(|entry| match entry {
                    ast::DictEntry::Pair(key, value) => match &unparenthesized(key).node {
                        ast::Expr::Literal(ast::Literal::String(name)) => Some(ast::CallArg::Named(
                            ast::Spanned::new(name.clone(), ast::Span::default()),
                            value.clone(),
                        )),
                        _ => None,
                    },
                    ast::DictEntry::Spread(_) => None,
                })
                .collect()
        }
        ast::CallArg::Positional(_) | ast::CallArg::Named(_, _) => None,
    }
}
/// Plan a call's supplied arguments into declaration slots before lowering any expression.
///
/// This validates the whole call before any *argument* ownership read is emitted, then leaves the returned
/// expressions in source evaluation order. A method call is the one exception on the callee side: its receiver is
/// read first, because source evaluation observes the receiver before the arguments, so a refusal here can follow a
/// receiver read that the refused call never consumes. The caller can therefore lower values left-to-right while the
/// final argument vector follows declaration order. Preset-default slots are intentionally omitted from positional
/// binding and may be skipped in the vector because the call's [`bir::ArgumentBinding`] records each supplied operand's
/// declaration slot; an omitted ordinary default is recorded the same way, as a defaulted slot.
///
/// `callee` is the caller's own description of the target (`function \`add\``, `local callable \`g\``,
/// `method \`add\``), so a refusal names the specific spelling that failed rather than a generic label.
pub(super) fn plan_declared_args<'a>(
    callee: &str,
    params: &[DeclaredSlot],
    args: &'a [ast::CallArg],
) -> Result<Vec<(usize, &'a ast::Spanned<ast::Expr>)>, String> {
    if params.iter().any(|param| param.is_rest) {
        return Err(format!("{callee} has a rest parameter"));
    }
    let positional_slots: Vec<usize> = params
        .iter()
        .enumerate()
        .filter_map(|(index, param)| (!param.is_partial_preset).then_some(index))
        .collect();
    let mut slots: Vec<Option<&ast::Spanned<ast::Expr>>> = vec![None; params.len()];
    let mut positional_index = 0usize;
    let mut planned = Vec::with_capacity(args.len());
    for arg in args {
        let (index, expr) = match arg {
            ast::CallArg::Positional(expr) => {
                if positional_index >= positional_slots.len() {
                    return Err(format!(
                        "{callee} expects at most {} positional arguments, got {}",
                        positional_slots.len(),
                        args.len()
                    ));
                }
                let index = positional_slots[positional_index];
                positional_index += 1;
                (index, expr)
            }
            ast::CallArg::Named(arg_name, expr) => {
                let Some(index) = params
                    .iter()
                    .position(|param| param.name.as_deref() == Some(arg_name.node.as_str()))
                else {
                    return Err(format!("{callee} has no parameter `{}`", arg_name.node));
                };
                (index, expr)
            }
            ast::CallArg::PositionalUnpack(_) => {
                return Err(format!("{callee} called with a positional argument spread"));
            }
            ast::CallArg::KeywordUnpack(_) => {
                return Err(format!("{callee} called with a keyword argument spread"));
            }
        };
        if slots[index].is_some() {
            let parameter = params[index].name.as_deref().unwrap_or("<unnamed>");
            return Err(format!("{callee} receives `{parameter}` more than once"));
        }
        slots[index] = Some(expr);
        planned.push((index, expr));
    }

    let required = params.iter().filter(|param| !param.has_default).count();
    if let Some((_index, parameter)) = params
        .iter()
        .enumerate()
        .find(|(index, parameter)| slots[*index].is_none() && !parameter.has_default)
    {
        return Err(format!(
            "{callee} expects at least {required} required arguments, got {}; missing required parameter `{}`",
            args.len(),
            parameter.name.as_deref().unwrap_or("<unnamed>")
        ));
    }
    // An omitted interior default needs no refusal any more. #1124 had to reject one because a flat operand vector
    // could not say which slot a later operand filled; `bir::ArgumentBinding` now records exactly that, so a sparse
    // call is representable rather than ambiguous.
    Ok(planned)
}
/// Wrap fixed operands as single-value element list entries.
///
/// Used by every lowering path that produces a known number of values -- the overwhelming majority. Only a source
/// spread produces a [`bir::ArgumentElement::Spread`], so this keeps those call sites reading as they did before
/// element lists became variable-arity.
pub(super) fn fixed_elements(operands: Vec<bir::Operand>) -> Vec<bir::ArgumentElement> {
    operands.into_iter().map(bir::ArgumentElement::One).collect()
}
