//! Web Path storage follows the authenticated Axum re-export, independently of filesystem Path.

use super::*;
use incan_semantics_core::SymbolOrigin;

/// The checked web surface retains its original Rust owner and public tuple payload.
#[test]
fn web_path_retains_native_tuple_payload() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.web import Path\n\ndef accept(value: Path[int]) -> None:\n    pass\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let declarations = library_nominal_declarations(checker.type_info());
    let path = declarations
        .iter()
        .find(|declaration| declaration.name == "Path")
        .ok_or("missing web Path")?;
    assert_eq!(path.native_item, "incan_stdlib_web::web::request::Path");
    assert_eq!(path.fields, ["0"]);
    assert_eq!(path.type_parameters, ["T"]);
    assert!(matches!(&path.canonical.origin, SymbolOrigin::RustCrate(owner) if owner == &["axum", "extract", "Path"]));
    Ok(())
}

/// Filesystem imports retain their own source owner and cannot select the Axum tuple ABI.
#[test]
fn filesystem_path_does_not_select_web_storage() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.fs import Path\n\ndef accept(value: Path) -> None:\n    pass\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    assert!(
        library_nominal_declarations(checker.type_info())
            .iter()
            .all(|declaration| !declaration.native_item.contains("web::request::Path"))
    );
    Ok(())
}
