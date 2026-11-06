//! Checked literal presets on source-local trait method partials.

use super::{bir, body_named, build};

/// A trait partial retains its target identity and supplies the omitted checked literal as an ordinary bound argument.
#[test]
fn trait_partial_retains_literal_preset() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/codegen_functions_and_aliases/partial_methods.incn"
        ),
        &["trait_partial_presets"],
    )?;
    let body = body_named(&module, "use")?;
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
        .ok_or("missing trait partial call")?;
    assert_eq!(target.name, "short");
    assert_eq!(
        target
            .canonical
            .as_ref()
            .ok_or("missing checked target")?
            .declaration_name,
        "label"
    );
    assert!(
        matches!(args.last(), Some(bir::ArgumentElement::One(bir::Operand::Constant(bir::Constant::Str(value)))) if value == "name")
    );
    assert!(
        matches!(&target.binding, bir::ArgumentBinding::Resolved { arguments, defaulted_slots } if arguments.len() == 1 && arguments[0].slot == 0 && defaulted_slots.is_empty())
    );
    Ok(())
}

/// A caller's named override remains the supplied value, so retaining a preset cannot replace explicit arguments.
#[test]
fn trait_partial_preserves_named_override() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"trait Named:
    def label(self, prefix: str) -> str:
        return prefix

    short = partial label(prefix="preset")

model User with Named:
    name: str

pub def use(user: User) -> str:
    return user.short(prefix="override")
"#,
        &["trait_partial_override"],
    )?;
    let body = body_named(&module, "use")?;
    let args = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(_),
                args,
                ..
            } => Some(args),
            _ => None,
        })
        .ok_or("missing trait partial override call")?;
    assert_eq!(args.len(), 2);
    assert!(
        matches!(args.last(), Some(bir::ArgumentElement::One(bir::Operand::Constant(bir::Constant::Str(value)))) if value == "override")
    );
    Ok(())
}
