//! Generated-Rust spelling of default arguments expanded outside their callable's module (#1771).

use incan_emit::IrCodegen;
use incan_frontend::{lexer, parser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Parse one source module.
fn parse(source: &str) -> Result<incan_frontend::ast::Program, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    Ok(parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?)
}

/// Remove whitespace so assertions do not depend on the pretty-printer's line breaks.
fn compact(code: &str) -> String {
    code.chars().filter(|character| !character.is_whitespace()).collect()
}

/// #1771: `a` and `b` each declare `const SIZE` and a function defaulting to it. A caller importing both expands each
/// default outside its module, where `SIZE` alone names neither; each default is spelled through the module that
/// declares its callable, so `first()` passes `a`'s `SIZE` and `second()` passes `b`'s.
#[test]
fn same_named_default_consts_resolve_in_their_callables_modules_issue1771() -> TestResult {
    let a = parse("const SIZE: int = 1\n\npub def first(n: int = SIZE) -> int:\n    return n\n")?;
    let b = parse("const SIZE: int = 2\n\npub def second(n: int = SIZE) -> int:\n    return n\n")?;
    let main = parse(
        "from a import first\nfrom b import second\n\ndef main() -> None:\n    println(first())\n    println(second())\n",
    )?;
    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("a", &a, vec!["a".to_string()]);
    codegen.add_module_with_path_segments("b", &b, vec!["b".to_string()]);
    let (main_code, _) =
        codegen.try_generate_multi_file_nested(&main, &[vec!["a".to_string()], vec!["b".to_string()]])?;
    let code = compact(&main_code);
    assert!(
        code.contains("(crate::a::SIZE"),
        "`first()` takes `a`'s SIZE:\n{main_code}"
    );
    assert!(
        code.contains("(crate::b::SIZE"),
        "`second()` takes `b`'s SIZE:\n{main_code}"
    );
    Ok(())
}
