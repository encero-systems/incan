//! Declaration-bound model placeholders and checked method frames for native specialization.

use super::{IncanPrimitiveType, IncanType, bir, body_named, build, named_targets};

/// Inferred owner arguments on a constructor retain the concrete checked carrier inside a generic call.
#[test]
fn inferred_model_constructor_carriers_are_closed() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"model Boxed[T]:
    pub value: T

def get_value[T](boxed: Boxed[T]) -> T:
    return boxed.value

def main() -> None:
    println(get_value(Boxed(value=41)))
    println(get_value(Boxed(value="forty-one")))
"#,
        &["m", "inferred_model_carriers"],
    )?;
    let main = body_named(&module, "main")?;
    let int = IncanType::Primitive(IncanPrimitiveType::Int);
    let text = IncanType::Primitive(IncanPrimitiveType::Str);
    let carriers = main
        .locals
        .iter()
        .filter(|local| local.ty != IncanType::Primitive(IncanPrimitiveType::Unit))
        .map(|local| local.ty.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        carriers,
        [
            IncanType::Generic {
                base: "Boxed".into(),
                args: vec![int.clone()]
            },
            int.clone(),
            IncanType::Generic {
                base: "Boxed".into(),
                args: vec![text.clone()]
            },
            text.clone(),
        ]
    );
    let targets = named_targets(&module, "main");
    let instantiations = targets
        .iter()
        .filter(|target| target.name == "get_value")
        .map(|target| target.type_args.clone())
        .collect::<Vec<_>>();
    assert_eq!(instantiations, [vec![int], vec![text]]);
    Ok(())
}

/// Model binders remain ordered placeholders in layouts, receiver methods, and deferred default frames.
#[test]
fn model_owner_parameter_frames_are_retained() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"model Envelope[T]:
    pub value: T
    pub label: str = "label"

    def get(self) -> T:
        return self.value

    def pick[U](self, value: U) -> U:
        return value

def main() -> None:
    envelope = Envelope[int](value=42)
    envelope.get()
    envelope.pick[str]("picked")
"#,
        &["m", "model_parameter_frames"],
    )?;
    let [declaration] = module.nominal_declarations.as_slice() else {
        return Err("expected one retained model layout".into());
    };
    assert_eq!(declaration.type_parameters, ["T"]);
    assert_eq!(declaration.type_parameter_count, 1);
    assert_eq!(declaration.field_types[0], IncanType::TypeVar("T".into()));
    assert!(module.is_well_formed_nominal_declaration(declaration));
    let defaults = declaration
        .field_default_body
        .as_ref()
        .ok_or("missing model default frame")?;
    assert_eq!(defaults.locals[0].ty, IncanType::TypeVar("T".into()));

    let get = body_named(&module, "get")?;
    assert_eq!(get.return_type, IncanType::TypeVar("T".into()));
    assert!(get.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Return { value: Some(bir::Operand::Place(read)) }
            if read.fact == bir::OwnershipFact::Clone
    )));
    let pick = body_named(&module, "pick")?;
    assert_eq!(pick.type_parameters, ["U"]);
    assert_eq!(pick.return_type, IncanType::TypeVar("U".into()));
    Ok(())
}
