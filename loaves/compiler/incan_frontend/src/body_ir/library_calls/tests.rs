//! Crate-call records built from stdlib declarations: what each parameter takes when a call omits it.

use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use super::library_call;
use crate::symbols::TypeInfo;
use crate::typechecker::stdlib_loader::StdlibAstCache;

/// A selected inherent overload keeps its exact declaration instead of the cache's last same-named method.
#[test]
fn sdk_method_overload_retains_selected_signature() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = source_path(&["io"]);
    let Some(TypeInfo::Class(class)) = cache.lookup_type(&path, "_BytesIO") else {
        return Err("missing byte stream class".into());
    };
    let methods = class.method_overloads.get("read").ok_or("missing read overloads")?;
    let method = methods
        .iter()
        .find(|method| {
            method
                .params
                .first()
                .is_some_and(|param| param.ty == crate::symbols::ResolvedType::Int)
        })
        .ok_or("missing byte read overload")?;
    let source = method.identity.clone().ok_or("missing byte read identity")?;
    let identity = published(source, "incan_stdlib_system", &["io"]);
    let call = library_call(&identity, &mut cache).ok_or("missing selected byte read delegation")?;
    assert_eq!(call.canonical, identity);
    assert_eq!(
        call.parameters[1],
        super::semantic_type_from_resolved(&crate::symbols::ResolvedType::Int)
    );
    assert_eq!(
        call.return_type,
        super::semantic_type_from_resolved(&method.return_type)
    );
    assert_eq!(
        call.rust_path().as_deref(),
        Some("incan_stdlib_system::io::_BytesIO::read")
    );
    Ok(())
}

/// A Body IR provider-slot identity receives its own crate-call record while legacy keeps the source reference.
#[test]
fn provider_trait_slot_retains_matching_delegation() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = source_path(&["traits", "error"]);
    let error = cache.lookup_trait(&path, "Error").ok_or("Error trait missing")?;
    let source = error
        .methods
        .get("source")
        .and_then(|method| method.identity.clone())
        .ok_or("source slot missing")?;
    let provider = published(source.clone(), "incan_stdlib_core", &["traits", "error"]);
    let mut facts = crate::typechecker::TypeCheckInfo::default();
    facts.references.resolved_identities.insert((1, 2), source.clone());
    facts.traits.provider_slot_identities.insert((1, 2), provider.clone());
    let records = super::collect(&facts);
    assert!(
        records.iter().any(|record| record.canonical == provider),
        "{records:#?}"
    );
    assert_eq!(facts.resolved_identity(crate::ast::Span::new(1, 2)), Some(&source));
    Ok(())
}

/// The source module path of a stdlib module under the `std` root.
fn source_path(module_path: &[&str]) -> Vec<String> {
    std::iter::once("std")
        .chain(module_path.iter().copied())
        .map(str::to_owned)
        .collect()
}

/// Rebase a stdlib source identity onto the SDK package that publishes its module, as a linked SDK names it.
fn published(mut identity: CanonicalSymbolId, library: &str, module_path: &[&str]) -> CanonicalSymbolId {
    identity.origin = SymbolOrigin::Package {
        library: library.to_owned(),
        module_path: module_path.iter().map(|segment| (*segment).to_owned()).collect(),
    };
    identity
}

/// The SDK identity of a stdlib module's function.
fn function_identity(
    cache: &mut StdlibAstCache,
    library: &str,
    module_path: &[&str],
    name: &str,
) -> Result<CanonicalSymbolId, Box<dyn std::error::Error>> {
    let identity = cache
        .lookup_identity(&source_path(module_path), name)
        .ok_or_else(|| format!("missing stdlib function `{name}`"))?;
    Ok(published(identity, library, module_path))
}

/// The SDK identity of a stdlib model's method.
fn method_identity(
    cache: &mut StdlibAstCache,
    library: &str,
    module_path: &[&str],
    owner: &str,
    name: &str,
) -> Result<CanonicalSymbolId, Box<dyn std::error::Error>> {
    let Some(TypeInfo::Model(model)) = cache.lookup_type(&source_path(module_path), owner) else {
        return Err(format!("missing stdlib model `{owner}`").into());
    };
    let identity = model
        .methods
        .get(name)
        .and_then(|method| method.identity.clone())
        .ok_or_else(|| format!("missing stdlib method `{owner}.{name}`"))?;
    Ok(published(identity, library, module_path))
}

/// What the record of one callee holds for each parameter, in its serialized spelling.
fn recorded_defaults(
    identity: &CanonicalSymbolId,
    cache: &mut StdlibAstCache,
) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let call = library_call(identity, cache).ok_or("the callee must have a crate-call record")?;
    let record = serde_json::to_value(&call)?;
    let defaults = record
        .get("parameter_defaults")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("the record must list its parameter defaults: {record}"))?;
    Ok(defaults.clone())
}

/// The constant a recorded default passes, or why it has none.
fn passed_constant(default: &serde_json::Value) -> Result<serde_json::Value, String> {
    let computation = default
        .get("Source")
        .ok_or_else(|| format!("the default must be a closed computation: {default}"))?;
    if computation.get("stmts") != Some(&serde_json::json!([])) {
        return Err(format!("a recorded stdlib default runs no statements: {computation}"));
    }
    computation
        .pointer("/result/Constant")
        .cloned()
        .ok_or_else(|| format!("a recorded stdlib default is a constant: {computation}"))
}

/// A function's literal defaults pass as written, and a default naming a const its module imports passes the value the
/// const's declaring module gives it; a parameter without a default is required.
#[test]
fn a_stdlib_call_record_holds_literal_and_const_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let digest = function_identity(
        &mut cache,
        "incan_stdlib_data",
        &["hash", "_streaming"],
        "reader_digest",
    )?;
    let defaults = recorded_defaults(&digest, &mut cache)?;
    assert_eq!(defaults.len(), 4, "{defaults:?}");
    assert_eq!(defaults[0], serde_json::json!("Required"));
    assert_eq!(defaults[1], serde_json::json!("Required"));
    assert_eq!(passed_constant(&defaults[2])?, serde_json::json!({"Int": 65536}));
    assert_eq!(passed_constant(&defaults[3])?, serde_json::json!({"Int": 0}));

    let stream = function_identity(&mut cache, "incan_stdlib_system", &["io"], "BytesIO")?;
    let defaults = recorded_defaults(&stream, &mut cache)?;
    assert_eq!(defaults.len(), 1, "{defaults:?}");
    assert_eq!(passed_constant(&defaults[0])?, serde_json::json!({"Bytes": []}));

    let assert_eq = function_identity(&mut cache, "incan_stdlib_testing", &["testing"], "assert_eq")?;
    let defaults = recorded_defaults(&assert_eq, &mut cache)?;
    assert_eq!(defaults.len(), 3, "{defaults:?}");
    assert_eq!(
        passed_constant(&defaults[2])?,
        serde_json::json!({"Str": "assertion failed: left != right"})
    );
    Ok(())
}

/// A static method's negated integer default passes as one constant, after its required parameter.
#[test]
fn a_stdlib_static_method_record_folds_a_negated_default() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let invalid = method_identity(
        &mut cache,
        "incan_stdlib_data",
        &["collections"],
        "OrdinalMapError",
        "invalid_key_record",
    )?;
    let defaults = recorded_defaults(&invalid, &mut cache)?;
    assert_eq!(defaults.len(), 2, "{defaults:?}");
    assert_eq!(defaults[0], serde_json::json!("Required"));
    assert_eq!(passed_constant(&defaults[1])?, serde_json::json!({"Int": -1}));
    Ok(())
}

/// A default that computes its value, such as a call, is recorded as unsupported, so a call omitting it refuses.
#[test]
fn a_stdlib_call_record_refuses_a_computed_default() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let decompress = function_identity(
        &mut cache,
        "incan_stdlib_compression",
        &["compression", "_auto"],
        "decompress_auto",
    )?;
    let defaults = recorded_defaults(&decompress, &mut cache)?;
    assert_eq!(defaults.len(), 2, "{defaults:?}");
    assert_eq!(defaults[0], serde_json::json!("Required"));
    assert!(defaults[1].get("Unsupported").is_some(), "{defaults:?}");
    Ok(())
}
