//! Checked SDK trait dispatch remains distinct from same-named inherent method signatures.

use super::*;
use incan_semantics_core::executable_representation::{decode_module, encode_module};
use std::collections::BTreeSet;

/// Inspect every selected read/write declaration at the exact call identity rather than choosing an SDK method by name.
fn selected_native_widths(
    module: &bir::BodyIrModule,
    method: &str,
    path: &str,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let body = body_named(module, "probe")?;
    let mut widths = BTreeSet::new();
    for statement in &body.block.stmts {
        let bir::StatementKind::Call {
            callee: bir::Callee::Method(target),
            ..
        } = &statement.kind
        else {
            continue;
        };
        if target.name != method {
            continue;
        }
        let encoded = serde_json::to_value(target)?;
        let declaration: bir::StdlibDelegation = serde_json::from_value(encoded["native_trait_delegation"].clone())
            .map_err(|error| {
                std::io::Error::other(format!("selected SDK trait declaration missing: {error}; {encoded}"))
            })?;
        assert_eq!(declaration.rust_path().as_deref(), Some(path));
        assert_eq!(
            declaration.native_type_arguments.len(),
            2,
            "the native trait call needs Self and its width: {declaration:#?}"
        );
        widths.insert(declaration.native_type_arguments[1].to_string());
    }
    Ok(widths)
}

/// Overloaded SDK writes retain legacy's BinaryWrite instantiation separately for u8 and u16.
#[test]
fn sdk_binary_write_retains_selected_native_trait_width() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.io import BytesIO, Endian, IoError\n\ndef probe() -> Result[None, IoError]:\n    stream = BytesIO(b\"\")\n    small: u8 = 2\n    word: u16 = 772\n    stream.write(small, Endian.Little)?\n    stream.write(word, Endian.Little)?\n    return Ok(None)\n",
        &["selected_write"],
    )?;
    assert_eq!(
        selected_native_widths(&module, "write", "incan_stdlib_system::io::BinaryWrite::write")?,
        BTreeSet::from(["u8".to_owned(), "u16".to_owned()])
    );
    Ok(())
}

/// Expected result types select distinct BinaryRead instantiations without changing the legacy inference snapshot.
#[test]
fn sdk_binary_read_retains_selected_native_trait_width() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.io import BytesIO, Endian, IoError\n\ndef probe() -> Result[None, IoError]:\n    stream = BytesIO(b\"abcd\")\n    small: u8 = stream.read(Endian.Little)?\n    word: u16 = stream.read(Endian.Little)?\n    println(small)\n    println(word)\n    return Ok(None)\n",
        &["selected_read"],
    )?;
    assert_eq!(
        selected_native_widths(&module, "read", "incan_stdlib_system::io::BinaryRead::read")?,
        BTreeSet::from(["u8".to_owned(), "u16".to_owned()])
    );
    Ok(())
}

/// Calls through declared bounds preserve the trait's chosen width while leaving Self for native specialization.
#[test]
fn sdk_binary_bounds_retain_selected_native_trait_width() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.io import BinaryRead, BinaryWrite, Endian, IoError\n\ndef probe[W with BinaryWrite[u32], R with BinaryRead[u16]](writer: W, reader: R, item: u32) -> Result[u16, IoError]:\n    writer.write(item, Endian.Big)?\n    return reader.read(Endian.Little)\n",
        &["selected_bounds"],
    )?;
    assert_eq!(
        selected_native_widths(&module, "write", "incan_stdlib_system::io::BinaryWrite::write")?,
        BTreeSet::from(["u32".to_owned()])
    );
    assert_eq!(
        selected_native_widths(&module, "read", "incan_stdlib_system::io::BinaryRead::read")?,
        BTreeSet::from(["u16".to_owned()])
    );
    Ok(())
}

/// A source method with the same spelling grants no SDK trait dispatch, even when it takes a numeric width.
#[test]
fn source_write_does_not_gain_a_native_sdk_trait_call() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Local:\n    value: int\n\n    def write(self, item: u8) -> u8:\n        return item\n\ndef probe(value: Local, item: u8) -> u8:\n    return value.write(item)\n",
        &["source_write"],
    )?;
    let body = body_named(&module, "probe")?;
    let target = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } if target.name == "write" => Some(target),
            _ => None,
        })
        .ok_or("source write target missing")?;
    assert!(serde_json::to_value(target)?["native_trait_delegation"].is_null());
    Ok(())
}
/// A source method without a selected SDK delegation retains every positional field in the executable wire form.
#[test]
fn unselected_method_target_roundtrips() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Item:\n    value: int\n    def read(self) -> int:\n        return self.value\n\ndef main() -> int:\n    return Item(value=7).read()\n",
        &["unselected_method_wire"],
    )?;
    assert!(
        body_named(&module, "main")?
            .block
            .stmts
            .iter()
            .any(|statement| matches!(
                &statement.kind,
                bir::StatementKind::Call { callee: bir::Callee::Method(target), .. }
                    if target.native_trait_delegation.is_none()
            ))
    );
    assert_executable_roundtrip(&module)
}

/// Empty native and source binder vectors remain explicit positions even when a stdlib wrapper is nongeneric.
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
    assert_executable_roundtrip(&module)
}

/// A selected native trait signature survives publication with its empty defaults and checked native instantiation.
#[test]
fn selected_sdk_method_target_roundtrips() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.io import BytesIO, Endian, IoError\n\ndef probe() -> Result[None, IoError]:\n    stream = BytesIO(b\"\")\n    value: u8 = 2\n    stream.write(value, Endian.Little)?\n    return Ok(None)\n",
        &["selected_method_wire"],
    )?;
    assert!(
        body_named(&module, "probe")?
            .block
            .stmts
            .iter()
            .any(|statement| matches!(
                &statement.kind,
                bir::StatementKind::Call { callee: bir::Callee::Method(target), .. }
                    if target.native_trait_delegation.is_some()
            ))
    );
    assert_executable_roundtrip(&module)
}

/// Exercise the production positional codec without projecting away the checked callable records being tested.
fn assert_executable_roundtrip(module: &bir::BodyIrModule) -> Result<(), Box<dyn std::error::Error>> {
    let encoded = encode_module(module)?;
    assert_eq!(&decode_module(&encoded)?, module);
    Ok(())
}
