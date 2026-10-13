//! Channel send errors retain their checked payload type without treating source newtypes as runtime structs.

use super::{body_named, build};
use incan_semantics_core::IncanType;

/// A send's checked value must close its generic error payload before the direct route allocates result storage.
#[test]
fn channel_send_retains_closed_error_payload() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "import std.async\nfrom std.async.channel import channel\n\nasync def main() -> None:\n    tx, rx = channel(2)\n    _ = await tx.send(1)\n",
        &["m", "channel_error"],
    )?;
    let body = body_named(&module, "main")?;
    assert!(body.locals.iter().any(|local| matches!(&local.ty,
        IncanType::Generic { base, args } if base == "Result" && args.iter().any(|arg| matches!(arg,
            IncanType::Generic { base, args } if base == "SendError" && args == &[IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int)]
        ))
    )), "{}", body.render_snapshot());
    assert!(
        body.locals.iter().all(|local| !open_send_error(&local.ty)),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// The original unpacked-channel body has closed send errors even while its receive payload remains open.
#[test]
fn unpacked_channel_retains_closed_errors_and_open_receive() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/ownership_parameters_and_receivers/channel_ends_unpacked_and_sent_through_a_wrapper.incn"
        ),
        &["m", "unpacked_channel"],
    )?;
    let body = body_named(&module, "main")?;
    assert!(
        body.locals.iter().all(|local| !open_send_error(&local.ty)),
        "{}",
        body.render_snapshot()
    );
    assert!(body.locals.iter().any(|local| matches!(&local.ty,
        IncanType::Generic { base, args } if base == "Option" && args.iter().any(|ty| matches!(ty, IncanType::TypeVar(_) | IncanType::Unknown))
    )), "{}", body.render_snapshot());
    Ok(())
}

/// A declared string sender preserves its checked error argument rather than adopting another carrier.
#[test]
fn annotated_channel_send_keeps_error_payload() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "import std.async\nfrom std.async.channel import channel\n\nasync def main() -> None:\n    tx, rx = channel[str](2)\n    match await tx.send(\"sent\"):\n        Ok(_) => pass\n        Err(error) => println(error.message())\n",
        &["m", "annotated_channel"],
    )?;
    let body = body_named(&module, "main")?;
    assert!(body.locals.iter().any(|local| matches!(&local.ty,
        IncanType::Generic { base, args } if base == "SendError" && args == &[IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Str)]
    )), "{}", body.render_snapshot());
    Ok(())
}

/// A generic newtype method's Self result remains its checked receiver type at the native boundary.
#[test]
fn newtype_self_result_keeps_checked_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "type Token[T] = newtype T:\n    def replace(self, value: T) -> Self:\n        return Token(value)\n\ndef retain(token: Token[int]) -> None:\n    replacement = token.replace(2)\n",
        &["m", "newtype_self"],
    )?;
    let body = body_named(&module, "retain")?;
    assert!(
        body.locals.iter().all(|local| local.ty != IncanType::SelfType),
        "{}",
        body.render_snapshot()
    );
    assert!(body.locals.iter().filter(|local| matches!(&local.ty,
        IncanType::Generic { base, args } if base == "Token" && args == &[IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int)]
    )).count() >= 3, "{}", body.render_snapshot());
    Ok(())
}

/// Identify only a send error's open payload, including the Result or Awaitable that encloses it.
fn open_send_error(ty: &IncanType) -> bool {
    match ty {
        IncanType::Generic { base, args } => {
            (base == "SendError"
                && args
                    .iter()
                    .any(|arg| matches!(arg, IncanType::TypeVar(_) | IncanType::Unknown)))
                || args.iter().any(open_send_error)
        }
        IncanType::Tuple(items) => items.iter().any(open_send_error),
        _ => false,
    }
}
