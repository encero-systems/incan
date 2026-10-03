#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Rust interop regressions driven through the CLI: receiver borrowing, generic scenarios, and metadata-free inference.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_rust_interop_tests_root.rs");

#[test]
fn build_combined_rust_and_source_imports_preserves_never_return_issue381() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "combined_rust_and_source_imports_preserve_never_return",
        r#"

[rust-dependencies]
polyglot_probe = { path = "rust/polyglot_probe" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::polyglot_probe import DialectType
from prism import PrismCursor


def main() -> None:
    pass
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("prism.incn"),
        r#"from rust::incan_std_core::errors import raise_value_error
from rust::std::primitive import i32 as RustI32


pub model PrismCursor:
    pub offset: int


def fail_to_lower() -> RustI32:
    return raise_value_error("cannot lower cursor")
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("polyglot_probe").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("polyglot probe source directory had no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "polyglot_probe"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub enum DialectType {
    PostgreSql,
}
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for combined Rust and source imports");
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "generated Rust for combined imports with a diverging Rust helper",
    );
    Ok(())
}

/// Ensures f-string values compile through both borrowed and owned Rust interop boundaries.
#[test]
fn build_inline_fstring_rust_interop_variants_issue716() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tiny_error");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tiny_error\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub enum TinyError {
    Execution(String),
}

pub fn consume(err: TinyError) -> i64 {
    match err {
        TinyError::Execution(message) => message.len() as i64,
    }
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "inline_fstring_rust_interop_variants_issue716",
        r#"
[rust-dependencies]
tiny_error = { path = "rust/tiny_error" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::incan_std_core::errors import raise_value_error
from rust::tiny_error import TinyError, consume


def fail_inline(value: str) -> int:
    return raise_value_error(f"bad value `{value}`")


def fail_local(value: str) -> int:
    message = f"bad value `{value}`"
    return raise_value_error(message)


def make_error(value: str) -> int:
    return consume(TinyError.Execution(f"bad value `{value}`"))


def main() -> None:
    println(str(make_error("x")))
    fail_inline("x")
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for inline f-string Rust interop variants",
    );
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for inline f-string Rust &str and String enum variants issue716",
    );
    Ok(())
}

#[test]
fn build_static_str_const_rust_string_struct_field() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tiny_option");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tiny_option\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub struct FunctionOption {
    pub name: String,
    pub enabled: bool,
}

pub fn option_name(option: FunctionOption) -> String {
    option.name
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "static_str_const_rust_string_struct_field",
        r#"
[rust-dependencies]
tiny_option = { path = "rust/tiny_option" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tiny_option import FunctionOption, option_name


pub const OPTION_NAME: str = "sketch_family"


def main() -> None:
    option = FunctionOption(name=OPTION_NAME, enabled=True)
    println(option_name(option))
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for a static str const in a Rust String field",
    );
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for static str const into Rust String struct field",
    );
    Ok(())
}

#[test]
fn build_metadata_free_into_bound_tokenizer_encode_issue804() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tokenizers");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tokenizers\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub struct Tokenizer;

pub struct EncodeInput<'a>(&'a str);

impl<'a> From<&'a str> for EncodeInput<'a> {
    fn from(value: &'a str) -> Self {
        Self(value)
    }
}

impl Tokenizer {
    pub fn new() -> Self {
        Self
    }

    pub fn encode<'a, E>(&self, value: E, _add_special_tokens: bool) -> Result<(), ()>
    where
        E: Into<EncodeInput<'a>>,
    {
        let _ = value.into();
        Ok(())
    }
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "metadata_free_into_bound_tokenizer_encode_issue804",
        r#"
[rust-dependencies]
tokenizers = { path = "rust/tokenizers" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tokenizers import Tokenizer

def main() -> None:
    tokenizer = Tokenizer.new()
    literal = tokenizer.encode("literal", False)
    text = "variable"
    variable = tokenizer.encode(text, False)
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(
        &bake_output,
        "explicit Oven bake for metadata-free Into-bound tokenizer encode",
    );
    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for metadata-free Into-bound tokenizer encode issue804",
    );
    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/metadata_free_into_bound_tokenizer_encode_issue804/src/main.rs"),
    )?;
    assert!(
        generated.contains("tokenizer.encode(\"literal\", false)"),
        "literal must preserve its direct &str shape, got:\n{generated}"
    );
    assert!(
        generated.contains("tokenizer.encode((text).as_str(), false)"),
        "owned Incan strings must become &str for the Into-bound method, got:\n{generated}"
    );
    Ok(())
}

/// A comprehension over a by-value Rust iterator (`std::env::Args` has no `.iter()`) consumes it exactly as the
/// `for` statement over the same value does, and `list(args())` collects it the same way (#1490, #1464).
#[test]
fn comprehension_over_rust_iterator_consumes_it_by_value_issue1490() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "comprehension_rust_iterator_issue1490", "")?;
    fs::write(
        &main_path,
        r#"from rust::std::env import args


def lengths() -> list[int]:
    return [len(argument) for argument in args()]


def present_arguments() -> list[str]:
    return [argument for argument in args() if len(argument) > 0]


pub def main() -> None:
    arguments: list[str] = [argument for argument in args()]
    println(f"{len(arguments)}")
    println(len(lengths()))
    println(len(present_arguments()))
    collected = list(args())
    println(len(collected))
    for argument in args():
        println(len(argument) > 0)
"#,
    )?;

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "explicit Oven bake for the #1490 comprehension program");
    let run_output = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run_output, "incan run for the #1490 comprehension program");
    assert_eq!(
        String::from_utf8(run_output.stdout)?.lines().collect::<Vec<_>>(),
        vec!["1", "1", "1", "1", "true"],
        "every argument-vector read must see exactly the program name"
    );

    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/comprehension_rust_iterator_issue1490/src/main.rs"),
    )?;
    // prettyplease breaks a method chain across lines, so the guard compares without whitespace.
    let compact = generated.split_whitespace().collect::<String>();
    assert!(
        !compact.contains("(args()).iter()") && !compact.contains("(args()).clone()"),
        "a by-value Rust iterator must be neither borrowed with .iter() nor cloned:\n{generated}"
    );
    assert!(
        generated.contains("((args()).into_iter())"),
        "the comprehension must consume the iterator through IntoIterator:\n{generated}"
    );
    Ok(())
}
