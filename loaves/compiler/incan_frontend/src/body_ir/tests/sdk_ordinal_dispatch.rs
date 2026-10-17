//! Checked scalar SDK trait receivers retain Self independently of their physical native carriers.

use super::{IncanType, TypeCheckInfo, bir, body_named, build, library_call_delegations, named_targets};
use crate::typechecker::stdlib_loader::StdlibAstCache;

/// A generic ordinal call retains its lexical Self binder, while the associated call keeps its existing type receiver.
#[test]
fn ordinal_calls_retain_checked_self_types() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.collections import OrdinalKey\n\npub def key_bytes[T with OrdinalKey](value: T) -> bytes:\n    return value.ordinal_bytes()\n\npub def key_encoding[T with OrdinalKey](_value: T) -> str:\n    return T.ordinal_encoding()\n",
        &["main"],
    )?;
    let call = body_named(&module, "key_bytes")?
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("missing ordinal receiver call")?;
    let selected = call.sdk_trait.as_ref().ok_or("missing checked ordinal SDK trait")?;
    let encoded = serde_json::to_value(selected)?;
    assert_eq!(encoded.get("receiver_type"), Some(&serde_json::json!({"TypeVar": "T"})));
    let associated = named_targets(&module, "key_encoding");
    assert_eq!(
        associated
            .first()
            .ok_or("missing associated ordinal call")?
            .receiver_type,
        Some(IncanType::TypeVar("T".to_owned()))
    );
    let mut older = encoded;
    older
        .as_object_mut()
        .ok_or("SDK dispatch is not an object")?
        .remove("receiver_type");
    let older: bir::StdlibTraitCall = serde_json::from_value(older)?;
    assert_eq!(
        serde_json::to_value(older)?.get("receiver_type"),
        Some(&serde_json::Value::Null)
    );
    Ok(())
}

/// The exact SDK trait's associated slot has no value receiver and retains Self as its native implementation binder.
#[test]
fn ordinal_associated_slot_retains_native_self_signature() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let declaration = cache
        .lookup_trait(&["std".to_owned(), "collections".to_owned()], "OrdinalKey")
        .ok_or("missing ordinal trait")?;
    let identity = declaration
        .methods
        .get("ordinal_encoding")
        .and_then(|method| method.identity.clone())
        .ok_or("missing ordinal associated identity")?;
    let mut facts = TypeCheckInfo::default();
    facts.references.resolved_identities.insert((1, 2), identity.clone());
    let declarations = library_call_delegations(&facts);
    let call = declarations
        .iter()
        .find(|call| call.canonical == identity)
        .ok_or("missing ordinal associated native signature")?;
    assert!(call.parameters.is_empty());
    assert_eq!(call.type_parameters, ["Self"]);
    assert_eq!(
        call.rust_path().as_deref(),
        Some("incan_stdlib_data::collections::OrdinalKey::ordinal_encoding")
    );
    Ok(())
}
