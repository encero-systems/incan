//! Native stdlib carriers retain checked ownership independently of source types with the same spelling.

use super::{build, library_nominal_declarations};
use crate::{lexer, parser, typechecker::TypeChecker};
use incan_semantics_core::{CompilerNodeId, SymbolOrigin};

/// An untouched ValidationError builtin in an unused signature still retains its foreign ABI carrier.
#[test]
fn validation_error_retains_builtin_native_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def forward(error: ValidationError) -> ValidationError:\n    return error\n\ndef main() -> None:\n    println(\"validation carrier\")\n";
    let program = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".to_owned(), "native_validation".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let declarations = library_nominal_declarations(checker.type_info());
    let error = declarations
        .iter()
        .find(|owner| owner.name == "ValidationError")
        .ok_or("missing native error carrier")?;
    assert_eq!(error.canonical.origin, SymbolOrigin::Builtin);
    assert_eq!(error.native_item, "incan_std_core::validation::ValidationError");
    assert!(
        error.fields.is_empty(),
        "the error ABI stays opaque until field operations are proved"
    );
    let mut module = build("def main() -> None:\n    pass\n", &["m", "validation_owner"])?;
    module.module_id = CompilerNodeId::module("builtin::native::ValidationError");
    assert!(module.is_well_formed_nominal_declaration(error));
    let mut forged = error.clone();
    forged.native_item = "incan_std_core::reflection::FieldInfo".to_owned();
    assert!(!module.is_well_formed_nominal_declaration(&forged));
    Ok(())
}

/// A real source model called ValidationError keeps its own fields and never selects the builtin carrier.
#[test]
fn source_validation_error_keeps_its_declared_layout() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model ValidationError:\n    reason: str\n\ndef main() -> None:\n    error = ValidationError(reason=\"custom\")\n    println(error.reason)\n";
    let module = build(source, &["m", "source_validation"])?;
    let error = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "ValidationError")
        .ok_or("missing source error")?;
    assert!(error.native_item.is_empty());
    assert_eq!(error.fields, ["reason"]);
    assert!(matches!(error.canonical.origin, SymbolOrigin::Module(_)));
    Ok(())
}
