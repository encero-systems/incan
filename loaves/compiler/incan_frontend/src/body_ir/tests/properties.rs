//! Checked property getter bodies and reads for direct execution.

use super::{IncanPrimitiveType, IncanType, SemanticSourceTargetKind, body_named, build};

/// A computed property of a model lowers to a receiver-only getter body carrying the property's own identity, and a
/// property read is a call of that getter on the borrowed receiver.
#[test]
fn computed_properties_lower_to_getter_bodies_their_reads_call() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "model Account:\n  cents: int\n\n  property dollars -> int:\n    return self.cents // 100\n\n",
        "def main() -> None:\n  account = Account(cents=1250)\n  println(account.dollars)\n",
    );
    let module = build(source, &["m", "computed_property"])?;
    let getter = body_named(&module, "dollars")?;
    let canonical = getter.canonical.as_ref().ok_or("getter identity missing")?;
    assert_eq!(canonical.kind, SemanticSourceTargetKind::Property);
    assert!(module.body_has_canonical_direct_call_id(getter));
    let getter_rendered = getter.render_snapshot();
    assert!(!getter_rendered.contains("unsupported("), "{getter_rendered}");
    assert_eq!(getter.params.len(), 1);
    assert_eq!(getter.return_type, IncanType::Primitive(IncanPrimitiveType::Int));
    let rendered = body_named(&module, "main")?.render_snapshot();
    assert!(rendered.contains("call method:dollars(borrow("), "{rendered}");
    Ok(())
}
