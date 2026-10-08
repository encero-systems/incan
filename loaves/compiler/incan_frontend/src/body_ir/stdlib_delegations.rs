//! Preserve only proven transparent stdlib forwarding; arbitrary source implementations remain unsupported.

use super::{CanonicalSymbolId, IncanPrimitiveType, IncanType, TypeCheckInfo, ast, bir, semantic_type_from_resolved};
use crate::typechecker::{TypeChecker, stdlib_loader::StdlibAstCache};
use incan_semantics_core::SymbolOrigin;

/// Resolve source declarations by the reference's checked identity, deduplicating aliases deterministically.
pub(super) fn collect(type_info: &TypeCheckInfo) -> Vec<bir::StdlibDelegation> {
    let identities = type_info
        .references
        .resolved_identities
        .values()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut cache = StdlibAstCache::new();
    identities
        .iter()
        .filter_map(|identity| delegation(identity, &mut cache))
        .collect()
}

/// Prove that one undecorated scalar wrapper, or the canonical SDK task-spawn wrapper, returns its Rust callee with
/// every parameter exactly once in declaration order. Checking supplies the native identity; the source spelling
/// never becomes an execution path. Generic task carriers require separately retained checked instantiation facts.
fn delegation(identity: &CanonicalSymbolId, cache: &mut StdlibAstCache) -> Option<bir::StdlibDelegation> {
    let source_identity = cache.callable_source_identity(identity)?;
    let SymbolOrigin::Module(path) = &source_identity.origin else {
        return None;
    };
    let program = cache.callable_program(identity)?;
    let mut functions = program
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.node {
            ast::Declaration::Function(function) => Some(function),
            _ => None,
        });
    let function = functions.next()?;
    if functions.next().is_some() {
        return None;
    }
    let task_spawn = path.as_slice() == ["std", "async", "task"] && function.name == "spawn";
    if function.is_async()
        || !function.decorators.is_empty()
        || (!task_spawn && !function.type_params.is_empty())
        || function
            .params
            .iter()
            .any(|parameter| parameter.node.default.is_some() || parameter.node.kind != ast::ParamKind::Normal)
    {
        return None;
    }
    // The SDK spawn wrapper documents its contract with a leading literal statement. It has no execution effect;
    // retaining that docstring must not make a proven forwarding call disappear from Body IR.
    let body = if task_spawn
        && matches!(
            function.body.first().map(|statement| &statement.node),
            Some(ast::Statement::Expr(ast::Spanned {
                node: ast::Expr::Literal(ast::Literal::String(_)),
                ..
            }))
        ) {
        &function.body[1..]
    } else {
        function.body.as_slice()
    };
    let [statement] = body else {
        return None;
    };
    let ast::Statement::Return(Some(value)) = &statement.node else {
        return None;
    };
    let ast::Expr::Call(callee, type_args, arguments) = &value.node else {
        return None;
    };
    if !type_args.is_empty() || arguments.len() != function.params.len() {
        return None;
    }
    for (argument, parameter) in arguments.iter().zip(&function.params) {
        let ast::CallArg::Positional(argument) = argument else {
            return None;
        };
        let ast::Expr::Ident(name) = &argument.node else {
            return None;
        };
        if name != &parameter.node.name {
            return None;
        }
    }
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker.check_program(&program).ok()?;
    let target = checker.type_info().resolved_identity(callee.span)?.clone();
    let binding = checker.type_info().declarations.function_bindings.get(&function.name)?;
    if binding.identity.as_ref() != Some(&source_identity) {
        return None;
    }
    let parameters = binding
        .params
        .iter()
        .map(|parameter| semantic_type_from_resolved(&parameter.ty))
        .collect::<Vec<_>>();
    let return_type = semantic_type_from_resolved(&binding.return_type);
    if !task_spawn && !parameters.iter().chain(std::iter::once(&return_type)).all(scalar) {
        return None;
    }
    let fact = bir::StdlibDelegation {
        canonical: identity.clone(),
        target,
        parameters,
        return_type,
    };
    let path = fact.rust_path()?;
    if task_spawn && path != "incan_std_async::task::spawn" {
        return None;
    }
    Some(fact)
}

/// Keep the delegation profile to native copy scalars; allocation and borrow behavior require richer facts.
fn scalar(ty: &IncanType) -> bool {
    matches!(
        ty,
        IncanType::Primitive(
            IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Bool | IncanPrimitiveType::Unit
        )
    )
}

#[cfg(test)]
mod tests {
    use super::{StdlibAstCache, SymbolOrigin, delegation};

    /// Generic SDK spawn forwarding retains its checked native target without admitting arbitrary source wrappers.
    #[test]
    fn sdk_task_spawn_retains_its_checked_delegation() -> Result<(), Box<dyn std::error::Error>> {
        let mut cache = StdlibAstCache::new();
        let path = vec!["std".to_owned(), "async".to_owned(), "task".to_owned()];
        let identity = cache
            .lookup_identity(&path, "spawn")
            .ok_or("missing SDK spawn identity")?;
        let program = cache.callable_program(&identity).ok_or("missing SDK spawn source")?;
        let mut checker = super::TypeChecker::new();
        checker.set_current_module_path(Some(path));
        let checked = checker.check_program(&program);
        let fact = delegation(&identity, &mut cache);
        assert!(
            fact.is_some(),
            "checked: {checked:?}\nbindings: {:?}\nreferences: {:?}",
            checker.type_info().declarations.function_bindings,
            checker.type_info().references.resolved_identities
        );
        assert_eq!(
            fact.and_then(|fact| fact.rust_path()).as_deref(),
            Some("incan_std_async::task::spawn")
        );
        Ok(())
    }

    /// SDK publication may rebase the owner, but a wrong package or declaration span cannot gain forwarding rights.
    #[test]
    fn package_delegations_require_the_exact_catalog_owned_declaration() -> Result<(), Box<dyn std::error::Error>> {
        let mut cache = StdlibAstCache::new();
        let path = vec!["std".to_string(), "math".to_string()];
        let mut identity = cache.lookup_identity(&path, "sqrt").ok_or("missing sqrt identity")?;
        identity.origin = SymbolOrigin::Package {
            library: "incan_stdlib_data".to_string(),
            module_path: vec!["math".to_string()],
        };
        let fact = delegation(&identity, &mut cache).ok_or("SDK sqrt must retain its forwarding fact")?;
        assert_eq!(fact.canonical, identity);
        assert_eq!(fact.rust_path().as_deref(), Some("libm::sqrt"));
        identity.declaration_span.start += 1;
        assert!(delegation(&identity, &mut cache).is_none());
        identity.declaration_span.start -= 1;
        identity.origin = SymbolOrigin::Package {
            library: "foreign_math".to_string(),
            module_path: vec!["math".to_string()],
        };
        assert!(delegation(&identity, &mut cache).is_none());
        Ok(())
    }
}
