//! Checked method-partial frames and presets for direct execution.

use super::*;

/// A method partial on a model or class retains its target method's frame, through a method alias too, with the presets
/// as defaults beside the target's own, and a call of the partial omits the preset as a defaulted slot of its target.
#[test]
fn method_partials_retain_target_frames_with_presets_as_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "model User:\n  name: str\n\n",
        "  def label(self, prefix: str, suffix: str = \"!\") -> str:\n    return prefix + suffix\n\n",
        "  display = label\n  short = partial display(prefix=\"name\")\n\n",
        "class Counter:\n  pub count: int\n\n",
        "  def bump(mut self, by: int) -> None:\n    self.count += by\n\n  bump_one = partial bump(by=1)\n\n",
        "def main() -> None:\n  println(User(name=\"Ada\").short())\n",
    );
    let module = build(source, &["m", "method_partial_frames"])?;
    let user = module
        .nominal_declarations
        .iter()
        .find(|declaration| declaration.name == "User")
        .ok_or("model declaration missing")?;
    let [short] = user.method_partial_defaults.as_slice() else {
        return Err(format!("one partial frame expected: {:?}", user.method_partial_defaults).into());
    };
    assert_eq!(short.name, "short");
    assert_eq!(short.target.declaration_name, "label");
    assert_eq!(short.frame.canonical.as_ref(), Some(&short.target));
    assert!(matches!(
        short.frame.params.as_slice(),
        [
            _,
            bir::CallableParam {
                default: bir::CallableParamDefault::Source(_),
                ..
            },
            bir::CallableParam {
                default: bir::CallableParamDefault::Source(_),
                ..
            },
        ]
    ));
    assert!(module.is_well_formed_nominal_declaration(user));
    let counter = module
        .nominal_declarations
        .iter()
        .find(|declaration| declaration.name == "Counter")
        .ok_or("class declaration missing")?;
    let [bump_one] = counter.method_partial_defaults.as_slice() else {
        return Err(format!(
            "one class partial frame expected: {:?}",
            counter.method_partial_defaults
        )
        .into());
    };
    assert_eq!(bump_one.target.declaration_name, "bump");
    assert!(module.is_well_formed_nominal_declaration(counter));
    let rendered = body_named(&module, "main")?.render_snapshot();
    assert!(rendered.contains("call method:short defaults=[0, 1]"), "{rendered}");
    Ok(())
}
