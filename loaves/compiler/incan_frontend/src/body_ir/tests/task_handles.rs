//! Registered SDK task storage preserves concrete arguments without admitting unrelated Rust paths.

use super::body_named;
use crate::body_ir::{build_body_ir_module_v0, library_nominal_declarations};
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};
use incan_semantics_core::{IncanPrimitiveType, IncanType};

/// The registered native task alias retains its argument, opaque fields and missing Clone capability.
#[test]
fn sdk_task_handle_results_retain_native_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::incan_std_async::task import JoinHandle as Handle\n\ndef retain(handle: Handle[int]) -> None:\n    pass\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["m".to_owned(), "task_handle".to_owned()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let body = body_named(&module, "retain")?;
    assert!(body.locals.iter().any(|local| matches!(&local.ty,
        IncanType::Generic { base, args } if base == "JoinHandle" && args == &[IncanType::Primitive(IncanPrimitiveType::Int)]
    )), "{}", body.render_snapshot());
    let layouts = library_nominal_declarations(checker.type_info());
    let handle = layouts
        .iter()
        .find(|layout| layout.name == "JoinHandle")
        .ok_or("missing native handle")?;
    assert!(handle.fields.is_empty());
    assert!(!handle.derives.iter().any(|name| name == "Clone"));
    Ok(())
}

/// An equal-spelled foreign Rust task is not the registered SDK task carrier.
#[test]
fn foreign_task_alias_keeps_its_rust_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "from rust::tokio::task import JoinHandle as Foreign\n\ndef retain(handle: Foreign[int]) -> None:\n    pass\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["m".to_owned(), "foreign_task".to_owned()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let body = body_named(&module, "retain")?;
    assert!(
        body.locals.iter().any(|local| matches!(&local.ty,
            IncanType::RustInteropPath(path) if path.starts_with("tokio::task::JoinHandle<")
        )),
        "{}",
        body.render_snapshot()
    );
    assert!(
        library_nominal_declarations(checker.type_info())
            .iter()
            .all(|layout| layout.name != "JoinHandle")
    );
    Ok(())
}
