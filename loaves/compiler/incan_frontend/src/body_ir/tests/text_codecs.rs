//! Builtin codec calls retain their named slots and constant defaults before native lowering.

use super::*;
use incan_semantics_core::SymbolOrigin;

/// Frozen codecs keep the same builtin identity and complete constant slots as mutable text and bytes.
#[test]
fn frozen_codec_calls_retain_complete_slots() -> Result<(), Box<dyn std::error::Error>> {
    let source = "const RAW: FrozenBytes = b\"ok\"\nconst TEXT: FrozenStr = \"ok\"\n\ndef read() -> Result[str, ValidationError]:\n    _ = TEXT.encode()\n    return RAW.decode()\n";
    let module = build(source, &["m", "frozen_codec_slots"])?;
    let body = body_named(&module, "read")?;
    let calls = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                args,
                ..
            } => Some((target, args)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2, "{}", body.render_snapshot());
    assert!(calls.iter().all(|(_, args)| matches!(args.first().and_then(bir::ArgumentElement::as_one), Some(bir::Operand::Place(read)) if read.place.global().is_none())), "{}", body.render_snapshot());
    assert!(
        calls
            .iter()
            .all(|(target, args)| target.canonical.is_some() && args.len() >= 2),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// Legacy's dynamic codec guards reorder and may skip expressions, so unsupported effects must stay refused.
#[test]
fn effectful_codec_expressions_keep_a_named_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def value() -> bytes:\n    println(\"value\")\n    return b\"ok\"\n\ndef label() -> str:\n    println(\"label\")\n    return \"utf8\"\n\ndef main() -> None:\n    _ = value().decode(encoding=label())\n",
        &["m", "codec_effects"],
    )?;
    assert!(
        body_named(&module, "main")?
            .render_snapshot()
            .contains("builtin codec observable expressions")
    );
    Ok(())
}

/// A named errors policy never becomes the encoding label; omitted labels retain the registry's defaults.
#[test]
fn builtin_text_codec_calls_retain_complete_slots() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def decode(data: bytes, policy: str) -> Result[str, ValidationError]:\n    return data.decode(errors=policy)\n\ndef encode(text: str) -> bytes:\n    return text.encode()\n",
        &["m", "codec_slots"],
    )?;
    for (name, count, encoding) in [("encode", 2, "utf-8"), ("decode", 3, "utf-8")] {
        let body = body_named(&module, name)?;
        let (target, args) = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::Method(target),
                    args,
                    ..
                } => Some((target, args)),
                _ => None,
            })
            .ok_or("missing codec call")?;
        assert_eq!(args.len(), count, "{}", body.render_snapshot());
        assert!(
            matches!(&target.binding, bir::ArgumentBinding::Resolved { defaulted_slots, .. } if defaulted_slots.is_empty())
        );
        assert!(
            matches!(args.get(1).and_then(bir::ArgumentElement::as_one), Some(bir::Operand::Constant(bir::Constant::Str(label))) if label == encoding)
        );
        if name == "decode" {
            assert!(matches!(
                args.get(2).and_then(bir::ArgumentElement::as_one),
                Some(bir::Operand::Place(_))
            ));
        }
    }
    Ok(())
}

/// An application encode member keeps its own call contract and acquires no builtin codec defaults.
#[test]
fn source_encode_member_keeps_source_slots() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Encoder:\n    value: int\n\n    def encode(self) -> int:\n        return self.value\n\ndef read(value: Encoder) -> int:\n    return value.encode()\n",
        &["m", "source_codec"],
    )?;
    let body = body_named(&module, "read")?;
    assert!(body.block.stmts.iter().any(|statement| matches!(&statement.kind,
        bir::StatementKind::Call { callee: bir::Callee::Method(target), args, .. }
        if target.canonical.as_ref().is_some_and(|identity| matches!(identity.origin, SymbolOrigin::Module(_))) && args.len() == 1
    )));
    Ok(())
}
