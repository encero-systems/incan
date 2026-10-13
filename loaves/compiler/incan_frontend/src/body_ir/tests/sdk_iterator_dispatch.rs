//! SDK loop hooks retain their native trait declarations independently of the written iterable call.

use super::{TypeChecker, build_body_ir_module_v0, lexer, parser};

/// Taking and restoring written call facts must not remove the SDK declarations needed by synthetic loop hooks.
#[test]
fn checked_sdk_iteration_hooks_retain_native_delegations() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/lowering_declarations/stdlib_trait_default_names_its_own_module_type.incn"
    );
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["m".to_owned(), "sdk_iterator_dispatch".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let facts = checker.type_info();
    let module = build_body_ir_module_v0(&program, &path, facts);
    assert!(!facts.protocols.body_ir_iterations.is_empty());
    for hooks in facts.protocols.body_ir_iterations.values() {
        for hook in [&hooks.iter, &hooks.next] {
            let identity = hook.canonical.as_ref().ok_or("missing checked SDK hook identity")?;
            let native = module
                .stdlib_delegations
                .iter()
                .find(|declaration| &declaration.canonical == identity)
                .ok_or_else(|| format!("missing native hook declaration: {}", identity.render_compact()))?;
            assert!(native.rust_path().is_some());
            assert!(!native.parameters.is_empty());
        }
    }
    Ok(())
}
