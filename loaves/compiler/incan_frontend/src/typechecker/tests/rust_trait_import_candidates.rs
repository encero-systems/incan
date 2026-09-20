//! A method call through an imported Rust trait counts as a use of that import, metadata or not (#1450).
//!
//! Without inspected metadata the compiler cannot say which methods `std::borrow::Borrow` provides, or even that it
//! is a trait. These tests pin the fact the typechecker records instead: such an import has an unknown method
//! surface, and a method call that no inspected surface resolves names every unknown-surface import as a candidate
//! the generated `use` must survive for.

use super::*;

/// The issue's reduced producer: a standard-library trait whose method is called on a borrowed receiver.
const BORROWED_PATH: &str = r#"
from rust::std::borrow import Borrow
from rust::std::path import PathBuf

pub def borrowed(value: &PathBuf) -> &PathBuf:
    return value.borrow()
"#;

/// Typecheck `source` and return the checker for fact inspection.
fn checked(source: &str) -> Result<TypeChecker, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex failed: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse failed: {errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("expected the program to typecheck: {errors:?}")))?;
    Ok(checker)
}

/// Return the candidate bindings recorded for the method call spelled `call` in `source`.
fn candidates_for(checker: &TypeChecker, source: &str, call: &str) -> Option<Vec<String>> {
    let start = source.find(call)?;
    checker
        .type_info()
        .rust_method_trait_import_candidates(Span::new(start, start + call.len()))
        .map(<[String]>::to_vec)
}

/// An import with neither metadata nor a fallback vocabulary is registered with an unknown method surface.
#[test]
fn metadata_free_rust_import_has_an_unknown_method_surface() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(BORROWED_PATH)?;
    let import = checker
        .type_info()
        .rust
        .trait_imports
        .get("Borrow")
        .ok_or("a metadata-free Rust import must stay a trait candidate")?;
    assert!(!import.methods_known, "{import:?}");
    assert!(import.methods.is_empty(), "{import:?}");
    assert_eq!(import.trait_path, "std::borrow::Borrow");
    Ok(())
}

/// A call that no inspected surface resolves names the unknown-surface imports as its candidates.
#[test]
fn unresolved_method_call_records_unknown_surface_imports_as_candidates() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(BORROWED_PATH)?;
    assert_eq!(
        candidates_for(&checker, BORROWED_PATH, "value.borrow()"),
        Some(vec!["Borrow".to_string()])
    );
    Ok(())
}

/// The candidate is the local binding, so an aliased import is retained under its alias.
#[test]
fn aliased_import_is_recorded_under_its_binding() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::std::borrow import Borrow as Borrowed
from rust::std::path import PathBuf

pub def borrowed(value: &PathBuf) -> &PathBuf:
    return value.borrow()
"#;
    let checker = checked(source)?;
    assert_eq!(
        candidates_for(&checker, source, "value.borrow()"),
        Some(vec!["Borrowed".to_string()])
    );
    Ok(())
}

/// A known-surface import that does not declare the method is not a candidate for it.
#[test]
fn known_surface_import_without_the_method_is_not_a_candidate() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::std::borrow import Borrow
from rust::std::io import Read
from rust::std::path import PathBuf

pub def borrowed(value: &PathBuf) -> &PathBuf:
    return value.borrow()
"#;
    let checker = checked(source)?;
    let read = checker
        .type_info()
        .rust
        .trait_imports
        .get("Read")
        .ok_or("the fallback vocabulary must register `Read`")?;
    assert!(read.methods_known, "{read:?}");
    assert_eq!(
        candidates_for(&checker, source, "value.borrow()"),
        Some(vec!["Borrow".to_string()])
    );
    Ok(())
}

/// A call an import with a declared surface claims records no candidates.
#[test]
fn claimed_method_call_records_no_candidates() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::std::borrow import Borrow
from rust::std::io import Read
from rust::std::fs import File

pub def drain(mut file: File) -> int:
    mut out: bytes = b""
    file.read_to_end(out)
    return len(out)
"#;
    let checker = checked(source)?;
    assert_eq!(candidates_for(&checker, source, "file.read_to_end(out)"), None);
    assert!(
        checker
            .type_info()
            .rust
            .method_trait_import_uses
            .values()
            .any(|import_use| import_use.binding == "Read"),
        "the fallback-vocabulary trait that declares the method claims the call"
    );
    Ok(())
}

/// A program without an unresolved method call records no candidates, so an unused import stays prunable.
#[test]
fn program_without_unresolved_calls_records_no_candidates() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::std::borrow import Borrow
from rust::std::path import PathBuf

pub def keep(value: PathBuf) -> PathBuf:
    return value
"#;
    let checker = checked(source)?;
    assert!(
        checker.type_info().rust.method_trait_import_candidates.is_empty(),
        "{:?}",
        checker.type_info().rust.method_trait_import_candidates
    );
    Ok(())
}
