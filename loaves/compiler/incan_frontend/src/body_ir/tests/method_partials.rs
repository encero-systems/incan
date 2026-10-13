//! Checked method-partial default frames and receiver snapshots for direct execution.

use super::{IncanType, bir, body_named, build};
use incan_semantics_core::{IncanPrimitiveType, SemanticSourceTargetKind};

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

/// Aliases, closed generic owners and `self` retain the same captured receiver and method forwarding contract.
#[test]
fn method_partials_capture_receivers_and_keep_checked_targets() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/codegen_functions_and_aliases/local_partials_of_methods_of_values.incn"
    );
    let module = build(source, &["method_partials"])?;
    let mut count = 0;
    for body in &module.bodies {
        for statement in &body.block.stmts {
            let bir::StatementKind::Assign {
                rvalue:
                    bir::Rvalue::Closure {
                        params,
                        captured_operands,
                        body: closure,
                    },
                ..
            } = &statement.kind
            else {
                continue;
            };
            count += 1;
            assert_eq!(captured_operands.len(), 2);
            assert_eq!(closure.capture_locals.len(), 2);
            let receiver = closure.capture_locals[0];
            assert_eq!(body.locals[receiver.index()].origin, bir::LocalOrigin::Captured);
            let [forward] = closure.stmts.as_slice() else {
                return Err("a method partial must have exactly one forwarding call".into());
            };
            let bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                args,
                ..
            } = &forward.kind
            else {
                return Err("a method partial must forward to the checked method".into());
            };
            let canonical = target.canonical.as_ref().ok_or("method identity missing")?;
            assert_eq!(canonical.kind, SemanticSourceTargetKind::Method);
            assert!(matches!(target.name.as_str(), "label" | "pair" | "greet"));
            assert_eq!(target.binding, bir::ArgumentBinding::resolved_positional(params.len()));
            let Some(bir::ArgumentElement::One(bir::Operand::Place(read))) = args.first() else {
                return Err("method receiver must be the first forwarding operand".into());
            };
            assert_eq!(read.place.local_id(), Some(receiver));
            assert_eq!(read.fact, bir::OwnershipFact::Borrow);
            assert!(
                params
                    .iter()
                    .any(|param| matches!(param.default, bir::CallableParamDefault::PartialPreset { .. }))
            );
            if target.name == "pair" {
                assert_eq!(params[0].ty, IncanType::Primitive(IncanPrimitiveType::Int));
                assert_eq!(
                    body.locals[receiver.index()].ty,
                    IncanType::Generic {
                        base: "Box".to_string(),
                        args: vec![IncanType::Primitive(IncanPrimitiveType::Int)]
                    }
                );
            }
        }
    }
    assert_eq!(count, 5, "{}", module.render_snapshot());
    Ok(())
}

/// A receiver snapshot is a completed temporary assignment before a later preset can mutate its source place.
#[test]
fn method_partial_materializes_receiver_before_preset_effects() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model User:\n  name: str\n  def label(self, prefix: str) -> str:\n    return prefix + self.name\n\ndef rename(mut user: User) -> str:\n  user.name = \"after\"\n  return \"<\"\n\ndef main() -> None:\n  mut user = User(name=\"before\")\n  held = partial user.label(prefix=rename(user))\n  println(held())\n";
    let module = build(source, &["partial_receiver_order"])?;
    let body = body_named(&module, "main")?;
    let receiver = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { captured_operands, .. },
                ..
            } => captured_operands.first(),
            _ => None,
        })
        .ok_or("method partial capture missing")?;
    let bir::Operand::Place(read) = receiver else {
        return Err("expected a materialized receiver place".into());
    };
    let local = read.place.local_id().ok_or("receiver temporary missing")?;
    assert_eq!(body.locals[local.index()].origin, bir::LocalOrigin::Temporary);
    let snapshot = body.block.stmts.iter().position(|statement| matches!(
        &statement.kind, bir::StatementKind::Assign { place, rvalue: bir::Rvalue::Use(_), .. } if place.local_id() == Some(local)
    )).ok_or("receiver snapshot assignment missing")?;
    let mutation = body.block.stmts.iter().position(|statement| matches!(
        &statement.kind, bir::StatementKind::Call { callee: bir::Callee::Function(bir::CallableTarget::Named(target)), .. } if target.name == "rename"
    )).ok_or("preset mutation call missing")?;
    assert!(snapshot < mutation, "{}", module.render_snapshot());
    Ok(())
}

/// An unretained method source default is explicitly unsupported, while a construction-time preset fills its slot.
#[test]
fn method_partial_keeps_unretained_defaults_explicit() -> Result<(), Box<dyn std::error::Error>> {
    for (preset, supported) in [("", false), ("step=2", true)] {
        let source = format!(
            "model Counter:\n  value: int\n  def add(self, step: int = 1) -> int:\n    return self.value + step\n\ndef make(counter: Counter) -> int:\n  held = partial counter.add({preset})\n  return held()\n"
        );
        let module = build(&source, &["partial_method_default"])?;
        let body = body_named(&module, "make")?;
        let params = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Assign {
                    rvalue: bir::Rvalue::Closure { params, .. },
                    ..
                } => Some(params),
                _ => None,
            })
            .ok_or("method partial missing")?;
        let [param] = params.as_slice() else {
            return Err("expected the method's one declared slot".into());
        };
        match (&param.default, supported) {
            (bir::CallableParamDefault::PartialPreset { .. }, true) => {}
            (bir::CallableParamDefault::Unsupported { description, .. }, false) => {
                assert!(description.contains("partial target add declares a default Body IR could not source"))
            }
            _ => return Err("method default admission changed".into()),
        }
    }
    Ok(())
}
