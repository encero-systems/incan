//! Execution evidence replacing the set-aggregate refusal boundary from #1247's representation-only handoff.

use incan::backend::replacement::{ReplacementValue, execute_free_function};
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_semantics_core::body_ir::BodyIrModule;

/// Lower one self-contained, typechecked source module into the Body IR the replacement backend consumes.
///
/// The test goes through the real frontend pipeline so the executed helpers come from checked source.
fn lower_typed_body_ir(source: &str) -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["replacement_hashed_boundary".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Both set membership forms execute through the existing compiler-owned helper identities.
#[test]
fn replacement_executes_both_set_membership_forms() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> bool:
  xs = {1, 2}
  if 3 not in xs:
    return 1 in xs
  return false
"#;
    let module = lower_typed_body_ir(source)?;
    assert_eq!(
        execute_free_function(&module, "main", &[])?.value,
        ReplacementValue::Bool(true)
    );
    let snapshot = module.render_snapshot();
    for helper in ["set_contains", "set_not_contains"] {
        assert!(
            snapshot.contains(&format!("call helper:{helper}(")),
            "Body IR must retain the {helper} membership that executes: {snapshot}"
        );
    }
    Ok(())
}
