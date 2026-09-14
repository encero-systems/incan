//! Source-authored foreign supertraits retain their absolute Rust identities (#1427).

use crate::backend::IrCodegen;
use crate::frontend::{lexer, parser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Exercise fallback AST lowering, including alias precedence over the builtin Display spelling.
#[test]
fn rust_supertrait_codegen_preserves_imported_alias_and_arguments() -> TestResult {
    let source = r#"
from rust::std::fmt import Display
from rust::std::convert import From as Convert

pub trait Label with Display:
    pass

pub trait Number with Convert[int]:
    pass
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse: {errors:?}")))?;
    let rust = IrCodegen::new()
        .try_generate(&ast)
        .map_err(|error| std::io::Error::other(format!("generate: {error:?}")))?;
    assert!(rust.contains("pub trait Label: ::std::fmt::Display"), "{rust}");
    assert!(rust.contains("pub trait Number: ::std::convert::From<i64>"), "{rust}");
    Ok(())
}

/// A source marker's foreign parent remains a bound; lowering must not synthesize an overlapping blanket impl.
#[test]
fn rust_supertrait_codegen_does_not_implement_foreign_parent() -> TestResult {
    let source = r#"
from std.serde import json
from rust::serde::de import DeserializeOwned

pub trait DecodeReady with DeserializeOwned:
    pass

@derive(json)
pub model Config with DecodeReady:
    value: int
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse: {errors:?}")))?;
    let mut checker = crate::frontend::typechecker::TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("check: {errors:?}")))?;
    let mut codegen = IrCodegen::new();
    codegen.set_prechecked_type_info(checker.type_info().clone(), std::collections::HashMap::new());
    let rust = codegen
        .try_generate(&ast)
        .map_err(|error| std::io::Error::other(format!("generate: {error:?}")))?;
    assert!(
        rust.contains("trait DecodeReady: ::serde::de::DeserializeOwned"),
        "{rust}"
    );
    assert!(rust.contains("impl DecodeReady for Config"), "{rust}");
    assert!(
        rust.contains("serde::Serialize"),
        "Incan derive must supply serialization: {rust}"
    );
    assert!(
        rust.contains("serde::Deserialize"),
        "Incan derive must supply deserialization: {rust}"
    );
    assert!(
        !rust.contains("impl ::serde::de::DeserializeOwned for Config"),
        "{rust}"
    );
    assert!(!rust.contains("impl DeserializeOwned for Config"), "{rust}");
    Ok(())
}
