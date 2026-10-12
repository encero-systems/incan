//! Frozen collection constants retain their checked type and literal elements at the Body IR boundary.

use super::{body_named, build};

/// A frozen constant's static data must survive without turning a shadowing parameter into that constant.
#[test]
fn frozen_list_constants_retain_static_elements() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "const NAMES: FrozenList[str] = [\"alpha\", \"beta\"]\n\ndef main() -> int:\n    return len(NAMES)\n\ndef shadow(NAMES: FrozenList[str]) -> int:\n    return len(NAMES)\n",
        &["m", "frozen_lists"],
    )?;
    let main = body_named(&module, "main")?.render_snapshot();
    assert!(main.contains("alpha") && main.contains("beta"), "{main}");
    let shadow = body_named(&module, "shadow")?.render_snapshot();
    assert!(!shadow.contains("alpha") && !shadow.contains("beta"), "{shadow}");
    Ok(())
}
