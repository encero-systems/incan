//! Checked callable records retain every positional field in the production executable codec.

use super::{TypeChecker, bir, body_named, build, lexer, library_call_delegations, parser};
use incan_semantics_core::executable_representation::{decode_module, encode_module};

/// An ordinary method is a control for records without any transparent SDK delegation.
#[test]
fn ordinary_method_roundtrips() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Item:\n    value: int\n    def read(self) -> int:\n        return self.value\n\ndef main() -> int:\n    return Item(value=7).read()\n",
        &["ordinary_method_wire"],
    )?;
    assert!(body_named(&module, "main")?.block.stmts.iter().any(|statement| {
        matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Method(_),
                ..
            }
        )
    }));
    assert_roundtrip(&module)
}

/// A nongeneric transparent wrapper keeps empty source and native argument vectors as wire positions.
#[test]
fn empty_delegation_vectors_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "import std.math\n\ndef main() -> float:\n    return math.sqrt(16.0)\n",
        &["empty_delegation_wire"],
    )?;
    let delegation = module
        .stdlib_delegations
        .first()
        .ok_or("missing checked sqrt delegation")?;
    assert!(delegation.type_parameters.is_empty());
    assert!(delegation.native_type_arguments.is_empty());
    assert_roundtrip(&module)
}

/// A checked associated SDK slot preserves its explicit native receiver binding and empty defaults.
#[test]
fn associated_delegation_roundtrips() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.collections import OrdinalKey\n\npub def encoding[T with OrdinalKey](_value: T) -> str:\n    return T.ordinal_encoding()\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let module_path = vec!["associated_delegation_wire".to_owned()];
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut module = super::build_body_ir_module_v0(&program, &module_path, checker.type_info());
    // Associated SDK slots use the compiled-signature collector rather than transparent source wrappers.
    module.stdlib_delegations = library_call_delegations(checker.type_info());
    assert!(
        !module.stdlib_delegations.is_empty(),
        "missing checked ordinal delegation"
    );
    assert_roundtrip(&module)
}

/// Exercise the actual positional encoding without projecting away the checked records under test.
fn assert_roundtrip(module: &bir::BodyIrModule) -> Result<(), Box<dyn std::error::Error>> {
    let encoded = encode_module(module)?;
    assert_eq!(&decode_module(&encoded)?, module);
    Ok(())
}
