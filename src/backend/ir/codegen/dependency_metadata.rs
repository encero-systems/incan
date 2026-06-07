//! Dependency metadata planning for IR code generation.

use std::collections::{HashMap, HashSet};

use crate::backend::ir::expr::{IrDictEntry, IrGeneratorClause, IrListEntry, MethodKind, Pattern};
use crate::backend::ir::{IrDeclKind, IrExpr, IrExprKind, IrProgram, IrStmt, IrStmtKind};
use crate::frontend::ast::{self, Declaration, Expr, ImportKind, ImportPath, Program};
use crate::frontend::decorator_resolution;
use crate::frontend::module::canonicalize_source_module_segments;
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_core::lang::{generated_support, stdlib, surface::result_methods};

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

/// Resolve a source import path to the generated Rust module path used for dependency emission.
fn generated_module_path_for_source_import(path: &ImportPath, current_module_path: &[String]) -> Option<Vec<String>> {
    let resolved_segments = if path.parent_levels > 0 {
        let keep = current_module_path.len().checked_sub(path.parent_levels)?;
        let mut resolved = current_module_path[..keep].to_vec();
        resolved.extend(path.segments.clone());
        resolved
    } else {
        path.segments.clone()
    };
    let mut segments = canonicalize_source_module_segments(&resolved_segments);

    if segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) {
        segments[0] = stdlib::INCAN_STD_NAMESPACE.to_string();
    }

    Some(segments)
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

/// Return a `std.result` helper that generated method-call emission may call directly.
///
/// This planner runs before typed IR emission, so it mirrors the source-shape side of the emitter's helper predicate:
/// only the known Result combinators with a plain identifier callback can route through the Incan-authored helper.
/// Closures and callable objects remain on direct Rust method emission and do not need a generated `std.result` item.
fn result_helper_used_by_method_call(method: &str, args: &[ast::CallArg]) -> Option<&'static str> {
    let id = result_methods::from_str(method)?;
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
    let Some(ast::CallArg::Positional(callback)) = args.first() else {
        return None;
    };
    matches!(callback.node, Expr::Ident(_)).then_some(result_methods::as_str(id))
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
}

/// Return whether one lowered program uses a surface that backend emission routes through generated Rust paths.
fn ir_program_uses_direct_generated_path_support(
    program: &IrProgram,
    _support: &generated_support::GeneratedPathSupport,
) -> bool {
    program
        .declarations
        .iter()
        .any(ir_decl_uses_direct_generated_path_support)
}

/// Return whether a top-level lowered declaration contains a direct generated-path support trigger.
fn ir_decl_uses_direct_generated_path_support(decl: &crate::backend::ir::IrDecl) -> bool {
    match &decl.kind {
        IrDeclKind::Function(func) => ir_stmts_use_direct_generated_path_support(&func.body),
        IrDeclKind::Struct(_) | IrDeclKind::Enum(_) => false,
        IrDeclKind::Trait(trait_decl) => trait_decl
            .methods
            .iter()
            .any(|method| ir_stmts_use_direct_generated_path_support(&method.body)),
        IrDeclKind::Impl(impl_decl) => impl_decl
            .methods
            .iter()
            .any(|method| ir_stmts_use_direct_generated_path_support(&method.body)),
        IrDeclKind::Const { value, .. } | IrDeclKind::Static { value, .. } => {
            ir_expr_uses_direct_generated_path_support(value)
        }
        IrDeclKind::TypeAlias { .. } | IrDeclKind::SymbolAlias { .. } | IrDeclKind::Import { .. } => false,
    }
}

/// Return whether any statement in a lowered block contains a direct generated-path support trigger.
fn ir_stmts_use_direct_generated_path_support(stmts: &[IrStmt]) -> bool {
    stmts.iter().any(ir_stmt_uses_direct_generated_path_support)
}

/// Return whether one lowered statement contains a direct generated-path support trigger in an expression position.
fn ir_stmt_uses_direct_generated_path_support(stmt: &IrStmt) -> bool {
    match &stmt.kind {
        IrStmtKind::Expr(expr)
        | IrStmtKind::Yield(expr)
        | IrStmtKind::Let { value: expr, .. }
        | IrStmtKind::Assign { value: expr, .. }
        | IrStmtKind::CompoundAssign { value: expr, .. } => ir_expr_uses_direct_generated_path_support(expr),
        IrStmtKind::Return(expr) => expr.as_ref().is_some_and(ir_expr_uses_direct_generated_path_support),
        IrStmtKind::Break { value, .. } => value.as_ref().is_some_and(ir_expr_uses_direct_generated_path_support),
        IrStmtKind::While { condition, body, .. } => {
            ir_expr_uses_direct_generated_path_support(condition) || ir_stmts_use_direct_generated_path_support(body)
        }
        IrStmtKind::For { iterable, body, .. } => {
            ir_expr_uses_direct_generated_path_support(iterable) || ir_stmts_use_direct_generated_path_support(body)
        }
        IrStmtKind::Loop { body, .. } | IrStmtKind::Block(body) => ir_stmts_use_direct_generated_path_support(body),
        IrStmtKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            ir_expr_uses_direct_generated_path_support(condition)
                || ir_stmts_use_direct_generated_path_support(then_branch)
                || else_branch
                    .as_ref()
                    .is_some_and(|branch| ir_stmts_use_direct_generated_path_support(branch))
        }
        IrStmtKind::Match { scrutinee, arms } => {
            ir_expr_uses_direct_generated_path_support(scrutinee)
                || arms.iter().any(|arm| {
                    arm.bindings.iter().any(|binding| {
                        ir_expr_uses_direct_generated_path_support(&binding.value)
                            || binding
                                .guard_value
                                .as_ref()
                                .is_some_and(ir_expr_uses_direct_generated_path_support)
                    }) || arm
                        .guard
                        .as_ref()
                        .is_some_and(ir_expr_uses_direct_generated_path_support)
                        || ir_expr_uses_direct_generated_path_support(&arm.body)
                })
        }
        IrStmtKind::Continue(_) => false,
    }
}

/// Return whether one lowered expression contains semantics that emission routes through generated Rust paths.
fn ir_expr_uses_direct_generated_path_support(expr: &IrExpr) -> bool {
    match &expr.kind {
        IrExprKind::KnownMethodCall {
            kind: MethodKind::Iterator(_),
            ..
        } => true,
        IrExprKind::KnownMethodCall { receiver, args, .. } => {
            ir_expr_uses_direct_generated_path_support(receiver)
                || args
                    .iter()
                    .any(|arg| ir_expr_uses_direct_generated_path_support(&arg.expr))
        }
        IrExprKind::MethodCall { receiver, args, .. } => {
            ir_expr_uses_direct_generated_path_support(receiver)
                || args
                    .iter()
                    .any(|arg| ir_expr_uses_direct_generated_path_support(&arg.expr))
        }
        IrExprKind::Call { func, args, .. } => {
            ir_expr_uses_direct_generated_path_support(func)
                || args
                    .iter()
                    .any(|arg| ir_expr_uses_direct_generated_path_support(&arg.expr))
        }
        IrExprKind::BuiltinCall { args, .. } => args.iter().any(ir_expr_uses_direct_generated_path_support),
        IrExprKind::BinOp { left, right, .. } => {
            ir_expr_uses_direct_generated_path_support(left) || ir_expr_uses_direct_generated_path_support(right)
        }
        IrExprKind::UnaryOp { operand, .. }
        | IrExprKind::Await(operand)
        | IrExprKind::Try(operand)
        | IrExprKind::Cast { expr: operand, .. }
        | IrExprKind::NumericResize { expr: operand, .. }
        | IrExprKind::InteropCoerce { expr: operand, .. } => ir_expr_uses_direct_generated_path_support(operand),
        IrExprKind::RegisterCallableName { callable, .. } => ir_expr_uses_direct_generated_path_support(callable),
        IrExprKind::CacheGenericDecoratedFunction { value, .. } => ir_expr_uses_direct_generated_path_support(value),
        IrExprKind::Field { object, .. } => ir_expr_uses_direct_generated_path_support(object),
        IrExprKind::Index { object, index } => {
            ir_expr_uses_direct_generated_path_support(object) || ir_expr_uses_direct_generated_path_support(index)
        }
        IrExprKind::Slice {
            target,
            start,
            end,
            step,
        } => {
            ir_expr_uses_direct_generated_path_support(target)
                || [start, end, step]
                    .into_iter()
                    .flatten()
                    .any(|expr| ir_expr_uses_direct_generated_path_support(expr))
        }
        IrExprKind::ListComp {
            element,
            pattern,
            iterable,
            filter,
        } => {
            ir_expr_uses_direct_generated_path_support(element)
                || ir_pattern_uses_direct_generated_path_support(pattern)
                || ir_expr_uses_direct_generated_path_support(iterable)
                || filter
                    .as_ref()
                    .is_some_and(|expr| ir_expr_uses_direct_generated_path_support(expr))
        }
        IrExprKind::DictComp {
            key,
            value,
            pattern,
            iterable,
            filter,
        } => {
            ir_expr_uses_direct_generated_path_support(key)
                || ir_expr_uses_direct_generated_path_support(value)
                || ir_pattern_uses_direct_generated_path_support(pattern)
                || ir_expr_uses_direct_generated_path_support(iterable)
                || filter
                    .as_ref()
                    .is_some_and(|expr| ir_expr_uses_direct_generated_path_support(expr))
        }
        IrExprKind::Generator { element, clauses } => {
            ir_expr_uses_direct_generated_path_support(element)
                || clauses.iter().any(|clause| match clause {
                    IrGeneratorClause::For { pattern, iterable } => {
                        ir_pattern_uses_direct_generated_path_support(pattern)
                            || ir_expr_uses_direct_generated_path_support(iterable)
                    }
                    IrGeneratorClause::If(expr) => ir_expr_uses_direct_generated_path_support(expr),
                })
        }
        IrExprKind::List(items) => items.iter().any(|item| match item {
            IrListEntry::Element(expr) | IrListEntry::Spread(expr) => ir_expr_uses_direct_generated_path_support(expr),
        }),
        IrExprKind::Dict(items) => items.iter().any(|item| match item {
            IrDictEntry::Pair(key, value) => {
                ir_expr_uses_direct_generated_path_support(key) || ir_expr_uses_direct_generated_path_support(value)
            }
            IrDictEntry::Spread(expr) => ir_expr_uses_direct_generated_path_support(expr),
        }),
        IrExprKind::Set(items) | IrExprKind::Tuple(items) => {
            items.iter().any(ir_expr_uses_direct_generated_path_support)
        }
        IrExprKind::Struct { fields, .. } => fields
            .iter()
            .any(|(_, value)| ir_expr_uses_direct_generated_path_support(value)),
        IrExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            ir_expr_uses_direct_generated_path_support(condition)
                || ir_expr_uses_direct_generated_path_support(then_branch)
                || else_branch
                    .as_ref()
                    .is_some_and(|expr| ir_expr_uses_direct_generated_path_support(expr))
        }
        IrExprKind::Match { scrutinee, arms } => {
            ir_expr_uses_direct_generated_path_support(scrutinee)
                || arms.iter().any(|arm| {
                    ir_pattern_uses_direct_generated_path_support(&arm.pattern)
                        || arm.bindings.iter().any(|binding| {
                            ir_expr_uses_direct_generated_path_support(&binding.value)
                                || binding
                                    .guard_value
                                    .as_ref()
                                    .is_some_and(ir_expr_uses_direct_generated_path_support)
                        })
                        || arm
                            .guard
                            .as_ref()
                            .is_some_and(ir_expr_uses_direct_generated_path_support)
                        || ir_expr_uses_direct_generated_path_support(&arm.body)
                })
        }
        IrExprKind::Closure { body, .. } => ir_expr_uses_direct_generated_path_support(body),
        IrExprKind::Block { stmts, value } => {
            ir_stmts_use_direct_generated_path_support(stmts)
                || value
                    .as_ref()
                    .is_some_and(|expr| ir_expr_uses_direct_generated_path_support(expr))
        }
        IrExprKind::Loop { body } => ir_stmts_use_direct_generated_path_support(body),
        IrExprKind::Race { arms, .. } => arms.iter().any(|arm| {
            ir_expr_uses_direct_generated_path_support(&arm.awaitable)
                || ir_expr_uses_direct_generated_path_support(&arm.body)
        }),
        IrExprKind::Range { start, end, .. } => [start, end]
            .into_iter()
            .flatten()
            .any(|expr| ir_expr_uses_direct_generated_path_support(expr)),
        IrExprKind::Format { parts } => parts.iter().any(|part| match part {
            crate::backend::ir::expr::FormatPart::Literal(_) => false,
            crate::backend::ir::expr::FormatPart::Expr { expr, .. } => ir_expr_uses_direct_generated_path_support(expr),
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

/// Return whether a lowered pattern contains expression payloads that require direct generated-path support.
fn ir_pattern_uses_direct_generated_path_support(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Literal(expr) => ir_expr_uses_direct_generated_path_support(expr),
        Pattern::Tuple(items) | Pattern::Or(items) => items.iter().any(ir_pattern_uses_direct_generated_path_support),
        Pattern::Struct { fields, .. } => fields
            .iter()
            .any(|(_, pattern)| ir_pattern_uses_direct_generated_path_support(pattern)),
        Pattern::Enum { fields, .. } => fields.iter().any(ir_pattern_uses_direct_generated_path_support),
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
                    let Some(module_path) = generated_module_path_for_source_import(module, current_module_path) else {
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
                    let Some(segments) = generated_module_path_for_source_import(path, current_module_path) else {
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
        let _ = crate::frontend::ast_walk::any_expr_in_program(program, |expr| {
            if let Expr::MethodCall(_, method, _, args) = expr
                && let Some(helper) = result_helper_used_by_method_call(method, args)
            {
                reachable
                    .entry(vec![
                        stdlib::INCAN_STD_NAMESPACE.to_string(),
                        stdlib::STDLIB_RESULT.to_string(),
                    ])
                    .or_default()
                    .insert(helper.to_string());
            }
            false
        });
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
    reachable
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
