//! A module-qualified type annotation must reach generated Rust as a type path, never as a dotted identifier.
//!
//! See #1437. `import errors` followed by `Result[None, errors.TomlError]` typechecked, but the annotation resolved to
//! `Unknown` in the checker and lowered to the literal spelling `errors.TomlError`, which the Rust emitter then handed
//! to `proc_macro2::Ident::new` -- a panic (`"errors.TomlError" is not a valid Ident`), not a diagnostic. The direct
//! import `from errors import TomlError` with `Result[None, TomlError]` always emitted correctly.
//!
//! The root is in the middle end. The checker now resolves the dotted spelling through the module member registry
//! and records the proven declaration under the spelling; lowering reads that fact and places the type at the module
//! the spelling walked. The emitter is unchanged: a `::` path is a shape it has always spelled.

use incan_emit::IrCodegen;
use incan_frontend::ast::Program;
use incan_frontend::{lexer, parser};

/// The provider module both spellings reach: one public model, one public function returning it.
const ERRORS_MODULE: &str = concat!(
    "pub model TomlError:\n",
    "    pub message: str\n",
    "\n",
    "pub def make() -> TomlError:\n",
    "    return TomlError(message=\"x\")\n",
);

/// Parse one fixture, keeping fixture failures as ordinary test errors.
fn parse(source: &str) -> Result<Program, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?)
}

/// Generate the root module of a two-module project whose root is `main` and whose dependency is `errors`.
fn generated_root(main: &str) -> Result<String, Box<dyn std::error::Error>> {
    let errors = parse(ERRORS_MODULE)?;
    let main = parse(main)?;
    let mut codegen = IrCodegen::new();
    codegen.add_module("errors", &errors);
    let (root, _modules) = codegen
        .try_generate_multi_file(&main, &["errors"])
        .map_err(|error| std::io::Error::other(format!("generation failed: {error}")))?;
    Ok(root)
}

/// Strip whitespace so an assertion can name a token sequence without depending on `prettyplease` line breaks.
fn compact(code: &str) -> String {
    code.chars().filter(|character| !character.is_whitespace()).collect()
}

/// `errors.TomlError` inside a generic return annotation emits the type at its module, like the direct import does.
///
/// The control is the same program written with `from errors import TomlError`; it emits the bare name plus a
/// `use`. The qualified spelling has no import binding for the bare name, so it must emit a path instead -- what
/// must hold is that both compile to a `Result` whose error type resolves to the one `TomlError` in `crate::errors`.
#[test]
fn a_module_qualified_return_annotation_emits_a_type_path_issue1437() -> Result<(), Box<dyn std::error::Error>> {
    let qualified = generated_root(concat!(
        "import errors\n",
        "\n",
        "pub def result() -> Result[None, errors.TomlError]:\n",
        "    return Ok(None)\n",
        "\n",
        "def main() -> None:\n",
        "    pass\n",
    ))?;
    let direct = generated_root(concat!(
        "from errors import TomlError\n",
        "\n",
        "pub def result() -> Result[None, TomlError]:\n",
        "    return Ok(None)\n",
        "\n",
        "def main() -> None:\n",
        "    pass\n",
    ))?;

    let compact_qualified = compact(&qualified);
    assert!(
        compact_qualified.contains("->Result<(),crate::errors::TomlError,>"),
        "the qualified annotation must emit the type at its module:\n{qualified}"
    );
    assert!(
        !compact_qualified.contains("errors.TomlError"),
        "no dotted spelling may survive into generated Rust:\n{qualified}"
    );
    let compact_direct = compact(&direct);
    assert!(
        compact_direct.contains("usecrate::errors::TomlError;") && compact_direct.contains("->Result<(),TomlError,>"),
        "the direct-import control changed shape, so this test no longer compares like with like:\n{direct}"
    );
    Ok(())
}

/// The qualified spelling works wherever an annotation can appear, not only in a return position.
///
/// A parameter, a local binding annotation, and a model field each lower through the same type path, and each
/// used to reach the emitter as a dotted identifier.
#[test]
fn a_module_qualified_type_is_a_path_in_every_annotation_position_issue1437() -> Result<(), Box<dyn std::error::Error>>
{
    let rust = generated_root(concat!(
        "import errors\n",
        "\n",
        "pub model Report:\n",
        "    pub failure: errors.TomlError\n",
        "\n",
        "pub def describe(failure: errors.TomlError) -> str:\n",
        "    held: errors.TomlError = failure\n",
        "    return held.message\n",
        "\n",
        "def main() -> None:\n",
        "    report = Report(failure=errors.make())\n",
        "    println(describe(report.failure))\n",
    ))?;
    let compact_rust = compact(&rust);
    for (position, shape) in [
        ("model field", "pubfailure:crate::errors::TomlError,"),
        ("parameter", "(failure:crate::errors::TomlError,)->String"),
        ("local binding", "letheld:crate::errors::TomlError=failure;"),
    ] {
        assert!(
            compact_rust.contains(shape),
            "the {position} annotation must emit the module path:\n{rust}"
        );
    }
    assert!(
        !compact_rust.contains("errors.TomlError"),
        "no dotted spelling may survive into generated Rust:\n{rust}"
    );
    Ok(())
}

/// A dotted spelling the checker cannot prove is a diagnostic at the annotation, not an emitter panic.
///
/// This is the other half of #1437's contract: an unsupported shape has to be refused before lowering, because the
/// emitter can only spell what lowering hands it. The refusal names the module and the missing member.
#[test]
fn an_unprovable_qualified_type_is_refused_by_the_checker_issue1437() -> Result<(), Box<dyn std::error::Error>> {
    let outcome = generated_root(concat!(
        "import errors\n",
        "\n",
        "pub def result() -> Result[None, errors.Missing]:\n",
        "    return Ok(None)\n",
        "\n",
        "def main() -> None:\n",
        "    pass\n",
    ));
    let Err(error) = outcome else {
        return Err("the annotation names no declaration and must be refused".into());
    };
    let message = error.to_string();
    assert!(
        message.contains("`errors.Missing` is not a type: module `errors` declares no type or trait named `Missing`"),
        "the refusal must name the module and the missing member: {message}"
    );
    Ok(())
}
