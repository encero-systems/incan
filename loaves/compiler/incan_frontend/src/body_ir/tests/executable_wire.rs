//! Production positional encoding preserves ordinary calls and complete checked SDK delegation records.

use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::executable_representation::{decode_module, encode_module};

use super::{bir, body_named, build, build_body_ir_module_v0, library_call_delegations};
use crate::typechecker::TypeChecker;
use crate::{lexer, parser};

/// An ordinary method's absent SDK selection occupies its position without fabricating a delegation.
#[test]
fn ordinary_method_targets_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Item:\n    value: int\n    def read(self) -> int:\n        return self.value\n\ndef main() -> int:\n    return Item(value=7).read()\n",
        &["ordinary_method_wire"],
    )?;
    assert!(
        body_named(&module, "main")?
            .block
            .stmts
            .iter()
            .any(|statement| matches!(
                &statement.kind,
                bir::StatementKind::Call { callee: bir::Callee::Method(target), .. } if target.sdk_trait.is_none()
            ))
    );
    assert_roundtrip(&module)
}

/// A checked nongeneric stdlib wrapper encodes its empty declaration-binder vector as a positional field.
#[test]
fn nongeneric_delegation_roundtrips() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "import std.math\n\ndef main() -> float:\n    return math.sqrt(16.0)\n",
        &["nongeneric_delegation_wire"],
    )?;
    let delegation = module
        .stdlib_delegations
        .first()
        .ok_or("checked sqrt delegation absent")?;
    assert!(delegation.type_parameters.is_empty());
    assert_roundtrip(&module)
}

/// The driver's genuine SDK record keeps its empty default vector even when the declaration has a Self binder.
#[test]
fn sdk_declaration_defaults_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.io import BinaryReader, BytesIO\n\ndef main() -> None:\n    reader = BytesIO(b\"abcd\")\n    stream = reader.chunks(2)\n    println(stream.chunk_size)\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["sdk_declaration_wire".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let records = library_call_delegations(checker.type_info());
    let selected = records
        .iter()
        .find(|record| record.canonical.declaration_name == "chunks")
        .ok_or("checked chunks delegation absent")?;
    assert_eq!(selected.type_parameters, ["Self"]);
    assert!(selected.parameter_defaults.is_empty());
    let mut module = build_body_ir_module_v0(&program, &["sdk_declaration_wire".to_owned()], checker.type_info());
    for record in records {
        if !module
            .stdlib_delegations
            .iter()
            .any(|existing| existing.canonical == record.canonical)
        {
            module.stdlib_delegations.push(record);
        }
    }
    assert_roundtrip(&module)
}

/// Use the production binary codec without JSON normalization, projection or a substitute record layout.
fn assert_roundtrip(module: &BodyIrModule) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = encode_module(module)?;
    assert_eq!(&decode_module(&bytes)?, module);
    Ok(())
}
