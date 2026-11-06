//! Optional SDK defaults retain their declaring parameter's checked carrier.

use super::{TypeChecker, bir, lexer, library_call_delegations, parser};
use incan_semantics_core::{IncanPrimitiveType, IncanType};

/// A completed outer call releases its pending read, so a following final SDK input still transfers ownership.
#[test]
fn completed_outer_call_releases_pending_read() -> Result<(), Box<dyn std::error::Error>> {
    let module = super::build(
        "from std.compression.snappy.raw import compress, decompress\nfrom std.compression import CompressionError\n\ndef show(before: bytes, after: bytes) -> None:\n    println(before == after)\n\ndef check() -> Result[bytes, CompressionError]:\n    data = b\"x\"\n    show(data, decompress(compress(data)?)?)\n    return compress(data)\n",
        &["m", "released_sdk_read"],
    )?;
    let body = super::body_named(&module, "check")?;
    let read = body
        .block
        .stmts
        .iter()
        .rev()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                args,
                ..
            } if target.name == "compress" => args.first().and_then(bir::ArgumentElement::as_one),
            _ => None,
        })
        .ok_or("missing final SDK input")?;
    let bir::Operand::Place(read) = read else {
        return Err("SDK input is not a retained read".into());
    };
    assert_eq!(read.fact, bir::OwnershipFact::Move, "{}", body.render_snapshot());
    assert!(read.last_use, "{}", body.render_snapshot());
    Ok(())
}

/// An earlier outer-call read stays alive while a later argument's nested SDK call takes an owned value.
#[test]
fn nested_sdk_argument_preserves_pending_outer_read() -> Result<(), Box<dyn std::error::Error>> {
    let module = super::build(
        "from std.compression.snappy.raw import compress, decompress\nfrom std.compression import CompressionError\n\ndef show(before: bytes, after: bytes) -> None:\n    println(before == after)\n\ndef check() -> Result[None, CompressionError]:\n    data = b\"x\"\n    show(data, decompress(compress(data)?)?)\n    return Ok(None)\n",
        &["m", "pending_sdk_read"],
    )?;
    let body = super::body_named(&module, "check")?;
    let read = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                args,
                ..
            } if target.name == "compress" => args.first().and_then(bir::ArgumentElement::as_one),
            _ => None,
        })
        .ok_or("missing nested SDK input")?;
    let bir::Operand::Place(read) = read else {
        return Err("SDK input is not a retained read".into());
    };
    assert_eq!(read.fact, bir::OwnershipFact::Clone, "{}", body.render_snapshot());
    assert!(!read.last_use, "{}", body.render_snapshot());
    Ok(())
}

/// A compression signature's imported File and byte stream remain owned by the system SDK.
#[test]
fn compression_stream_retains_system_signature_members() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.compression.gzip import compress_stream\nfrom std.io import BytesIO\n\ndef main() -> None:\n    compress_stream(BytesIO(), BytesIO(), None, 2)\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let identities = super::super::library_type_identities(checker.type_info());
    for name in ["File", "_BytesIO"] {
        assert!(identities.iter().any(|identity| identity.declaration_name == name
            && matches!(&identity.origin, incan_semantics_core::SymbolOrigin::Package { library, .. } if library == "incan_stdlib_system")),
            "missing {name} system identity: {identities:?}");
    }
    Ok(())
}

/// Implicit SDK Error formatting keeps the exact checked message identity in its native call inventory.
#[test]
fn sdk_error_display_retains_selected_message() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.io import IoError\n\ndef main() -> None:\n    error = IoError(kind=\"io\", detail=\"detail\")\n    println(error)\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let selected = checker
        .type_info()
        .calls
        .error_message_displays
        .values()
        .filter_map(|display| display.identity.as_ref())
        .find(|identity| identity.declaration_name == "message")
        .ok_or("missing checked SDK display selection")?;
    let calls = library_call_delegations(checker.type_info());
    assert!(
        calls.iter().any(|call| &call.canonical == selected),
        "selected={selected:?}; calls={calls:?}"
    );
    Ok(())
}

/// A checked SDK enum's static all() call keeps a native signature rather than an unresolved imported target.
#[test]
fn codec_all_retains_selected_sdk_method() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.compression import Codec\n\ndef main() -> None:\n    codecs = Codec.all()\n    println(len(codecs))\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let (module, _, calls) = super::super::build_body_ir_module_v0_with_sdk_linkage(
        &program,
        &["main".to_owned()],
        checker.type_info(),
        &[],
    );
    let targets = super::named_targets(&module, "main");
    let target = targets
        .iter()
        .find(|target| target.name == "Codec.all")
        .ok_or("missing selected enum call")?;
    let selected = target.canonical.as_ref().ok_or("missing checked method identity")?;
    assert!(
        calls.iter().any(|call| &call.canonical == selected),
        "selected={selected:?}; calls={calls:?}"
    );
    Ok(())
}

/// The omitted gzip level is a typed None on Option[int], rather than an untyped unit or a missing SDK default.
#[test]
fn gzip_level_retains_optional_none_default() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.compression.gzip import compress\n\ndef main() -> None:\n    match compress(b\"x\"):\n        Ok(_) => pass\n        Err(_) => pass\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let calls = library_call_delegations(checker.type_info());
    let call = calls
        .iter()
        .find(|call| call.canonical.declaration_name == "compress")
        .ok_or("missing checked gzip function")?;
    assert_eq!(
        call.parameters.get(1),
        Some(&IncanType::Generic {
            base: "Option".to_owned(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
        })
    );
    let Some(bir::CallableParamDefault::Source(default)) = call.parameter_defaults.get(1) else {
        return Err(format!("missing optional default: {:?}", call.parameter_defaults).into());
    };
    assert!(default.stmts.is_empty());
    assert_eq!(default.result, bir::Operand::Constant(bir::Constant::None));
    Ok(())
}
