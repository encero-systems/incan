//! The checker's inferred Clone requirement has a direct-route identity without changing legacy call selection.

use super::{bir, body_named, build};
use crate::symbols::canonical_builtin_member_identity;
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};

/// Read one method target from the lowered declaration, requiring a method rather than a guessed function name.
fn method_target<'module>(
    module: &'module bir::BodyIrModule,
    name: &str,
) -> Result<&'module bir::MethodTarget, Box<dyn std::error::Error>> {
    body_named(module, name)?
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or_else(|| std::io::Error::other("missing checked method call").into())
}

/// An unbounded declaration parameter's accepted clone call retains the compiler-owned inferred requirement.
#[test]
fn inferred_parameter_clone_retains_a_builtin_contract() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def duplicate[T](value: T) -> T:\n    \"\"\"Clone through the requirement inferred for the parameter.\"\"\"\n    return value.clone()\n",
        &["m", "inferred_clone"],
    )?;
    let target = method_target(&module, "duplicate")?;
    assert_eq!(
        target.canonical.as_ref(),
        Some(&canonical_builtin_member_identity("Clone", "clone")),
    );
    assert!(target.type_args.is_empty());
    Ok(())
}

/// A source method with the same spelling keeps its declaration and must not acquire the inferred contract.
#[test]
fn source_clone_keeps_its_checked_method_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Counter:\n    \"\"\"Keep a source clone distinct from inferred trait dispatch.\"\"\"\n    value: int\n    def clone(self) -> int:\n        \"\"\"Return this source method's own result.\"\"\"\n        return self.value + 1\n\ndef selected(value: Counter) -> int:\n    \"\"\"Call the concrete method selected by the checker.\"\"\"\n    return value.clone()\n",
        &["m", "source_clone"],
    )?;
    let target = method_target(&module, "selected")?;
    let identity = target.canonical.as_ref().ok_or("missing source clone identity")?;
    assert_ne!(identity, &canonical_builtin_member_identity("Clone", "clone"));
    assert_eq!(identity.declaration_name, "clone");
    Ok(())
}

/// Synthetic protocol resolutions temporarily remove the inferred target and restore it without changing legacy facts.
#[test]
fn inferred_clone_identity_survives_call_site_fact_restoration() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def duplicate[T](value: T) -> T:\n    \"\"\"Keep this source call across synthetic resolution.\"\"\"\n    return value.clone()\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["m".to_string(), "clone_restoration".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut info = checker.type_info().clone();
    let start = source.find("value.clone()").ok_or("missing clone call")?;
    let span = crate::ast::Span::new(start, start + "value.clone()".len());
    let expected = canonical_builtin_member_identity("Clone", "clone");
    let original = crate::body_ir::build_body_ir_module_v0(&program, &path, &info);
    assert_eq!(
        method_target(&original, "duplicate")?.canonical.as_ref(),
        Some(&expected)
    );
    assert!(info.resolved_identity(span).is_none(), "legacy call identity changed");
    let facts = info.take_call_site_facts(span);
    info.clear_body_ir_call_site_facts(span);
    let cleared = crate::body_ir::build_body_ir_module_v0(&program, &path, &info);
    assert!(method_target(&cleared, "duplicate")?.canonical.is_none());
    info.restore_call_site_facts(span, facts);
    let restored = crate::body_ir::build_body_ir_module_v0(&program, &path, &info);
    assert_eq!(
        method_target(&restored, "duplicate")?.canonical.as_ref(),
        Some(&expected)
    );
    assert!(info.resolved_identity(span).is_none(), "legacy call identity changed");
    Ok(())
}
