//! SDK type dependencies retain declaring identities even without consumer imports.

use super::*;
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_semantics_core::SymbolOrigin;

/// The SDK's error layout names its Codec field type in its own module, independently of consumer bindings.
#[test]
fn stdlib_layout_fields_retain_nested_type_identities() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "compression".to_owned(), "_core".to_owned()];
    let mut identity = cache
        .lookup_identity(&path, "CompressionError")
        .ok_or("missing error identity")?;
    identity.origin = SymbolOrigin::Package {
        library: "incan_stdlib_compression".to_owned(),
        module_path: path[1..].to_vec(),
    };
    let mut info = TypeCheckInfo::default();
    info.declarations
        .named_type_identities
        .insert("CompressionError".to_owned(), identity);
    let identities = library_type_identities(&info);
    assert!(
        identities.iter().any(|identity| identity.declaration_name == "Codec"),
        "a nested SDK layout must retain Codec: {identities:?}"
    );
    Ok(())
}

/// A checked source-module binding to a generic SDK model retains its owner binder and field types.
#[test]
fn stdlib_generic_model_retains_declared_type_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "collections".to_owned()];
    let identity = cache.lookup_identity(&path, "Deque").ok_or("missing deque identity")?;
    let mut info = TypeCheckInfo::default();
    info.declarations
        .named_type_identities
        .insert("Deque".to_owned(), identity);
    let declarations = library_nominal_declarations(&info);
    let deque = declarations
        .iter()
        .find(|declaration| declaration.name == "Deque")
        .ok_or("missing SDK deque")?;
    assert_eq!(deque.type_parameters, vec!["T"]);
    assert_eq!(deque.type_parameter_count, 1);
    assert!(matches!(deque.canonical.origin, SymbolOrigin::Package { .. }));
    assert_eq!(deque.fields, vec!["items"]);
    Ok(())
}

/// A collection's length follows the selected method identity instead of the builtin vector operation.
#[test]
fn stdlib_collection_length_retains_selected_method() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.collections import Deque\n\ndef main() -> int:\n    queue = Deque[int].from_iter([1])\n    return len(queue)\n",
        &["m", "sdk_length"],
    )?;
    let main = body_named(&module, "main")?;
    assert!(main.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Call { callee: bir::Callee::Method(target), .. }
            if target.name == "__len__" && target.canonical.as_ref().is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Method)
    )), "length must retain its checked method: {}", main.render_snapshot());
    Ok(())
}

/// A stdlib trait default's return type retains its declaring module without a consumer import of that type.
#[test]
fn stdlib_trait_signature_retains_its_nominal_return_type() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "io".to_owned()];
    let reader = cache
        .lookup_trait(&path, "BinaryReader")
        .ok_or("missing reader trait")?;
    let method = reader.methods.get("chunks").ok_or("missing chunk method")?;
    let identity = method.identity.clone().ok_or("missing chunk identity")?;
    let mut info = TypeCheckInfo::default();
    info.references.resolved_identities.insert((1, 2), identity);
    let declarations = library_nominal_declarations(&info);
    assert!(
        declarations
            .iter()
            .any(|declaration| declaration.name == "ReaderChunks"),
        "the default's signature must retain ReaderChunks: {declarations:?}"
    );
    Ok(())
}

/// A checked SDK trait method delegates through its owning trait with a concrete Self binder.
#[test]
fn stdlib_trait_method_retains_its_sdk_call_signature() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "io".to_owned()];
    let reader = cache
        .lookup_trait(&path, "BinaryReader")
        .ok_or("missing reader trait")?;
    let identity = reader
        .methods
        .get("chunks")
        .and_then(|method| method.identity.clone())
        .ok_or("missing chunks identity")?;
    let mut info = TypeCheckInfo::default();
    info.references.resolved_identities.insert((1, 2), identity);
    let calls = library_call_delegations(&info);
    let call = calls
        .iter()
        .find(|call| call.canonical.declaration_name == "chunks")
        .ok_or("missing SDK trait call")?;
    assert_eq!(call.type_parameters, vec!["Self"]);
    assert_eq!(
        call.parameters[0],
        IncanType::Ref(Box::new(IncanType::TypeVar("Self".to_owned())))
    );
    assert_eq!(
        call.return_type,
        IncanType::Generic {
            base: "ReaderChunks".to_owned(),
            args: vec![IncanType::TypeVar("Self".to_owned())],
        }
    );
    assert_eq!(
        call.rust_path().as_deref(),
        Some("incan_stdlib_system::io::BinaryReader::chunks")
    );
    Ok(())
}

/// Frozen byte length keeps its canonical builtin receiver owner, independent of mutable byte-list methods.
#[test]
fn frozen_byte_length_retains_its_builtin_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def size(value: FrozenBytes) -> int:\n    return value.len()\n",
        &["m", "frozen_length"],
    )?;
    let body = body_named(&module, "size")?;
    let method = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => target.canonical.as_ref(),
            _ => None,
        })
        .ok_or("missing frozen byte method identity")?;
    assert_eq!(method.origin, SymbolOrigin::Builtin);
    assert_eq!(method.declaration_name, "frozenbytes.len", "{}", body.render_snapshot());
    Ok(())
}

/// Reading a frozen byte const retains its checked storage carrier instead of erasing it to mutable bytes.
#[test]
fn frozen_byte_const_reads_keep_their_checked_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "const DATA: FrozenBytes = b\"ab\"\n\ndef consume(value: FrozenBytes) -> None:\n    pass\n\ndef main() -> None:\n    consume(DATA)\n    DATA.len()\n",
        &["m", "frozen_read"],
    )?;
    let body = body_named(&module, "main")?;
    let receiver = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call { args, .. } => args.first().and_then(|argument| argument.as_one()),
            _ => None,
        })
        .ok_or("missing frozen byte argument")?;
    let bir::Operand::Place(read) = receiver else {
        return Err(format!("frozen storage was erased: {}", body.render_snapshot()).into());
    };
    let local = read.place.local_id().ok_or("frozen read has no local")?;
    assert_eq!(
        body.locals[local.index()].ty,
        IncanType::Primitive(IncanPrimitiveType::FrozenBytes)
    );
    let method_receiver = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(_),
                args,
                ..
            } => args.first().and_then(|argument| argument.as_one()),
            _ => None,
        })
        .ok_or("missing const method receiver")?;
    let bir::Operand::Place(read) = method_receiver else {
        return Err("const method receiver has no checked place".into());
    };
    assert!(
        read.place.local_id().is_some(),
        "const method receiver must materialize its value"
    );
    Ok(())
}

/// Implicit reflection result types retain the core runtime owner even without a FieldInfo import.
#[test]
fn reflection_metadata_retains_native_layout() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Record:\n    value: int\n\ndef main() -> None:\n    for info in Record(value=1).__fields__():\n        println(info.name)\n";
    let module = build(source, &["m", "native_reflection"])?;
    let declaration = module
        .nominal_declarations
        .iter()
        .find(|value| value.name == "FieldInfo")
        .ok_or("missing native reflection layout")?;
    let encoded = serde_json::to_value(declaration)?;
    assert_eq!(encoded["native_item"], "incan_std_core::reflection::FieldInfo");
    assert_eq!(
        declaration.fields,
        [
            "name",
            "alias",
            "description",
            "wire_name",
            "type_name",
            "has_default",
            "extra"
        ]
    );
    Ok(())
}

/// A source declaration named FieldInfo cannot acquire the runtime reflection representation.
#[test]
fn user_metadata_name_keeps_its_source_layout() -> Result<(), Box<dyn std::error::Error>> {
    let module = build("model FieldInfo:\n    label: str\n", &["m", "user_metadata"])?;
    let declaration = module.nominal_declarations.first().ok_or("missing user layout")?;
    let encoded = serde_json::to_value(declaration)?;
    assert!(encoded["native_item"].is_null() || encoded["native_item"] == "");
    assert_eq!(declaration.fields, ["label"]);
    Ok(())
}
