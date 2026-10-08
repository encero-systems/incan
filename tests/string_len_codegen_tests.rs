//! Native emission coverage for Unicode-scalar string length.

use incan::backend::IrCodegen;
use incan::frontend::{lexer, parser};

fn generate(source: &str, entrypoint: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut codegen = IrCodegen::new();
    codegen.set_externally_reachable_items(std::collections::HashSet::from([entrypoint.to_string()]));
    Ok(codegen.try_generate(&program)?)
}

/// Both public string-length spellings must emit the shared Unicode-scalar runtime helper.
#[test]
fn string_len_spelling_emit_shared_runtime_semantics() -> Result<(), Box<dyn std::error::Error>> {
    for expression in ["len(value)", "value.len()"] {
        let source = format!("def length(value: str) -> int:\n    return {expression}\n");
        let rust = generate(&source, "length")?;
        assert!(
            rust.contains("incan_stdlib::strings::str_len"),
            "{expression} did not emit shared string length:\n{rust}"
        );
        assert!(
            !rust.contains("value.len() as i64"),
            "{expression} leaked Rust byte length:\n{rust}"
        );
    }
    Ok(())
}

/// Non-string builtin length keeps element-count emission.
#[test]
fn list_len_keeps_collection_element_count_emission() -> Result<(), Box<dyn std::error::Error>> {
    let rust = generate(
        "def length(values: list[int]) -> int:\n    return len(values)\n",
        "length",
    )?;
    assert!(rust.contains(".len() as i64"), "{rust}");
    assert!(!rust.contains("incan_stdlib::strings::str_len"), "{rust}");
    Ok(())
}
