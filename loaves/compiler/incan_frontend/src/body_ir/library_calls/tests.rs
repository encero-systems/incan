//! Crate-call records built from stdlib declarations: what each parameter takes when a call omits it.

use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use super::library_call;
use crate::symbols::TypeInfo;
use crate::typechecker::stdlib_loader::StdlibAstCache;

/// Published generic owner methods retain lexical trait binders without retagging a same-spelled nominal argument.
#[test]
fn sdk_trait_arguments_retain_only_declared_lexical_binders() -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::IncanType;
    let mut cache = StdlibAstCache::new();
    let path = source_path(&["derives", "collection"]);
    let owner = cache
        .lookup_trait(&path, "FallibleIterator")
        .ok_or("missing standard protocol")?;
    let identity = owner
        .methods
        .get("__next__")
        .and_then(|method| method.identity.clone())
        .ok_or("missing poll slot")?;
    let dispatch = crate::typechecker::ResolvedMethodDispatch::Trait {
        trait_name: "FallibleIterator".to_owned(),
        module_path: Some(path),
        type_args: vec![
            crate::symbols::ResolvedType::Named("T".to_owned()),
            crate::symbols::ResolvedType::Named("E".to_owned()),
        ],
        implementation_type_params: Vec::new(),
        receiver_is_mutable: true,
    };
    let binders = vec!["T".to_owned(), "E".to_owned()];
    let retained = super::trait_dispatch(
        &identity,
        Some(&dispatch),
        Some(IncanType::TypeVar("Source".to_owned())),
        &binders,
    )
    .ok_or("missing checked poll dispatch")?;
    assert_eq!(
        retained.type_arguments,
        [IncanType::TypeVar("T".to_owned()), IncanType::TypeVar("E".to_owned())]
    );
    let nominal = super::trait_dispatch(&identity, Some(&dispatch), None, &[]).ok_or("missing nominal dispatch")?;
    assert_eq!(
        nominal.type_arguments,
        [IncanType::Named("T".to_owned()), IncanType::Named("E".to_owned())]
    );
    Ok(())
}

/// Generic adapter slots retain their declaring native trait and ordered Self, trait, and method binders.
#[test]
fn sdk_adapter_slots_retain_generic_native_signatures() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = source_path(&["derives", "collection"]);
    let declaration = cache
        .lookup_trait(&path, "FallibleIterator")
        .ok_or("missing iterator trait")?;
    for (name, parameters) in [
        ("take", vec!["Self", "T", "E"]),
        ("map", vec!["Self", "T", "E", "U", "MapFn"]),
        ("map_err", vec!["Self", "T", "E", "F", "ErrorMap"]),
    ] {
        let method = declaration.methods.get(name).ok_or("missing adapter method")?;
        let identity = method.identity.clone().ok_or("missing adapter identity")?;
        let call = library_call(&identity, &mut cache).ok_or_else(|| format!("missing {name} signature"))?;
        assert_eq!(call.type_parameters, parameters);
        assert_eq!(
            call.rust_path(),
            Some(format!(
                "incan_stdlib_core::derives::collection::FallibleIterator::{name}"
            ))
        );
        assert_eq!(call.canonical, identity);
    }
    Ok(())
}

/// Each explicit Index implementation retains its complete native Self, key, and result instantiation.
#[test]
fn sdk_index_implementations_retain_native_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let Some(TypeInfo::Newtype(value)) = cache.lookup_type(&source_path(&["json"]), "JsonValue") else {
        return Err("missing JsonValue".into());
    };
    let methods = value
        .method_overloads
        .get("__getitem__")
        .ok_or("missing Index implementations")?;
    assert_eq!(methods.len(), 2);
    for method in methods {
        let identity = published(
            method.identity.clone().ok_or("missing implementation identity")?,
            "incan_stdlib_data",
            &["json"],
        );
        let call = library_call(&identity, &mut cache).ok_or("missing native Index delegation")?;
        assert_eq!(
            call.rust_path().as_deref(),
            Some("incan_stdlib_core::traits::indexing::Index::__getitem__")
        );
        let target = method.trait_target.as_ref().ok_or("missing checked Index target")?;
        let expected = std::iter::once(super::IncanType::Named("JsonValue".to_owned()))
            .chain(target.type_args.iter().map(super::semantic_type_from_resolved))
            .collect::<Vec<_>>();
        let encoded = serde_json::to_value(&call)?;
        assert_eq!(
            encoded.get("native_type_arguments"),
            Some(&serde_json::to_value(&expected)?)
        );
        assert_eq!(call.canonical, identity);
    }
    Ok(())
}
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

/// Explicit trait implementations have distinct native instantiations and cannot use an inherent method path.
#[test]
fn sdk_trait_implementations_require_trait_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let Some(TypeInfo::Newtype(value)) = cache.lookup_type(&source_path(&["json"]), "JsonValue") else {
        return Err("missing JsonValue".into());
    };
    let methods = value
        .method_overloads
        .get("__getitem__")
        .ok_or("missing indexing implementations")?;
    assert_eq!(methods.len(), 2);
    for method in methods {
        assert!(method.trait_target.is_some());
        let identity = published(
            method.identity.clone().ok_or("missing indexing identity")?,
            "incan_stdlib_data",
            &["json"],
        );
        if let Some(call) = library_call(&identity, &mut cache) {
            assert_eq!(
                call.rust_path().as_deref(),
                Some("incan_stdlib_core::traits::indexing::Index::__getitem__")
            );
        }
    }
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

/// A generic SDK trait slot retains native Self, trait and method binders independently and in declaration order.
#[test]
fn sdk_trait_method_keeps_all_declared_binders() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let info = cache
        .lookup_trait(&source_path(&["derives", "collection"]), "FallibleIterator")
        .ok_or("source FallibleIterator trait absent")?;
    let mut identity = info
        .methods
        .get("map_err")
        .and_then(|method| method.identity.clone())
        .ok_or("source map_err slot absent")?;
    let SymbolOrigin::Module(path) = &identity.origin else {
        return Err("source map_err slot has no declaring module".into());
    };
    identity.origin = SymbolOrigin::Package {
        library: "incan_stdlib_core".to_owned(),
        module_path: path[1..].to_vec(),
    };
    let record = library_call(&identity, &mut cache).ok_or("generic SDK trait method has no call record")?;
    assert_eq!(record.canonical, identity);
    assert_eq!(record.type_parameters, ["Self", "T", "E", "F", "ErrorMap"]);
    assert_eq!(
        record.parameters,
        [
            incan_semantics_core::IncanType::Ref(Box::new(incan_semantics_core::IncanType::TypeVar("Self".into()))),
            incan_semantics_core::IncanType::TypeVar("ErrorMap".into()),
        ]
    );
    assert_eq!(
        record.return_type,
        incan_semantics_core::IncanType::Generic {
            base: "FallibleIterator".into(),
            args: vec![
                incan_semantics_core::IncanType::TypeVar("T".into()),
                incan_semantics_core::IncanType::TypeVar("F".into())
            ],
        }
    );
    let mut changed = identity.clone();
    changed.declaration_span.end += 1;
    assert!(library_call(&changed, &mut cache).is_none());
    Ok(())
}
