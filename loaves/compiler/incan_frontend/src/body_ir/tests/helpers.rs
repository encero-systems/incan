//! Helpers that more than one Body IR test module uses: the module builders (`build`,
//! `build_after_expected_typecheck_errors`), the body and target readers (`body_named`, `named_targets`, `rendered_f`,
//! `local_for_binding`) and the shared stand-in refusal statement. A fixture builder only one module needs stays
//! private in that module.

use super::*;

/// Collect the named-callable targets called directly in one lowered body, in statement order.
pub(super) fn named_targets<'module>(
    module: &'module bir::BodyIrModule,
    body_name: &str,
) -> Vec<&'module bir::NamedCallableTarget> {
    module
        .bodies
        .iter()
        .filter(|body| body.name == body_name)
        .flat_map(|body| &body.block.stmts)
        .filter_map(|stmt| match &stmt.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } => Some(target),
            _ => None,
        })
        .collect()
}

pub(super) fn build(source: &str, module_path: &[&str]) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Lower an intentionally-invalid source program after recording its typecheck diagnostics.
///
/// Positive coverage must go through [`build`], which requires ordinary typechecking. This helper is only for
/// Body IR's fail-closed assertions: after the source checker correctly rejects a program, lowering must still
/// make its unsupported representation explicit rather than approximating it.
pub(super) fn build_after_expected_typecheck_errors(
    source: &str,
    module_path: &[&str],
) -> Result<(bir::BodyIrModule, Vec<String>), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    let diagnostics = checker
        .check_program(&program)
        .err()
        .ok_or("expected the intentionally invalid source program to produce a diagnostic")?
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();
    Ok((
        build_body_ir_module_v0(&program, &module_path, checker.type_info()),
        diagnostics,
    ))
}

/// Lower one `def f(...)` body over `source` and return its rendered snapshot.
///
/// The operator tests below all assert against a single function body, so folding "build, find `f`, render" into
/// one call keeps each test's body about the operator rather than about the scaffolding.
pub(super) fn rendered_f(source: &str, module_leaf: &str) -> Result<String, Box<dyn std::error::Error>> {
    let module = build(source, &["m", module_leaf])?;
    Ok(body_named(&module, "f")?.render_snapshot())
}

/// Find the `_N` spelling of the local declared for source binding `name`, so a test can assert on reads of that
/// binding without pinning a local number.
pub(super) fn local_for_binding(snapshot: &str, name: &str) -> Option<String> {
    snapshot.lines().find_map(|line| {
        let (id, tail) = line.trim().strip_prefix("local ")?.split_once(' ')?;
        tail.starts_with(&format!("{name} : ")).then(|| format!("_{id}"))
    })
}

/// Return the named body from a lowered module.
pub(super) fn body_named<'a>(
    module: &'a bir::BodyIrModule,
    name: &str,
) -> Result<&'a bir::Body, Box<dyn std::error::Error>> {
    module
        .bodies
        .iter()
        .find(|body| body.name == name)
        .ok_or_else(|| format!("body `{name}` missing from the lowered module").into())
}

/// Source lines for the shared stand-in refusal, indented to sit inside an enclosing block.
pub(super) fn stand_in_refusal_stmt(indent: &str) -> String {
    format!("{indent}unsafe:\n{indent}  pass\n")
}
