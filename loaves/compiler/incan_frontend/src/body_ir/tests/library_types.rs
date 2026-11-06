//! SDK type dependencies retain declaring identities even without consumer imports.

use super::*;
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_semantics_core::SymbolOrigin;

/// The error type in ReaderChunks' checked FallibleIterator adoption is a dependency even without a field or import.
#[test]
fn sdk_trait_arguments_retain_nominal_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let identity = cache
        .lookup_identity(&["std".to_owned(), "io".to_owned()], "ReaderChunks")
        .ok_or("missing reader identity")?;
    let mut facts = TypeCheckInfo::default();
    facts
        .declarations
        .named_type_identities
        .insert("ReaderChunks".to_owned(), identity);
    let layouts = library_nominal_declarations(&facts);
    assert!(layouts.iter().any(|layout| layout.name == "IoError"), "{layouts:?}");
    Ok(())
}

/// A closed source function call already supplies the reader argument; nominal admission must not infer it again.
#[test]
fn sdk_reader_call_retains_checked_function_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.io import BinaryReader, BytesIO\n\ndef chunk_size[R with BinaryReader](reader: R) -> int:\n    return reader.chunks(2).chunk_size\n\ndef main() -> None:\n    println(chunk_size(BytesIO(b\"abcde\")))\n",
        &["m", "reader_instance"],
    )?;
    let owner = body_named(&module, "chunk_size")?
        .canonical
        .as_ref()
        .ok_or("missing function identity")?;
    let target = named_targets(&module, "main")
        .into_iter()
        .find(|target| target.canonical.as_ref() == Some(owner))
        .ok_or("missing checked reader call")?;
    assert_eq!(target.type_args, vec![IncanType::Named("_BytesIO".to_owned())]);
    Ok(())
}

/// A checked SDK signature and its semantic payloads alone cannot authorize an anonymous native enum alias.
#[test]
fn sdk_union_signature_requires_a_selected_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.fs import Path\n\ndef compare(first: Path, target: Path | str) -> None:\n    first.samefile(target)\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let path = vec!["m".to_owned(), "unadmitted_union".to_owned()];
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut module = build_body_ir_module_v0(&program, &path, checker.type_info());
    module.stdlib_delegations = library_call_delegations(checker.type_info());
    module
        .nominal_declarations
        .extend(library_nominal_declarations(checker.type_info()));
    assert!(
        module
            .stdlib_delegations
            .iter()
            .flat_map(|call| &call.parameters)
            .any(|ty| { matches!(ty, IncanType::Generic { base, .. } if base == "Union") }),
        "the negative must actually request an SDK union"
    );
    let declarations = library_native_union_declarations(&ProviderPlan::default(), &[module])?;
    assert!(declarations.is_empty(), "semantic shape must not mint native ownership");
    Ok(())
}

/// Older serialized modules omit native union facts and must load with an empty inventory, never inferred ownership.
#[test]
fn native_union_inventory_defaults_for_older_modules() -> Result<(), Box<dyn std::error::Error>> {
    let module = build("def main() -> None:\n    pass\n", &["m", "old_union_inventory"])?;
    let mut wire = serde_json::to_value(&module)?;
    assert_eq!(wire.get("native_union_declarations"), Some(&serde_json::json!([])));
    wire.as_object_mut()
        .ok_or("module is not an object")?
        .remove("native_union_declarations");
    let older: bir::BodyIrModule = serde_json::from_value(wire)?;
    assert_eq!(
        serde_json::to_value(older)?.get("native_union_declarations"),
        Some(&serde_json::json!([]))
    );
    Ok(())
}

/// The SDK's declared Eq and Ord retain the native comparison capabilities that the shared derive registry implies.
#[test]
fn stdlib_uuid_retains_implied_comparison_capabilities() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let identity = cache
        .lookup_identity(&["std".to_owned(), "uuid".to_owned()], "UUID")
        .ok_or("missing UUID identity")?;
    let mut facts = TypeCheckInfo::default();
    facts
        .declarations
        .named_type_identities
        .insert("UUID".to_owned(), identity);
    let layouts = library_nominal_declarations(&facts);
    let uuid = layouts
        .iter()
        .find(|layout| layout.name == "UUID")
        .ok_or("missing SDK UUID layout")?;
    assert!(
        uuid.derives.iter().any(|name| name == "PartialEq"),
        "{:?}",
        uuid.derives
    );
    assert!(
        uuid.derives.iter().any(|name| name == "PartialOrd"),
        "{:?}",
        uuid.derives
    );
    Ok(())
}
/// Methods on errors returned by SDK functions retain the declaration that supplies their compiled body.
#[test]
fn sdk_returned_error_retains_message_delegation() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "hash".to_owned(), "_core".to_owned()];
    let model = cache.lookup_type(&path, "HashError").ok_or("missing HashError")?;
    let crate::symbols::TypeInfo::Model(model) = model else {
        return Err("not a model".into());
    };
    let mut identity = model
        .methods
        .get("message")
        .and_then(|method| method.identity.clone())
        .ok_or("missing message")?;
    identity.origin = SymbolOrigin::Package {
        library: "incan_stdlib_data".to_owned(),
        module_path: path[1..].to_vec(),
    };
    let mut info = TypeCheckInfo::default();
    info.references.resolved_identities.insert((1, 2), identity);
    let calls = library_call_delegations(&info);
    assert!(
        calls.iter().any(|call| call.canonical.declaration_name == "message"),
        "missing message delegation: {calls:#?}"
    );
    Ok(())
}

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
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let declarations = library_nominal_declarations(checker.type_info());
    let declaration = declarations
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
    assert_eq!(
        declaration.field_types[0],
        IncanType::Primitive(IncanPrimitiveType::FrozenStr)
    );
    let module = build(source, &["m", "native_reflection"])?;
    let main = body_named(&module, "main")?;
    let loop_body = main
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .ok_or("missing metadata iteration")?;
    let projection = loop_body
        .stmts
        .iter()
        .find_map(|statement| {
            let bir::StatementKind::Call { args, .. } = &statement.kind else {
                return None;
            };
            args.iter()
                .filter_map(bir::ArgumentElement::as_one)
                .find_map(|argument| {
                    let bir::Operand::Place(read) = argument else {
                        return None;
                    };
                    read.place
                        .projection
                        .iter()
                        .find(|projection| matches!(projection, bir::PlaceElem::Field { name, .. } if name == "name"))
                })
        })
        .ok_or("missing metadata field read")?;
    assert!(
        matches!(projection, bir::PlaceElem::Field { canonical: Some(identity), .. }
        if identity == &declaration.field_identities[0]),
        "{projection:?}"
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
/// The checked SDK error adapter requires its actual package factory body rather than a coerced native opaque return.
#[test]
fn sdk_opaque_factory_retains_body_requirement() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/std_fallible_adapter_loop.incn"
    );
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["factory_requirements".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let requirements = library_factory_body_requirements(checker.type_info());
    assert!(
        requirements
            .iter()
            .any(|identity| identity.declaration_name == "map_err"
                && matches!(&identity.origin, SymbolOrigin::Package { library, .. } if library == "incan_stdlib_core")),
        "{requirements:?}"
    );
    assert!(
        !requirements
            .iter()
            .any(|identity| identity.declaration_name == "chunks"),
        "{requirements:?}"
    );
    Ok(())
}

/// A local same-spelled method acquires no SDK package-body requirement.
#[test]
fn source_factory_spelling_does_not_require_sdk_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Local:\n  value: str\n  def map_err(self) -> Local:\n    return Local(value=self.value)\ndef main() -> None:\n  println(Local(value=\"source\").map_err().value)\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["source_factory_spelling".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    assert!(library_factory_body_requirements(checker.type_info()).is_empty());
    Ok(())
}
