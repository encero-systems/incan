//! Dependency metadata planning for IR code generation.

use std::collections::{HashMap, HashSet};

use crate::backend::ir::expr::{IrDictEntry, IrGeneratorClause, IrListEntry, MethodKind, Pattern, VarRefKind};
use crate::backend::ir::{IrDecl, IrDeclKind, IrExpr, IrExprKind, IrProgram, IrStmt, IrStmtKind, IrType};
use crate::frontend::ast::{self, Declaration, Expr, ImportKind, ImportPath, Program};
use crate::frontend::decorator_resolution;
use crate::frontend::module::{canonicalize_source_module_segments, logical_source_import_candidates};
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_core::lang::{
    generated_support, stdlib,
    surface::result_methods,
    traits::{self as core_traits, TraitId},
};

/// Collect field-alias metadata for exported models.
pub(super) fn collect_model_field_aliases(
    main: &Program,
    deps: &[(&str, &Program)],
) -> HashMap<String, HashMap<String, String>> {
    let mut out: HashMap<String, HashMap<String, String>> = HashMap::new();

    let mut visit = |p: &Program| {
        for decl in &p.declarations {
            let Declaration::Model(m) = &decl.node else {
                continue;
            };

            let mut map: HashMap<String, String> = HashMap::new();
            for f in &m.fields {
                if let Some(alias) = &f.node.metadata.alias {
                    map.insert(alias.clone(), f.node.name.clone());
                }
            }

            if !map.is_empty() {
                out.entry(m.name.clone()).or_default().extend(map);
            }
        }
    };

    visit(main);
    for (_, dep) in deps {
        visit(dep);
    }

    out
}

/// Convert one canonical source path into its generated Rust module path.
fn generated_module_path_for_source_path(path: &[String]) -> Vec<String> {
    let mut segments = canonicalize_source_module_segments(path);
    if segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) {
        segments[0] = stdlib::INCAN_STD_NAMESPACE.to_string();
    }
    segments
}

/// Convert a dependency's source path into the Rust module path used by generated reachability metadata.
fn generated_module_path_for_dependency(path_segments: &[String]) -> Vec<String> {
    generated_module_path_for_source_path(path_segments)
}

/// Resolve a source import to a generated dependency module path.
///
/// Source programs may import a sibling by its local spelling while generated dependency modules retain their
/// canonical source path. Use that exact path when available; otherwise accept a suffix only when it identifies one
/// provider module. Multiple providers with the same suffix remain unresolved rather than selecting one by name.
fn generated_dependency_module_path_for_source_import(
    path: &ImportPath,
    current_module_path: &[String],
    module_paths: &HashSet<Vec<String>>,
) -> Option<Vec<String>> {
    let candidates = logical_source_import_candidates(current_module_path, path)
        .into_iter()
        .map(|candidate| generated_module_path_for_source_path(&candidate))
        .collect::<Vec<_>>();
    for candidate in &candidates {
        if module_paths.contains(candidate) {
            return Some(candidate.clone());
        }
    }
    let generated_path = candidates.last()?.clone();
    if path.is_absolute || path.parent_levels > 0 {
        return Some(generated_path);
    }

    let mut providers = module_paths
        .iter()
        .filter(|module_path| module_path.ends_with(&generated_path))
        .cloned()
        .collect::<Vec<_>>();
    providers.sort();
    providers.dedup();
    if providers.len() == 1 {
        Some(providers.remove(0))
    } else {
        // Keep existing handling for standard-library and non-provider imports. An ambiguous provider suffix is
        // intentionally left at its source spelling, so it cannot retain declarations from an arbitrary module.
        Some(generated_path)
    }
}

/// True when a dependency module should keep its public API even if the main module does not import every item.
pub(super) fn should_preserve_dependency_public_items(
    _module_path: &[String],
    preserve_non_stdlib_public_items: bool,
) -> bool {
    preserve_non_stdlib_public_items
}

/// Return whether a function carries the stdlib-backed web route decorator that lowers to a Rust proc-macro attribute.
fn has_web_route_passthrough_decorator(
    func: &ast::FunctionDecl,
    aliases: &HashMap<String, Vec<String>>,
    stdlib_cache: &mut StdlibAstCache,
) -> bool {
    func.decorators.iter().any(|decorator| {
        let resolved = decorator_resolution::resolve_decorator_path(&decorator.node, aliases);
        if resolved.len() < 2 {
            return false;
        }
        let module_segments = &resolved[..resolved.len() - 1];
        let name = &resolved[resolved.len() - 1];
        if name != "route" {
            return false;
        }
        let Some(meta) = stdlib_cache.lookup_function_meta(module_segments, name) else {
            return false;
        };
        meta.is_rust_extern && meta.rust_module_path.as_deref() == Some("incan_web_macros")
    })
}

/// Return whether a dependency module path is the source or generated owner of registered generated support.
fn module_path_matches_generated_support(
    module_path: &[String],
    support: &generated_support::GeneratedModuleSupport,
) -> bool {
    let dotted = module_path.join(".");
    dotted == support.source_module || dotted == support.generated_module
}

/// Keep the exact generated items that registered support macros expand against.
fn record_generated_support_required_items(
    reachable: &mut HashMap<Vec<String>, HashSet<String>>,
    current_module_path: &[String],
) {
    for support in generated_support::generated_module_supports() {
        if !module_path_matches_generated_support(current_module_path, support) {
            continue;
        }
        let required_items = reachable.entry(current_module_path.to_vec()).or_default();
        required_items.extend(support.required_items.iter().map(|item| (*item).to_string()));
    }
}

/// Keep support items for compiler-generated Rust paths when lowered IR uses the triggering semantic surface.
///
/// This deliberately runs on IR instead of source spelling. Domain APIs may legitimately define methods named
/// `filter`, `map`, or `count`; only calls that lowering classified as `MethodKind::Iterator` cause the emitter to
/// name `__incan_std.derives.collection` directly.
pub(super) fn record_direct_generated_path_support_items_from_ir(
    reachable: &mut HashMap<Vec<String>, HashSet<String>>,
    program: &IrProgram,
) {
    for support in generated_support::generated_path_supports() {
        if !ir_program_uses_direct_generated_path_support(program, support) {
            continue;
        }
        for module in [support.source_module, support.generated_module] {
            let module_path = module.split('.').map(str::to_string).collect::<Vec<_>>();
            let required_items = reachable.entry(module_path).or_default();
            required_items.extend(support.required_items.iter().map(|item| (*item).to_string()));
        }
    }
    record_result_helper_support_items_from_ir(reachable, program);
}

/// Return whether one lowered program uses a surface that backend emission routes through generated Rust paths.
fn ir_program_uses_direct_generated_path_support(
    program: &IrProgram,
    support: &generated_support::GeneratedPathSupport,
) -> bool {
    match support.trigger {
        generated_support::GeneratedPathSupportTrigger::IteratorMethod => ir_program_any_expr(program, &mut |expr| {
            matches!(
                expr.kind,
                IrExprKind::KnownMethodCall {
                    kind: MethodKind::Iterator(_),
                    ..
                }
            )
        }),
    }
}

/// Keep `std.result` helper items when lowered Result method calls route through Incan-authored helpers.
fn record_result_helper_support_items_from_ir(
    reachable: &mut HashMap<Vec<String>, HashSet<String>>,
    program: &IrProgram,
) {
    let mut helpers = HashSet::new();
    let _ = ir_program_any_expr(program, &mut |expr| {
        if let Some(helper) = result_helper_used_by_known_method_call(program, expr) {
            helpers.insert(helper.to_string());
        }
        false
    });
    if helpers.is_empty() {
        return;
    }
    reachable
        .entry(vec![
            stdlib::INCAN_STD_NAMESPACE.to_string(),
            stdlib::STDLIB_RESULT.to_string(),
        ])
        .or_default()
        .extend(helpers);
}

/// Return a `std.result` helper when a lowered Result method call will emit through that helper.
fn result_helper_used_by_known_method_call(program: &IrProgram, expr: &IrExpr) -> Option<&'static str> {
    let IrExprKind::KnownMethodCall {
        kind: MethodKind::Result(id),
        args,
        ..
    } = &expr.kind
    else {
        return None;
    };
    if !matches!(
        id,
        result_methods::ResultMethodId::Map
            | result_methods::ResultMethodId::MapErr
            | result_methods::ResultMethodId::AndThen
            | result_methods::ResultMethodId::OrElse
            | result_methods::ResultMethodId::Inspect
            | result_methods::ResultMethodId::InspectErr
    ) {
        return None;
    }
    let callback = args.first().map(|arg| &arg.expr)?;
    let IrExprKind::Var {
        name,
        ref_kind: VarRefKind::Value,
        ..
    } = &callback.kind
    else {
        return None;
    };
    if !matches!(callback.ty, IrType::Function { .. }) || program.function_registry.get(name).is_none() {
        return None;
    }
    Some(result_methods::as_str(*id))
}

/// Return whether any expression in a lowered program satisfies `predicate`.
fn ir_program_any_expr<P>(program: &IrProgram, predicate: &mut P) -> bool
where
    P: FnMut(&IrExpr) -> bool,
{
    program
        .declarations
        .iter()
        .any(|decl| ir_decl_any_expr(decl, predicate))
}

/// Return whether any expression in a top-level lowered declaration satisfies `predicate`.
fn ir_decl_any_expr<P>(decl: &IrDecl, predicate: &mut P) -> bool
where
    P: FnMut(&IrExpr) -> bool,
{
    match &decl.kind {
        IrDeclKind::Function(func) => ir_stmts_any_expr(&func.body, predicate),
        IrDeclKind::Struct(_) | IrDeclKind::Enum(_) => false,
        IrDeclKind::Trait(trait_decl) => trait_decl
            .methods
            .iter()
            .any(|method| ir_stmts_any_expr(&method.body, predicate)),
        IrDeclKind::Impl(impl_decl) => impl_decl
            .methods
            .iter()
            .any(|method| ir_stmts_any_expr(&method.body, predicate)),
        IrDeclKind::Const { value, .. } | IrDeclKind::Static { value, .. } => ir_expr_any_expr(value, predicate),
        IrDeclKind::TypeAlias { .. } | IrDeclKind::SymbolAlias { .. } | IrDeclKind::Import { .. } => false,
    }
}

/// Return whether any statement in a lowered block contains an expression satisfying `predicate`.
fn ir_stmts_any_expr<P>(stmts: &[IrStmt], predicate: &mut P) -> bool
where
    P: FnMut(&IrExpr) -> bool,
{
    stmts.iter().any(|stmt| ir_stmt_any_expr(stmt, predicate))
}

/// Return whether one lowered statement contains an expression satisfying `predicate`.
fn ir_stmt_any_expr<P>(stmt: &IrStmt, predicate: &mut P) -> bool
where
    P: FnMut(&IrExpr) -> bool,
{
    match &stmt.kind {
        IrStmtKind::Expr(expr)
        | IrStmtKind::Yield(expr)
        | IrStmtKind::Let { value: expr, .. }
        | IrStmtKind::Assign { value: expr, .. }
        | IrStmtKind::CompoundAssign { value: expr, .. } => ir_expr_any_expr(expr, predicate),
        IrStmtKind::Return(expr) => expr.as_ref().is_some_and(|expr| ir_expr_any_expr(expr, predicate)),
        IrStmtKind::Break { value, .. } => value.as_ref().is_some_and(|expr| ir_expr_any_expr(expr, predicate)),
        IrStmtKind::While { condition, body, .. } => {
            ir_expr_any_expr(condition, predicate) || ir_stmts_any_expr(body, predicate)
        }
        IrStmtKind::For { iterable, body, .. } => {
            ir_expr_any_expr(iterable, predicate) || ir_stmts_any_expr(body, predicate)
        }
        IrStmtKind::Loop { body, .. } | IrStmtKind::Block(body) => ir_stmts_any_expr(body, predicate),
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            ir_expr_any_expr(condition, predicate)
                || ir_stmts_any_expr(then_branch, predicate)
                || else_branch
                    .as_ref()
                    .is_some_and(|branch| ir_stmts_any_expr(branch, predicate))
        }
        IrStmtKind::Match { scrutinee, arms } => {
            ir_expr_any_expr(scrutinee, predicate)
                || arms.iter().any(|arm| {
                    arm.bindings.iter().any(|binding| {
                        ir_expr_any_expr(&binding.value, predicate)
                            || binding
                                .guard_value
                                .as_ref()
                                .is_some_and(|expr| ir_expr_any_expr(expr, predicate))
                    }) || arm.guard.as_ref().is_some_and(|expr| ir_expr_any_expr(expr, predicate))
                        || ir_expr_any_expr(&arm.body, predicate)
                })
        }
        IrStmtKind::Continue(_) => false,
    }
}

/// Return whether one lowered expression or any nested expression satisfies `predicate`.
fn ir_expr_any_expr<P>(expr: &IrExpr, predicate: &mut P) -> bool
where
    P: FnMut(&IrExpr) -> bool,
{
    if predicate(expr) {
        return true;
    }
    match &expr.kind {
        IrExprKind::KnownMethodCall { receiver, args, .. } => {
            ir_expr_any_expr(receiver, predicate) || args.iter().any(|arg| ir_expr_any_expr(&arg.expr, predicate))
        }
        IrExprKind::MethodCall { receiver, args, .. } => {
            ir_expr_any_expr(receiver, predicate) || args.iter().any(|arg| ir_expr_any_expr(&arg.expr, predicate))
        }
        IrExprKind::Call { func, args, .. } => {
            ir_expr_any_expr(func, predicate) || args.iter().any(|arg| ir_expr_any_expr(&arg.expr, predicate))
        }
        IrExprKind::BuiltinCall { args, .. } => args.iter().any(|expr| ir_expr_any_expr(expr, predicate)),
        IrExprKind::BinOp { left, right, .. } => {
            ir_expr_any_expr(left, predicate) || ir_expr_any_expr(right, predicate)
        }
        IrExprKind::UnaryOp { operand, .. }
        | IrExprKind::Await(operand)
        | IrExprKind::Try(operand)
        | IrExprKind::Cast { expr: operand, .. }
        | IrExprKind::NumericResize { expr: operand, .. }
        | IrExprKind::InteropCoerce { expr: operand, .. } => ir_expr_any_expr(operand, predicate),
        IrExprKind::RegisterCallableName { callable, .. } => ir_expr_any_expr(callable, predicate),
        IrExprKind::CacheGenericDecoratedFunction { value, .. } => ir_expr_any_expr(value, predicate),
        IrExprKind::Field { object, .. } => ir_expr_any_expr(object, predicate),
        IrExprKind::Index { object, index } => {
            ir_expr_any_expr(object, predicate) || ir_expr_any_expr(index, predicate)
        }
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            ir_expr_any_expr(target, predicate)
                || [start, end, step]
                    .into_iter()
                    .flatten()
                    .any(|expr| ir_expr_any_expr(expr, predicate))
        }
        IrExprKind::ListComp {
            element,
            pattern,
            iterable,
            filter,
        } => {
            ir_expr_any_expr(element, predicate)
                || ir_pattern_any_expr(pattern, predicate)
                || ir_expr_any_expr(iterable, predicate)
                || filter.as_ref().is_some_and(|expr| ir_expr_any_expr(expr, predicate))
        }
        IrExprKind::DictComp {
            key,
            value,
            pattern,
            iterable,
            filter,
        } => {
            ir_expr_any_expr(key, predicate)
                || ir_expr_any_expr(value, predicate)
                || ir_pattern_any_expr(pattern, predicate)
                || ir_expr_any_expr(iterable, predicate)
                || filter.as_ref().is_some_and(|expr| ir_expr_any_expr(expr, predicate))
        }
        IrExprKind::Generator { element, clauses } => {
            ir_expr_any_expr(element, predicate)
                || clauses.iter().any(|clause| match clause {
                    IrGeneratorClause::For { pattern, iterable } => {
                        ir_pattern_any_expr(pattern, predicate) || ir_expr_any_expr(iterable, predicate)
                    }
                    IrGeneratorClause::If(expr) => ir_expr_any_expr(expr, predicate),
                })
        }
        IrExprKind::List(items) => items.iter().any(|item| match item {
            IrListEntry::Element(expr) | IrListEntry::Spread(expr) => ir_expr_any_expr(expr, predicate),
        }),
        IrExprKind::Dict(items) => items.iter().any(|item| match item {
            IrDictEntry::Pair(key, value) => ir_expr_any_expr(key, predicate) || ir_expr_any_expr(value, predicate),
            IrDictEntry::Spread(expr) => ir_expr_any_expr(expr, predicate),
        }),
        IrExprKind::Set(items) | IrExprKind::Tuple(items) => items.iter().any(|expr| ir_expr_any_expr(expr, predicate)),
        IrExprKind::Struct { fields, .. } => fields.iter().any(|(_, value)| ir_expr_any_expr(value, predicate)),
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            ir_expr_any_expr(condition, predicate)
                || ir_expr_any_expr(then_branch, predicate)
                || else_branch
                    .as_ref()
                    .is_some_and(|expr| ir_expr_any_expr(expr, predicate))
        }
        IrExprKind::Match { scrutinee, arms } => {
            ir_expr_any_expr(scrutinee, predicate)
                || arms.iter().any(|arm| {
                    ir_pattern_any_expr(&arm.pattern, predicate)
                        || arm.bindings.iter().any(|binding| {
                            ir_expr_any_expr(&binding.value, predicate)
                                || binding
                                    .guard_value
                                    .as_ref()
                                    .is_some_and(|expr| ir_expr_any_expr(expr, predicate))
                        })
                        || arm.guard.as_ref().is_some_and(|expr| ir_expr_any_expr(expr, predicate))
                        || ir_expr_any_expr(&arm.body, predicate)
                })
        }
        IrExprKind::Closure { body, .. } => ir_expr_any_expr(body, predicate),
        IrExprKind::Block { stmts, value } => {
            ir_stmts_any_expr(stmts, predicate) || value.as_ref().is_some_and(|expr| ir_expr_any_expr(expr, predicate))
        }
        IrExprKind::Loop { body } => ir_stmts_any_expr(body, predicate),
        IrExprKind::Race { arms, .. } => arms
            .iter()
            .any(|arm| ir_expr_any_expr(&arm.awaitable, predicate) || ir_expr_any_expr(&arm.body, predicate)),
        IrExprKind::Range { start, end, .. } => [start, end]
            .into_iter()
            .flatten()
            .any(|expr| ir_expr_any_expr(expr, predicate)),
        IrExprKind::Format { parts } => parts.iter().any(|part| match part {
            crate::backend::ir::expr::FormatPart::Literal(_) => false,
            crate::backend::ir::expr::FormatPart::Expr { expr, .. } => ir_expr_any_expr(expr, predicate),
        }),
        IrExprKind::Var { .. }
        | IrExprKind::StaticRead { .. }
        | IrExprKind::StaticBinding { .. }
        | IrExprKind::AssociatedFunction { .. }
        | IrExprKind::TypeToken { .. }
        | IrExprKind::FunctionItem { .. }
        | IrExprKind::Unit
        | IrExprKind::None
        | IrExprKind::Bool(_)
        | IrExprKind::Int(_)
        | IrExprKind::IntLiteral(_)
        | IrExprKind::Float(_)
        | IrExprKind::Decimal(_)
        | IrExprKind::String(_)
        | IrExprKind::Bytes(_)
        | IrExprKind::Literal(_)
        | IrExprKind::FieldsList(_)
        | IrExprKind::SerdeToJson
        | IrExprKind::SerdeFromJson(_) => false,
    }
}

/// Return whether a lowered pattern contains expression payloads satisfying `predicate`.
fn ir_pattern_any_expr<P>(pattern: &Pattern, predicate: &mut P) -> bool
where
    P: FnMut(&IrExpr) -> bool,
{
    match pattern {
        Pattern::Literal(expr) => ir_expr_any_expr(expr, predicate),
        Pattern::Tuple(items) | Pattern::Or(items) => {
            items.iter().any(|pattern| ir_pattern_any_expr(pattern, predicate))
        }
        Pattern::Struct { fields, .. } => fields
            .iter()
            .any(|(_, pattern)| ir_pattern_any_expr(pattern, predicate)),
        Pattern::Enum { fields, .. } => fields.iter().any(|pattern| ir_pattern_any_expr(pattern, predicate)),
        Pattern::Wildcard | Pattern::Var(_) => false,
    }
}

/// Keep serde JSON traits when the serde activation planner will emit impls against the generated `json` module.
fn record_serde_json_trait_support_items(
    reachable: &mut HashMap<Vec<String>, HashSet<String>>,
    main: &Program,
    dependency_modules: &[(&str, &Program, Option<Vec<String>>)],
) {
    let deps = dependency_modules
        .iter()
        .map(|(name, program, _)| (*name, *program))
        .collect::<Vec<_>>();
    let (needs_serialize, needs_deserialize) = super::serde_activation::collect_serde_derives(main, &deps);
    if !needs_serialize && !needs_deserialize {
        return;
    }
    let items = reachable
        .entry(vec![
            stdlib::INCAN_STD_NAMESPACE.to_string(),
            stdlib::STDLIB_SERDE.to_string(),
            stdlib::STDLIB_JSON.to_string(),
        ])
        .or_default();
    if needs_serialize {
        items.insert("Serialize".to_string());
    }
    if needs_deserialize {
        items.insert("Deserialize".to_string());
    }
}

/// Collect declaration names referenced by one source type annotation.
///
/// Dependency emission prunes declarations that are not reachable from an import. A retained trait's public
/// signature is itself a reachability root: Rust cannot compile the trait if one of its same-module trait bounds or
/// parameter types was pruned. Keep this syntactic collection deliberately local to the defining module; imported
/// modules are already represented as independent dependency entries.
fn collect_type_signature_references(ty: &ast::Type, names: &mut HashSet<String>) {
    match ty {
        ast::Type::Simple(name) => {
            names.insert(name.clone());
        }
        ast::Type::Generic(name, args) => {
            names.insert(name.clone());
            for arg in args {
                collect_type_signature_references(&arg.node, names);
            }
        }
        ast::Type::Function(params, return_type) => {
            for param in params {
                collect_type_signature_references(&param.node, names);
            }
            collect_type_signature_references(&return_type.node, names);
        }
        ast::Type::Ref(inner) | ast::Type::RefMut(inner) => {
            collect_type_signature_references(&inner.node, names);
        }
        ast::Type::Tuple(elements) => {
            for element in elements {
                collect_type_signature_references(&element.node, names);
            }
        }
        ast::Type::Qualified(_)
        | ast::Type::ConstrainedPrimitive(_, _)
        | ast::Type::IntLiteral(_)
        | ast::Type::Unit
        | ast::Type::SelfType
        | ast::Type::Infer => {}
    }
}

/// Collect trait names referenced by one source trait bound.
fn collect_trait_bound_signature_references(bound: &ast::TraitBound, names: &mut HashSet<String>) {
    names.insert(bound.name.clone());
    for arg in &bound.type_args {
        collect_type_signature_references(&arg.node, names);
    }
}

/// Collect same-module declarations required to emit one trait's public surface.
///
/// Most dependencies come directly from source signatures. `Iterator.sum()` is the one temporary exception: its source
/// body is ordinary Incan, but the backend currently supplies its per-method `T: Sum[T]` Rust bound because Incan has
/// no syntax for a bound on an existing trait type parameter. Keep `Sum` alongside the retained `Iterator` declaration
/// until that source-level bound can be represented and lowered without this bridge.
fn trait_emission_references(trait_decl: &ast::TraitDecl) -> HashSet<String> {
    let mut names = HashSet::new();
    for type_param in &trait_decl.type_params {
        for bound in &type_param.bounds {
            collect_trait_bound_signature_references(bound, &mut names);
        }
    }
    for bound in &trait_decl.traits {
        collect_trait_bound_signature_references(&bound.node, &mut names);
    }
    for property in &trait_decl.properties {
        collect_type_signature_references(&property.node.return_type.node, &mut names);
    }
    for method in &trait_decl.methods {
        for type_param in &method.node.type_params {
            for bound in &type_param.bounds {
                collect_trait_bound_signature_references(bound, &mut names);
            }
        }
        if let Some(bound) = &method.node.trait_target {
            collect_trait_bound_signature_references(&bound.node, &mut names);
        }
        for param in &method.node.params {
            collect_type_signature_references(&param.node.ty.node, &mut names);
        }
        collect_type_signature_references(&method.node.return_type.node, &mut names);
    }
    if trait_decl.name == core_traits::as_str(TraitId::Iterator)
        && trait_decl
            .methods
            .iter()
            .any(|method| method.node.name == "sum" && method.node.body.is_some())
    {
        names.insert(core_traits::as_str(TraitId::Sum).to_string());
    }
    names
}

/// Extend selected trait declarations with the local trait declarations needed to emit their public surface.
///
/// This fixed-point closure is needed because an initial import can retain `Sum[T]` while its `sum` method refers to
/// `Iterator[T]`. Both source declarations must be emitted together, regardless of whether the importing program
/// happens to call an iterator method.
fn retain_same_module_trait_signature_dependencies(
    reachable: &mut HashMap<Vec<String>, HashSet<String>>,
    dependency_modules: &[(&str, &Program, Option<Vec<String>>)],
) {
    loop {
        let mut additions = Vec::new();
        for (module_name, program, path_segments) in dependency_modules {
            let source_module_path = path_segments
                .clone()
                .unwrap_or_else(|| vec![(*module_name).to_string()]);
            let module_path = generated_module_path_for_dependency(&source_module_path);
            let Some(selected) = reachable.get(&module_path) else {
                continue;
            };
            let declared_traits = program
                .declarations
                .iter()
                .filter_map(|decl| match &decl.node {
                    Declaration::Trait(trait_decl) => Some((trait_decl.name.as_str(), trait_decl)),
                    _ => None,
                })
                .collect::<HashMap<_, _>>();
            for selected_name in selected {
                let Some(trait_decl) = declared_traits.get(selected_name.as_str()) else {
                    continue;
                };
                for reference in trait_emission_references(trait_decl) {
                    if declared_traits.contains_key(reference.as_str()) && !selected.contains(&reference) {
                        additions.push((module_path.clone(), reference));
                    }
                }
            }
        }
        if additions.is_empty() {
            return;
        }
        for (module_path, name) in additions {
            reachable.entry(module_path).or_default().insert(name);
        }
    }
}

/// Collect dependency-module declarations that must remain reachable from externally visible roots such as imports,
/// ambient logging, and web route registration.
pub(super) fn collect_externally_reachable_items_by_module(
    main: &Program,
    dependency_modules: &[(&str, &Program, Option<Vec<String>>)],
) -> HashMap<Vec<String>, HashSet<String>> {
    let module_paths: HashSet<Vec<String>> = dependency_modules
        .iter()
        .map(|(name, _, path_segments)| path_segments.clone().unwrap_or_else(|| vec![(*name).to_string()]))
        .collect();

    /// Record dependency imports from checked module metadata.
    fn record_imports(
        reachable: &mut HashMap<Vec<String>, HashSet<String>>,
        program: &Program,
        current_module_path: &[String],
        module_paths: &HashSet<Vec<String>>,
    ) {
        record_generated_support_required_items(reachable, current_module_path);
        if crate::frontend::surface_semantics::uses_ambient_log_surface(program) {
            reachable
                .entry(vec!["std".to_string(), "logging".to_string()])
                .or_default()
                .insert("get_logger".to_string());
        }
        let mut module_import_bindings: HashMap<String, Vec<String>> = HashMap::new();
        for decl in &program.declarations {
            let Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::From { module, items } => {
                    let Some(module_path) =
                        generated_dependency_module_path_for_source_import(module, current_module_path, module_paths)
                    else {
                        continue;
                    };
                    let reachable_items = reachable.entry(module_path.clone()).or_default();
                    for item in items {
                        reachable_items.insert(item.name.clone());
                        let mut submodule_path = module_path.clone();
                        submodule_path.push(item.name.clone());
                        if module_paths.contains(&submodule_path) {
                            let binding = item.alias.clone().unwrap_or_else(|| item.name.clone());
                            module_import_bindings.insert(binding, submodule_path);
                        }
                    }
                }
                ImportKind::Module(path) => {
                    let Some(segments) =
                        generated_dependency_module_path_for_source_import(path, current_module_path, module_paths)
                    else {
                        continue;
                    };
                    if module_paths.contains(&segments) {
                        if let Some(binding) = import.alias.clone().or_else(|| path.segments.last().cloned()) {
                            module_import_bindings.insert(binding, segments);
                        }
                        continue;
                    }
                    let Some(item_name) = segments.last() else {
                        continue;
                    };
                    for module_path in module_paths {
                        if segments.len() == module_path.len() + 1 && segments.starts_with(module_path) {
                            reachable
                                .entry(module_path.clone())
                                .or_default()
                                .insert(item_name.clone());
                            break;
                        }
                    }
                }
                ImportKind::PubLibrary { .. }
                | ImportKind::PubFrom { .. }
                | ImportKind::RustCrate { .. }
                | ImportKind::RustFrom { .. }
                | ImportKind::Python(_) => {}
            }
        }
        if !module_import_bindings.is_empty() {
            let _ = crate::frontend::ast_walk::any_expr_in_program(program, |expr| {
                if let Expr::Field(object, field) = expr
                    && let Expr::Ident(binding) = &object.node
                    && let Some(module_path) = module_import_bindings.get(binding)
                {
                    reachable.entry(module_path.clone()).or_default().insert(field.clone());
                }
                if let Expr::MethodCall(object, method, _, _) = expr
                    && let Expr::Ident(binding) = &object.node
                    && let Some(module_path) = module_import_bindings.get(binding)
                {
                    reachable.entry(module_path.clone()).or_default().insert(method.clone());
                }
                false
            });
        }
        if module_paths.contains(current_module_path) {
            let aliases = decorator_resolution::collect_import_aliases(program);
            let mut stdlib_cache = StdlibAstCache::new();
            for decl in &program.declarations {
                let Declaration::Function(func) = &decl.node else {
                    continue;
                };
                if has_web_route_passthrough_decorator(func, &aliases, &mut stdlib_cache) {
                    reachable
                        .entry(current_module_path.to_vec())
                        .or_default()
                        .insert(func.name.clone());
                }
            }
        }
    }

    let mut reachable = HashMap::new();
    record_serde_json_trait_support_items(&mut reachable, main, dependency_modules);
    record_imports(&mut reachable, main, &[String::from("main")], &module_paths);
    for (name, program, path_segments) in dependency_modules {
        let module_path = path_segments.clone().unwrap_or_else(|| vec![(*name).to_string()]);
        record_imports(&mut reachable, program, &module_path, &module_paths);
    }
    retain_same_module_trait_signature_dependencies(&mut reachable, dependency_modules);
    reachable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Program {
        let tokens =
            crate::frontend::lexer::lex(source).unwrap_or_else(|errors| panic!("fixture should lex: {errors:?}"));
        crate::frontend::parser::parse(&tokens).unwrap_or_else(|errors| panic!("fixture should parse: {errors:?}"))
    }

    #[test]
    fn selected_trait_keeps_same_module_trait_used_by_its_signature() {
        let iterator = core_traits::as_str(TraitId::Iterator);
        let sum = core_traits::as_str(TraitId::Sum);
        let main = parse(&format!("from std.derives.collection import {sum}\n"));
        let collection = parse(&format!(
            r#"
pub trait {iterator}[T]:
    def __next__(mut self) -> Option[T]: ...

pub trait {sum}[T]:
    @classmethod
    def sum(cls, items: {iterator}[T]) -> Self: ...
"#,
        ));
        let reachable = collect_externally_reachable_items_by_module(
            &main,
            &[(
                "collection",
                &collection,
                Some(vec!["std".to_string(), "derives".to_string(), "collection".to_string()]),
            )],
        );
        let collection_path = vec![
            "__incan_std".to_string(),
            "derives".to_string(),
            "collection".to_string(),
        ];
        let Some(selected) = reachable.get(&collection_path) else {
            panic!("collection should be reachable");
        };
        assert!(selected.contains(sum));
        assert!(selected.contains(iterator));
    }

    #[test]
    fn selected_iterator_keeps_sum_used_by_source_owned_default() {
        let iterator = core_traits::as_str(TraitId::Iterator);
        let sum = core_traits::as_str(TraitId::Sum);
        let main = parse(&format!("from std.derives.collection import {iterator}\n"));
        let collection = parse(&format!(
            r#"
pub trait {iterator}[T]:
    def __next__(mut self) -> Option[T]: ...

    def sum(mut self) -> T:
        return T.sum(self)

pub trait {sum}[T]:
    @classmethod
    def sum(cls, items: {iterator}[T]) -> Self: ...
"#,
        ));
        let reachable = collect_externally_reachable_items_by_module(
            &main,
            &[(
                "collection",
                &collection,
                Some(vec!["std".to_string(), "derives".to_string(), "collection".to_string()]),
            )],
        );
        let collection_path = vec![
            "__incan_std".to_string(),
            "derives".to_string(),
            "collection".to_string(),
        ];
        let Some(selected) = reachable.get(&collection_path) else {
            panic!("collection should be reachable");
        };
        assert!(selected.contains(iterator));
        assert!(selected.contains(sum));
    }

    #[test]
    fn bare_source_import_keeps_unique_canonical_provider_item() {
        let main = parse("from text_vaults import Vault\n");
        let vaults = parse("pub class Vault:\n    value: str\n");
        let path = vec!["pkg".to_string(), "text_vaults".to_string()];
        let reachable =
            collect_externally_reachable_items_by_module(&main, &[("text_vaults", &vaults, Some(path.clone()))]);

        assert_eq!(reachable.get(&path), Some(&HashSet::from(["Vault".to_string()])));
    }

    #[test]
    fn bare_source_import_does_not_choose_ambiguous_provider_suffix() {
        let main = parse("from text_vaults import Vault\n");
        let vaults = parse("pub class Vault:\n    value: str\n");
        let first = vec!["pkg".to_string(), "text_vaults".to_string()];
        let second = vec!["other".to_string(), "text_vaults".to_string()];
        let reachable = collect_externally_reachable_items_by_module(
            &main,
            &[
                ("text_vaults", &vaults, Some(first.clone())),
                ("text_vaults", &vaults, Some(second.clone())),
            ],
        );

        assert!(!reachable.contains_key(&first));
        assert!(!reachable.contains_key(&second));
    }

    #[test]
    fn source_import_prefers_exact_sibling_over_source_root() {
        let path = ImportPath::simple(vec!["text_vaults".to_string()]);
        let sibling = vec!["pkg".to_string(), "text_vaults".to_string()];
        let root = vec!["text_vaults".to_string()];
        let module_paths = HashSet::from([root, sibling.clone()]);

        assert_eq!(
            generated_dependency_module_path_for_source_import(
                &path,
                &["pkg".to_string(), "consumer".to_string()],
                &module_paths,
            ),
            Some(sibling)
        );
    }
}

/// Dependency symbol facts gathered during codegen setup and reused by module emission.
#[derive(Debug, Clone, Default)]
pub(super) struct DependencySymbolMetadata {
    pub(super) module_paths: HashMap<String, Vec<String>>,
    pub(super) ambiguous_type_names: HashSet<String>,
    pub(super) value_module_paths: HashMap<String, Vec<String>>,
    pub(super) ambiguous_value_names: HashSet<String>,
    pub(super) enum_type_names: HashSet<String>,
}

/// Collect dependency symbol metadata needed by IR emission for cross-module nominal types and values.
pub(super) fn collect_dependency_symbol_metadata(
    deps: &[(&str, &Program, Option<Vec<String>>)],
) -> DependencySymbolMetadata {
    let mut paths: HashMap<String, Vec<String>> = HashMap::new();
    let mut ambiguous: HashSet<String> = HashSet::new();
    let mut value_paths: HashMap<String, Vec<String>> = HashMap::new();
    let mut ambiguous_values: HashSet<String> = HashSet::new();
    let mut enum_type_names: HashSet<String> = HashSet::new();
    let mut non_enum_type_names: HashSet<String> = HashSet::new();

    for (_name, program, path_segments) in deps {
        for decl in &program.declarations {
            if let Some(segs) = path_segments.as_ref()
                && let Some(name) = match &decl.node {
                    Declaration::Const(c) => Some(&c.name),
                    Declaration::Static(s) => Some(&s.name),
                    Declaration::Function(f) => Some(&f.name),
                    Declaration::Partial(p) => Some(&p.name),
                    Declaration::Alias(a) => Some(&a.name),
                    Declaration::Import(_)
                    | Declaration::Model(_)
                    | Declaration::Class(_)
                    | Declaration::Trait(_)
                    | Declaration::TypeAlias(_)
                    | Declaration::Newtype(_)
                    | Declaration::Enum(_)
                    | Declaration::TestModule(_)
                    | Declaration::Docstring(_) => None,
                }
            {
                if let Some(existing) = value_paths.get(name) {
                    if existing != segs {
                        ambiguous_values.insert(name.clone());
                    }
                } else {
                    value_paths.insert(name.clone(), segs.clone());
                }
            }

            let type_name = match &decl.node {
                Declaration::Model(m) => Some((&m.name, false)),
                Declaration::Class(c) => Some((&c.name, false)),
                Declaration::Enum(e) => Some((&e.name, true)),
                Declaration::TypeAlias(a) => Some((&a.name, false)),
                Declaration::Newtype(n) => Some((&n.name, false)),
                _ => None,
            };
            let Some((name, is_enum)) = type_name else {
                continue;
            };

            if is_enum {
                enum_type_names.insert(name.clone());
            } else {
                non_enum_type_names.insert(name.clone());
            }

            let Some(segs) = path_segments.as_ref() else {
                continue;
            };

            if let Some(existing) = paths.get(name) {
                if existing != segs {
                    ambiguous.insert(name.clone());
                }
            } else {
                paths.insert(name.clone(), segs.clone());
            }
        }
    }

    for name in &ambiguous {
        paths.remove(name);
    }
    for name in &ambiguous_values {
        value_paths.remove(name);
    }
    enum_type_names.retain(|name| !ambiguous.contains(name) && !non_enum_type_names.contains(name));

    DependencySymbolMetadata {
        module_paths: paths,
        ambiguous_type_names: ambiguous,
        value_module_paths: value_paths,
        ambiguous_value_names: ambiguous_values,
        enum_type_names,
    }
}
