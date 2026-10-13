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
