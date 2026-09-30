//! A trait-qualified Rust call on a receiver of unknown type stays as open as it is without metadata (#1561).
//!
//! The guard `borrow_mut()` returns through an uninspected `Rc[RefCell[...]]` has no type the checker knows. With
//! `std::io::Read` inspected, `Read.by_ref(guard)` records no receiver borrow, so the backend keeps its reborrow
//! through the guard, and the call's result is of unknown type, so `take` and `read_to_end` stay open. The standard
//! library's `BytesIO.read`, `File.read` and compression streams read this way, and a consumer that checks them from
//! source holds that inspection.

use incan_frontend::test_support::seeded_rust_inspect_workspace;
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};
use incan_lang::interop::{
    RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustTraitAssoc, RustTraitInfo, RustVisibility,
};

use super::generated_programs::{run_checked_with_stdlib, run_with_stdlib};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Reads of an in-memory buffer through the guard its `RefCell` hands out: bounded by `take` as `BytesIO.read` does,
/// and through an adapter that takes the reader `Read.by_ref` returns as the compression streams do.
const BOUNDED_READS: &str = r#"
from rust::std::cell import RefCell
from rust::std::io import BufReader, Cursor, Read
from rust::std::rc import Rc


class Buffer:
    handle: Rc[RefCell[Cursor[bytes]]]

    def read(self, size: int) -> bytes:
        mut out: bytes = b""
        mut cursor = self.handle.borrow_mut()
        match Read.by_ref(cursor).take(size).read_to_end(out):
            Ok(_) => return out
            Err(_) => return b""

    def read_buffered(self) -> bytes:
        mut out: bytes = b""
        mut cursor = self.handle.borrow_mut()
        mut reader = BufReader.new(Read.by_ref(cursor))
        match reader.read_to_end(out):
            Ok(_) => return out
            Err(_) => return b""


def main() -> None:
    buffer = Buffer(handle=Rc.new(RefCell.new(Cursor.new(b"abcdef"))))
    println(len(buffer.read(4)))
    println(len(buffer.read(4)))
    other = Buffer(handle=Rc.new(RefCell.new(Cursor.new(b"xyz"))))
    println(len(other.read_buffered()))
"#;

/// A `std::io::Read` method signature whose first parameter is the declared receiver.
fn read_method(name: &str, receiver: &str, params: &[(&str, &str)], return_type: &str) -> RustTraitAssoc {
    let receiver = RustParam {
        name: Some("self".to_string()),
        type_display: receiver.to_string(),
    };
    RustTraitAssoc::Function {
        name: name.to_string(),
        signature: RustFunctionSig {
            receiver_contract: None,
            type_params: Vec::new(),
            params: std::iter::once(receiver)
                .chain(params.iter().map(|(name, ty)| RustParam {
                    name: Some((*name).to_string()),
                    type_display: (*ty).to_string(),
                }))
                .collect(),
            return_type: return_type.to_string(),
            is_async: false,
            is_unsafe: false,
        },
    }
}

/// The program builds and runs with `std::io::Read` inspected, as it does without any Rust metadata.
#[test]
fn bounded_reads_through_an_untyped_guard_build_with_read_inspected_issue1561() -> TestResult {
    let tokens = lexer::lex(BOUNDED_READS).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let workspace = seeded_rust_inspect_workspace()?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".to_string()]));
    checker.set_rust_inspect_manifest_dir(workspace.path().to_path_buf());
    checker.rust_inspect_cache.insert_test_item(
        workspace.path(),
        RustItemMetadata {
            canonical_path: "std::io::Read".to_string(),
            definition_path: Some("std::io::Read".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Trait(RustTraitInfo {
                items: vec![
                    read_method("read_to_end", "&mut self", &[("buf", "&mut Vec<u8>")], "Result<usize>"),
                    read_method("by_ref", "&mut self", &[], "&mut Self"),
                    read_method("take", "self", &[("limit", "u64")], "Take<Self>"),
                ],
                derive_macro: None,
            }),
        },
    )?;
    checker
        .check_with_imports(&program, &[])
        .map_err(|errors| format!("check failed: {errors:?}"))?;
    assert_eq!(run_checked_with_stdlib(&program, &checker)?, "4\n2\n3\n");
    assert_eq!(run_with_stdlib(BOUNDED_READS)?, "4\n2\n3\n");
    Ok(())
}
