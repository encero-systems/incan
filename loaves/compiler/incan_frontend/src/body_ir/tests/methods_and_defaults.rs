//! Method bodies and parameter defaults: receiver reads and mutation, abstract, default and static trait methods,
//! chained and tuple-swap assignment, recorded parameter types, deferred default computations for top-level, generic,
//! trait and byte-string defaults, refused defaults that restore ownership state, and newtype and enum method bodies.

use super::*;

/// Generated reflection retains its checked typed/string mode and excludes type-private slots, not all private slots.
#[test]
fn reflection_retains_checked_mode_and_field_policy() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Local:\n    name: str\n\npub model Visible:\n    pub name: str\n    secret: int\n\ndef generic[T](value: T) -> None:\n    println(value.__field_value__(\"name\"))\n    value.__field_items__()\n\ndef main() -> None:\n    local = Local(name=\"Ada\")\n    local.__field_value__(\"name\")\n    local.__field_items__()\n",
        &["reflection_policy"],
    )?;
    let json = serde_json::to_value(&module)?;
    for (name, expected) in [("Local", vec![0]), ("Visible", vec![0])] {
        let declaration = json["nominal_declarations"]
            .as_array()
            .ok_or("missing declarations")?
            .iter()
            .find(|value| value["name"] == name)
            .ok_or("missing nominal")?;
        assert_eq!(declaration["reflection_fields"], serde_json::json!(expected));
    }
    for (body, expected) in [
        ("main", ["TypedValue", "TypedItems"]),
        ("generic", ["StringValue", "StringItems"]),
    ] {
        let value = serde_json::to_string(body_named(&module, body)?)?;
        for mode in expected {
            assert!(value.contains(mode), "missing {mode} in {value}");
        }
    }
    Ok(())
}

/// Bare generic adoptions retain both their own and inherited canonical implementation slots.
#[test]
fn bare_generic_adoption_retains_supertrait_slots() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Read[Element]:\n    def read(self) -> Element: ...\n\ntrait Ordered[Item] with Read[Item]:\n    def same(self) -> Self: ...\n\nmodel Cell[Payload] with Ordered:\n    value: Payload\n    def read(self) -> Payload:\n        return self.value\n    def same(self) -> Self:\n        return self\n\ndef main() -> None:\n    println(Cell(value=7).read())\n",
        &["m", "bare_supertrait_slots"],
    )?;
    let owner = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Cell")
        .ok_or("missing Cell")?;
    for name in ["read", "same"] {
        let body = body_named(&module, name)?;
        assert!(
            module
                .trait_implementations
                .iter()
                .any(|slot| slot.owner == owner.canonical
                    && slot.method.declaration_name == name
                    && Some(&slot.implementation) == body.canonical.as_ref()),
            "missing {name} slot: {:#?}",
            module.trait_implementations
        );
    }
    Ok(())
}

/// A repeated consuming read clones the incoming carrier and leaves its enclosing scope responsible for its drop.
#[test]
fn consuming_carrier_in_a_loop_keeps_the_incoming_owner() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def main() -> None:\n  value: Option[str] = Some(\"kept\")\n  for index in range(2):\n    println(value.unwrap_or(\"fallback\"))\n",
        &["app", "repeated_carrier"],
    )?;
    let body = body_named(&module, "main")?;
    let local = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("value"))
        .ok_or("missing carrier local")?;
    let loop_body = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .ok_or("missing repeated body")?;
    let read = loop_body
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
                ..
            } if read.place.local_id() == Some(local.id) => Some(read),
            _ => None,
        })
        .ok_or("missing carrier snapshot")?;
    assert_eq!(read.fact, bir::OwnershipFact::Clone, "{}", body.render_snapshot());
    assert!(!read.last_use, "a repeated read is not the final dynamic use");
    assert!(
        body.block.stmts.iter().any(|statement| matches!(
            statement.kind, bir::StatementKind::Drop { local: dropped } if dropped == local.id
        )),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// A consuming carrier field is materialized before a default can mutate its owner in place.
#[test]
fn carrier_receiver_snapshot_precedes_default_effects() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Holder:\n  value: Option[str]\n\ndef reset(mut holder: Holder) -> str:\n  holder.value = Some(\"new\")\n  return \"fallback\"\n\ndef choose(mut holder: Holder) -> str:\n  return holder.value.unwrap_or(reset(holder))\n",
        &["app", "carrier_order"],
    )?;
    let body = body_named(&module, "choose")?;
    let snapshot = body
        .block
        .stmts
        .iter()
        .enumerate()
        .find_map(|(index, statement)| {
            let bir::StatementKind::Assign { place, rvalue } = &statement.kind else {
                return None;
            };
            let bir::Rvalue::Use(bir::Operand::Place(read)) = rvalue else {
                return None;
            };
            (read.fact == bir::OwnershipFact::Clone).then_some((index, place))
        })
        .ok_or_else(|| format!("missing carrier snapshot: {}", body.render_snapshot()))?;
    let default = body
        .block
        .stmts
        .iter()
        .position(|statement| {
            matches!(
                &statement.kind,
                bir::StatementKind::Call { callee: bir::Callee::Function(bir::CallableTarget::Named(target)), .. }
                    if target.name == "reset"
            )
        })
        .ok_or("missing default call")?;
    assert!(snapshot.0 < default, "{}", body.render_snapshot());
    let receiver = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                args,
                ..
            } if target.name == "unwrap_or" => match args.first() {
                Some(bir::ArgumentElement::One(bir::Operand::Place(read))) => Some(&read.place),
                _ => None,
            },
            _ => None,
        })
        .ok_or("missing consuming receiver")?;
    assert_eq!(receiver, snapshot.1);
    Ok(())
}

/// Static defaults are evaluated before reading lock-guarded storage, so Body IR preserves the global receiver.
#[test]
fn static_carrier_receiver_remains_after_default_effects() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "static VALUE: Option[str] = Some(\"old\")\n\ndef reset() -> str:\n  VALUE = Some(\"new\")\n  return \"fallback\"\n\ndef main() -> str:\n  return VALUE.unwrap_or(reset())\n",
        &["app", "static_carrier_order"],
    )?;
    let body = body_named(&module, "main")?;
    let receiver = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                args,
                ..
            } if target.name == "unwrap_or" => match args.first() {
                Some(bir::ArgumentElement::One(bir::Operand::Place(read))) => read.place.global(),
                _ => None,
            },
            _ => None,
        })
        .ok_or("missing delayed global receiver")?;
    assert_eq!(receiver.identity.declaration_name, "VALUE");
    Ok(())
}

/// Empty model field defaults retain the declaration's checked element type in their deferred frame.
#[test]
fn empty_model_field_defaults_retain_declared_elements() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/emit_declarations_and_modules/imported_model_constructor_default_fill/records.incn"
        ),
        &["records"],
    )?;
    let declaration = module
        .nominal_declarations
        .first()
        .ok_or("Record declaration missing")?;
    let frame = declaration
        .field_default_body
        .as_ref()
        .ok_or("Record default frame missing")?;
    for local in &frame.locals {
        assert!(
            !matches!(&local.ty, IncanType::Generic { args, .. } if args.iter().any(|ty|
                matches!(ty, IncanType::Unknown | IncanType::Infer))),
            "{}",
            module.render_snapshot()
        );
    }
    Ok(())
}

/// A concrete witness reached through a call also closes a later hidden trait input's retained argument binding.
#[test]
fn trait_value_returns_close_later_hidden_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Root:\n  pass\n\ntrait Mid with Root:\n  pass\n\nmodel Item with Mid:\n  number: int\n\ndef make() -> Mid:\n  return Item(number=3)\n\ndef upcast(value: Mid) -> Root:\n  return value\n\ndef main() -> None:\n  value = upcast(make())\n  println(7)\n",
        &["trait_value_return_calls"],
    )?;
    let targets = named_targets(&module, "main");
    let target = targets
        .iter()
        .find(|target| target.name == "upcast")
        .ok_or("upcast missing")?;
    assert_eq!(target.type_args, [IncanType::Named("Item".to_owned())]);
    Ok(())
}

/// An opaque trait result keeps the single concrete carrier selected by its checked return expression.
#[test]
fn trait_value_return_retains_concrete_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Named:\n  pass\n\nmodel Item with Named:\n  number: int\n\ndef make() -> Named:\n  return Item(number=3)\n\ndef main() -> None:\n  value = make()\n  println(7)\n",
        &["trait_value_return"],
    )?;
    assert_eq!(
        body_named(&module, "make")?.return_type,
        IncanType::Named("Item".to_owned())
    );
    assert!(
        body_named(&module, "main")?
            .locals
            .iter()
            .all(|local| local.ty != IncanType::Named("Named".to_owned())),
        "{}",
        module.render_snapshot()
    );
    Ok(())
}

/// An earlier concrete return witness must retain its model identity beside the declared trait's identity.
#[test]
fn concrete_return_witness_keeps_nominal_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Named:\n  pass\n\nmodel Item with Named:\n  number: int\n\ndef make() -> Named:\n  return Item(number=3)\n\ndef main() -> None:\n  value = make()\n  println(7)\n",
        &["concrete_return_identity"],
    )?;
    let body = body_named(&module, "make")?;
    let identity = body.named_type_identities.get("Item").ok_or("missing model identity")?;
    assert_eq!(identity.kind, SemanticSourceTargetKind::Model, "{body:#?}");
    Ok(())
}

/// A supertrait result preserves the hidden concrete input rather than allocating or converting a trait object.
#[test]
fn trait_value_supertrait_return_retains_input_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Root:\n  pass\n\ntrait Mid with Root:\n  pass\n\nmodel Item with Mid:\n  number: int\n\ndef upcast(value: Mid) -> Root:\n  return value\n\ndef main() -> None:\n  value = upcast(Item(number=3))\n  println(7)\n",
        &["trait_value_upcast"],
    )?;
    assert_eq!(
        body_named(&module, "upcast")?.return_type,
        IncanType::TypeVar("__IncanTrait0".to_owned())
    );
    assert!(
        body_named(&module, "main")?
            .locals
            .iter()
            .all(|local| local.ty != IncanType::Named("Root".to_owned()))
    );
    Ok(())
}

/// A method whose declared result is Self preserves its hidden receiver through an opaque trait return.
#[test]
fn trait_value_self_result_retains_hidden_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Named:\n  def same(self) -> Self: ...\n\nmodel Item with Named:\n  number: int\n  def same(self) -> Self:\n    return self\n\ndef forward(value: Named) -> Named:\n  return value.same()\n\ndef main() -> None:\n  value = forward(Item(number=3))\n  println(7)\n",
        &["trait_value_self_result"],
    )?;
    let body = body_named(&module, "forward")?;
    assert_eq!(body.return_type, IncanType::TypeVar("__IncanTrait0".to_owned()));
    assert!(
        body.locals
            .iter()
            .all(|local| local.ty != IncanType::Named("Named".to_owned())),
        "{body:#?}"
    );
    Ok(())
}

/// Distinct concrete return carriers cannot be silently collapsed into one opaque legacy result.
#[test]
fn trait_value_return_with_several_carriers_stays_open() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Named:\n  pass\n\nmodel One with Named:\n  number: int\n\nmodel Two with Named:\n  text: str\n\ndef choose(flag: bool) -> Named:\n  if flag:\n    return One(number=3)\n  return Two(text=\"x\")\n",
        &["trait_value_several_returns"],
    )?;
    assert_eq!(
        body_named(&module, "choose")?.return_type,
        IncanType::Named("Named".to_owned())
    );
    Ok(())
}

/// Legacy's trait-typed input is a hidden concrete binder, recorded independently of its source trait identity.
#[test]
fn trait_value_input_retains_hidden_binder_and_call_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Named:\n  pass\n\nmodel Item with Named:\n  number: int\n\ndef consume(value: Named) -> int:\n  return 7\n\ndef main() -> None:\n  println(consume(Item(number=3)))\n",
        &["trait_value_input"],
    )?;
    let body = body_named(&module, "consume")?;
    assert_eq!(body.type_parameters, ["__IncanTrait0"]);
    assert_eq!(body.params[0].ty, IncanType::TypeVar("__IncanTrait0".to_owned()));
    assert_eq!(
        body.named_type_identities.get("__IncanTrait0"),
        module.trait_declarations.first()
    );
    let targets = named_targets(&module, "main");
    let target = targets
        .iter()
        .find(|target| target.name == "consume")
        .ok_or("call missing")?;
    assert_eq!(target.type_args, [IncanType::Named("Item".to_owned())]);
    Ok(())
}

/// Two independently typed trait inputs receive separate binders and preserve named argument slot order.
#[test]
fn trait_value_inputs_bind_independent_concrete_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Named:\n  pass\n\nmodel One with Named:\n  number: int\n\nmodel Two with Named:\n  text: str\n\ndef consume(left: Named, right: Named) -> int:\n  return 7\n\ndef main() -> None:\n  println(consume(right=Two(text=\"x\"), left=One(number=3)))\n",
        &["trait_value_inputs"],
    )?;
    let body = body_named(&module, "consume")?;
    assert_eq!(body.type_parameters, ["__IncanTrait0", "__IncanTrait1"]);
    assert_eq!(body.params[1].ty, IncanType::TypeVar("__IncanTrait1".to_owned()));
    let targets = named_targets(&module, "main");
    let target = targets
        .iter()
        .find(|target| target.name == "consume")
        .ok_or("call missing")?;
    assert_eq!(
        target.type_args,
        [IncanType::Named("One".to_owned()), IncanType::Named("Two".to_owned())]
    );
    Ok(())
}

/// A refined opaque trait return refreshes its hidden carrier by slot when named arguments are written out of order.
#[test]
fn refined_trait_returns_refresh_out_of_order_named_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Named:\n  pass\n\nmodel One with Named:\n  number: int\n\nmodel Two with Named:\n  text: str\n\ndef make() -> Named:\n  return Two(text=\"x\")\n\ndef consume(left: Named, right: Named) -> int:\n  return 7\n\ndef main() -> None:\n  println(consume(right=make(), left=One(number=3)))\n",
        &["refined_trait_return_inputs"],
    )?;
    let targets = named_targets(&module, "main");
    let target = targets
        .iter()
        .find(|target| target.name == "consume")
        .ok_or("call missing")?;
    assert!(
        matches!(target.binding, bir::ArgumentBinding::ResolvedDeclarationOrder { .. }),
        "{target:?}"
    );
    assert_eq!(
        target.type_args,
        [IncanType::Named("One".to_owned()), IncanType::Named("Two".to_owned())]
    );
    Ok(())
}

/// The record-pattern fixture closes every enum construction even alongside inferred generic model and class calls.
#[test]
fn generic_enum_record_fixture_has_closed_temporaries() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_logic_comparisons_and_patterns/record_patterns_over_generic_models_and_classes.incn"
    );
    let module = build(source, &["record_enum_destination"])?;
    let body = body_named(&module, "main")?;
    let unclosed = body
        .locals
        .iter()
        .filter(|local| local.ty == IncanType::Named("Shape".to_owned()))
        .map(|local| (local.span, source.get(local.span.start..local.span.end)))
        .collect::<Vec<_>>();
    assert!(unclosed.is_empty(), "{unclosed:#?}");
    Ok(())
}

/// A checked generic enum destination closes unit and payload constructor temporaries and retains positional slots.
#[test]
fn generic_enum_constructors_keep_destination_and_slots() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "enum Shape[T]:\n  Filled(T)\n  Empty\n\ndef read(value: Shape[int]) -> int:\n  match value:\n    Shape.Filled(n) => return n\n    Shape.Empty => return 0\n\ndef main() -> None:\n  empty: Shape[int] = Shape.Empty\n  println(read(Shape.Filled(7)))\n",
        &["enum_destination"],
    )?;
    let body = body_named(&module, "main")?;
    assert!(
        body.locals
            .iter()
            .all(|local| local.ty != IncanType::Named("Shape".to_owned())),
        "{body:#?}"
    );
    for statement in &body.block.stmts {
        if let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::EnumVariant(target), _),
            ..
        } = &statement.kind
        {
            assert!(
                matches!(target.binding, bir::ArgumentBinding::Resolved { .. }),
                "{target:#?}"
            );
        }
    }
    let read = body_named(&module, "read")?;
    assert_eq!(
        read.locals
            .iter()
            .find(|local| local.name.as_deref() == Some("n"))
            .ok_or("payload local missing")?
            .ty,
        IncanType::Primitive(IncanPrimitiveType::Int)
    );
    Ok(())
}
/// Ordered enum binders and checked payload placeholders survive the Body IR boundary, including variant aliases.
#[test]
fn generic_enum_retains_ordered_binders_and_payloads() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "@derive(Clone)\nenum Holder[Elem with Clone]:\n  Items(list[Elem])\n  Empty\n  Full = alias Items\n",
        &["m", "generic_enum_layout"],
    )?;
    let declaration = module.enum_declarations.first().ok_or("generic enum layout missing")?;
    let encoded = serde_json::to_value(declaration)?;
    assert_eq!(encoded.get("type_parameters"), Some(&serde_json::json!(["Elem"])));
    assert_eq!(declaration.variants.len(), 2);
    assert_eq!(
        declaration.variants[0].fields,
        [incan_semantics_core::generic(
            "List".to_owned(),
            vec![IncanType::TypeVar("Elem".to_owned())]
        )]
    );
    assert!(module.is_well_formed_native_enum_declaration(declaration));
    Ok(())
}

/// Error fallback and a user's own string method retain their selected canonical targets; derived Display keeps Debug.
#[test]
fn nominal_display_retains_checked_method_selection() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.traits.error import Error\n\nmodel Failure with Error:\n  detail: str\n  def message(self) -> str:\n    return self.detail\n\nmodel Loud with Error:\n  def message(self) -> str:\n    return \"quiet\"\n  def __str__(self) -> str:\n    return \"LOUD\"\n\n@derive(Display)\nmodel Point:\n  x: int\n",
        &["m", "nominal_display_selection"],
    )?;
    let selected = |owner: &str| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let declaration = module
            .nominal_declarations
            .iter()
            .find(|declaration| declaration.name == owner)
            .ok_or("display owner missing")?;
        Ok(serde_json::to_value(declaration)?)
    };
    assert_eq!(selected("Failure")?["display_method"]["declaration_name"], "message");
    assert_eq!(selected("Loud")?["display_method"]["declaration_name"], "__str__");
    assert!(selected("Point")?["display_method"].is_null());
    Ok(())
}

/// A newtype's wrapped-value reads through self and a Self-typed parameter retain the same storage identity.
#[test]
fn self_field_reads_keep_declared_storage_identity() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "type Cell = newtype int:\n  def add(self, other: Self) -> int:\n    return self.0 + other.0\n",
        &["m", "self_field_identity"],
    )?;
    let owner = module.nominal_declarations.first().ok_or("model missing")?;
    let identity = owner.field_identities.first().ok_or("field identity missing")?;
    let body = body_named(&module, "add")?;
    let mut projections = 0;
    for statement in &body.block.stmts {
        if let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::BinaryOp(_, left, right),
            ..
        } = &statement.kind
        {
            for operand in [left, right] {
                if let bir::Operand::Place(read) = operand {
                    for projection in &read.place.projection {
                        if let bir::PlaceElem::Field { canonical, .. } = projection {
                            assert_eq!(canonical.as_ref(), Some(identity), "{module:#?}");
                            projections += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(projections, 2, "{module:#?}");
    Ok(())
}

/// A generic owner's method partial carries the same receiver and parameter types as its target frame.
#[test]
fn generic_method_partials_retain_owner_frames() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Wrapper[T]:\n  value: T\n  def tag(self, prefix: str) -> str:\n    return prefix\n  tagged = partial tag(prefix=\"wrapped\")\n",
        &["m", "generic_partial_frames"],
    )?;
    let owner = module.nominal_declarations.first().ok_or("generic owner missing")?;
    let [preset] = owner.method_partial_defaults.as_slice() else {
        return Err(format!("partial frame missing: {module:#?}").into());
    };
    let target = body_named(&module, "tag")?;
    assert_eq!(preset.frame.params[0].ty, target.params[0].ty);
    assert!(matches!(
        preset.frame.params[1].default,
        bir::CallableParamDefault::Source(_)
    ));
    Ok(())
}

/// TOML bundle aliases expand the checked module's derive metadata, while Descriptor is a checked opt-in without a
/// Rust derive macro; neither spelling may leak into the direct runtime's native derive list.
#[test]
fn retains_model_toml_and_descriptor_native_derives() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std import toml as manifest\n\n@derive(manifest)\nmodel Project:\n  name: str\n\n@derive(Descriptor)\nmodel Version:\n  major: int\n",
        &["m", "model_derive_metadata"],
    )?;
    let project = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Project")
        .ok_or("TOML model missing")?;
    assert!(project.derives.iter().any(|derive| derive == "serde::Serialize"));
    assert!(project.derives.iter().any(|derive| derive == "serde::Deserialize"));
    assert!(!project.derives.iter().any(|derive| derive == "manifest"));
    let version = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Version")
        .ok_or("descriptor model missing")?;
    assert!(!version.derives.iter().any(|derive| derive == "Descriptor"));
    Ok(())
}

/// A model combining owner arguments, an instantiated trait and an alias retains its layout, method frame and trait
/// implementation once, with canonical targets rather than an alias wrapper body.
#[test]
fn retains_model_with_type_parameters_trait_adoption_and_alias() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Read[T]:\n  def read(self) -> T: ...\n\nmodel Cell[T with Clone] with Read[T]:\n  value: T\n  get = read\n  def read(self) -> T:\n    return self.value\n\ndef main() -> None:\n  println(Cell(value=7).get())\n  println(Cell(value=\"text\").get())\n",
        &["m", "combined_model_shape"],
    )?;
    let owner = module.nominal_declarations.first().ok_or("model layout missing")?;
    assert_eq!(owner.name, "Cell");
    assert_eq!(owner.type_parameter_count, 1);
    let read = body_named(&module, "read")?;
    assert_eq!(owner.type_parameters, ["T"]);
    assert_eq!(read.name, "read");
    let implementation = module
        .trait_implementations
        .first()
        .ok_or("trait implementation missing")?;
    assert_eq!(implementation.owner, owner.canonical);
    assert!(module.is_well_formed_trait_implementation(implementation));
    Ok(())
}

/// Class defaults retain declaration-owned computations with the same omission contract as model defaults.
#[test]
fn retains_class_field_default_computations() -> Result<(), Box<dyn std::error::Error>> {
    let module = build("class Vault:\n  value: int = 7\n", &["class_defaults"])?;
    let declaration = module.nominal_declarations.first().ok_or("class declaration missing")?;
    let body = declaration
        .field_default_body
        .as_ref()
        .ok_or("class default frame missing")?;
    assert!(matches!(body.params[0].default, bir::CallableParamDefault::Source(_)));
    Ok(())
}

/// Class derives use the checked selection models use: explicit names, the derives they imply, then the automatic set.
#[test]
fn retains_checked_class_derive_selection() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "@derive(Display)\nclass Account:\n  pub owner: str\n",
        &["class_derives"],
    )?;
    let declaration = module
        .nominal_declarations
        .first()
        .ok_or("derived class declaration missing")?;
    assert_eq!(
        declaration.derives,
        ["Display", "Debug", "Clone", "FieldInfo", "IncanClass"]
    );
    Ok(())
}

/// A classmethod's `cls(...)` constructs its owner with the owner's declaration identity and layout in local and
/// project profiles; qualifying the result carrier must not erase the checked constructor target.
#[test]
fn classmethod_construction_retains_its_owner_declaration() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class Cell:\n  pub v: int\n  @classmethod\n  def of(cls, v: int) -> Self:\n    return cls(v=v)\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["classmethod_construction".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    for project in [false, true] {
        let module = if project {
            super::build_body_ir_module_v0_with_executable_context(&program, &path, checker.type_info(), &[])
        } else {
            super::build_body_ir_module_v0(&program, &path, checker.type_info())
        };
        let declaration = module.nominal_declarations.first().ok_or("class declaration missing")?;
        let body = body_named(&module, "of")?;
        let target = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Assign {
                    rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _),
                    ..
                } => Some(target),
                _ => None,
            })
            .ok_or("the classmethod construction must lower as a constructor aggregate")?;
        assert_eq!(
            target.canonical.as_ref(),
            Some(&declaration.canonical),
            "project={project}: {}",
            body.render_snapshot()
        );
        assert_eq!(
            target.direct_declaration_id.as_ref(),
            Some(&declaration.direct_declaration_id),
            "project={project}: {}",
            body.render_snapshot()
        );
        assert_eq!(target.canonical_field_layout.as_ref(), Some(&declaration.fields));
    }
    Ok(())
}

/// A static call keeps owner arguments in its receiver without duplicating them as method arguments.
#[test]
fn static_call_on_an_applied_class_keeps_its_owner_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class Box[T]:\n  pub value: T\n  @classmethod\n  def make(cls, value: T) -> Self:\n    return cls(value=value)\n\ndef main() -> int:\n  return Box[int].make(1).value\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["applied_static".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let facts = checker.type_info();
    let module = build_body_ir_module_v0(&program, &path, facts);
    let targets = named_targets(&module, "main");
    let make = targets
        .iter()
        .find(|target| target.name.ends_with("make"))
        .ok_or("no named call to `make`")?;
    assert_eq!(
        make.receiver_type,
        Some(IncanType::Generic {
            base: "Box".to_string(),
            args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
        })
    );
    assert!(
        make.type_args.is_empty(),
        "owner binders must not also become method binders: native={:?}, legacy={:?}",
        facts.calls.body_ir_call_type_arguments,
        facts.calls.call_site_monomorph_type_args,
    );
    Ok(())
}

/// Explicit and inferred method binders stay independent of an applied static owner.
#[test]
fn applied_static_call_preserves_independent_method_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "class Box[T]:\n  pub value: T\n  @staticmethod\n  def echo[U](value: U) -> U:\n    return value\n\ndef main() -> None:\n  println(Box[int].echo[str](\"explicit\"))\n  println(Box[int].echo(\"inferred\"))\n",
        &["independent_static"],
    )?;
    let targets = named_targets(&module, "main");
    let calls = targets
        .iter()
        .filter(|target| target.name.ends_with("echo"))
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    for target in calls {
        assert_eq!(target.type_args, [IncanType::Primitive(IncanPrimitiveType::Str)]);
        assert_eq!(
            target.receiver_type,
            Some(IncanType::Generic {
                base: "Box".to_string(),
                args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
            })
        );
    }
    Ok(())
}

/// Method aliases, a newtype's short rebinding of its own method, and lint suppressions keep their declarations.
#[test]
fn retains_aliased_and_lint_suppressed_declarations() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "@rust.allow(\"non_camel_case_types\")\nclass Counter:\n  pub n: int\n  grow = add\n\n  @rust.allow(\"non_snake_case\")\n  def add(mut self) -> None:\n    self.n += 1\n\ntype Meters = newtype int:\n  def value(self) -> int:\n    return self.0\n  get = value\n\n@rust.allow(\"clippy::large_enum_variant\")\nenum Size:\n  Small\n  Large(str)\n",
        &["aliases_and_lints"],
    )?;
    let retained = module
        .nominal_declarations
        .iter()
        .map(|declaration| declaration.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(retained, ["Counter", "Meters"]);
    assert!(
        module
            .enum_declarations
            .iter()
            .any(|declaration| declaration.name == "Size"),
        "a lint-suppressed enum keeps its native layout"
    );
    Ok(())
}

/// Explicit builtin derives survive collection of a newtype's checked nominal carrier.
#[test]
fn retains_explicit_newtype_derives() -> Result<(), Box<dyn std::error::Error>> {
    let module = build("@derive(Clone, Eq)\npub newtype EntryId = str\n", &["ids"])?;
    let nominal = module
        .nominal_declarations
        .first()
        .ok_or("derived newtype carrier missing")?;
    assert!(nominal.derives.iter().any(|selection| selection == "Clone"));
    assert!(nominal.derives.iter().any(|selection| selection == "Eq"));
    assert!(nominal.derives.iter().any(|selection| selection == "PartialEq"));
    let numeric = build("@derive(Clone, Eq)\npub newtype Count = int\n", &["counts"])?;
    let carrier = numeric
        .nominal_declarations
        .first()
        .ok_or("numeric newtype carrier missing")?;
    assert_eq!(carrier.derives, ["Debug", "Clone", "Copy", "Eq", "PartialEq"]);
    Ok(())
}

#[test]
fn lowers_an_immutable_receiver_read_through_a_field_projection() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Counter:\n  value: int\n\n  def get(self) -> int:\n    return self.value\n";
    let module = build(source, &["m", "receiver_read"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body get decl:m::receiver_read::Counter::get"));
    assert!(snapshot.contains("local 0 self : Counter [receiver]"));
    // `self.value` is a projected read of an `int` (Copy) field, so it reads `copy`, never `move` or `clone`.
    assert!(snapshot.contains("return copy(_0.value)"));

    Ok(())
}

#[test]
fn mut_self_receiver_origin_is_mutable_and_field_mutation_lowers() -> Result<(), Box<dyn std::error::Error>> {
    // `mut self` must remain a mutable receiver when its field assignment is lowered.
    let source = "model Counter:\n  value: int\n\n  def bump(mut self) -> None:\n    self.value = self.value + 1\n";
    let module = build(source, &["m", "receiver_mut"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body bump decl:m::receiver_mut::Counter::bump"));
    assert!(snapshot.contains("local 0 self : Counter [receiver_mut]"));
    assert!(
        !snapshot.contains("unsupported("),
        "mutable receiver field assignment should lower without a placeholder: {snapshot}"
    );

    Ok(())
}

#[test]
fn lowers_a_method_call_on_self_with_a_borrowed_receiver_argument() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Counter:\n  value: int\n\n  def get(self) -> int:\n    return self.value\n\n  def get_twice(self) -> int:\n    return self.get() + self.get()\n";
    let module = build(source, &["m", "method_call"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body get_twice decl:m::method_call::Counter::get_twice"));
    // Method-call receivers borrow, mirroring how any other method call's receiver already lowers.
    assert!(snapshot.contains("call method:get(borrow(_0))"));
    Ok(())
}

#[test]
fn abstract_trait_method_produces_no_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Greeter:\n  def greet(self) -> str: ...\n";
    let module = build(source, &["m", "abstract_method"])?;

    assert!(
        module.bodies.is_empty(),
        "an abstract method has no body to lower, and must not produce an Unsupported placeholder body either: {:?}",
        module.bodies
    );

    Ok(())
}

#[test]
fn lowers_tuple_assign_swap_with_correct_evaluation_order() -> Result<(), Box<dyn std::error::Error>> {
    // `arr[i], arr[j] = (arr[j], arr[i])` must read both original values before writing either target, or the
    // swap would clobber `arr[i]` before `arr[j]`'s read observes it. A leading plain-identifier target (`a, b
    // = ...`) always parses as `TupleUnpackStmt` instead (new bindings, possibly shadowing) -- lvalue index/
    // field targets are what actually reaches `TupleAssignStmt`, matching the parser's own routing
    // (`loaves/kernel/incan_syntax/src/parser/stmts.rs`'s `assignment_or_expr_stmt`).
    let source =
        "def swap(mut arr: list[int], i: int, j: int) -> int:\n  arr[i], arr[j] = (arr[j], arr[i])\n  return arr[i]\n";
    let module = build(source, &["m", "tuple_assign"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "tuple assign should not fall back: {snapshot}"
    );
    // Both targets should end up written via a plain `Assign` into an `[index]`-projected place, not
    // `Unsupported`.
    assert!(
        snapshot.matches("] = ").count() >= 2,
        "both index-projected targets should be assigned: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_a_default_trait_method_with_a_self_typed_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Identity:\n  def identity(self) -> Self:\n    return self\n";
    let module = build(source, &["m", "trait_default"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body identity decl:m::trait_default::Identity::identity"));
    assert!(snapshot.contains("local 0 self : Self [receiver]"));
    assert!(snapshot.contains("return clone(_0)"));

    Ok(())
}

#[test]
fn lowers_chained_assignment_right_to_left() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def chain() -> int:\n  x = y = z = 5\n  return x + y + z\n";
    let module = build(source, &["m", "chained"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "chained assignment should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains("const(5)"),
        "the rightmost target reads the literal value: {snapshot}"
    );
    Ok(())
}

/// A shared chained value retains its original type rather than reading an already wrapped target back into
/// another destination. Each target reads the same once-evaluated value in written order.
#[test]
fn chained_targets_read_the_original_value_before_destination_injection() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def value() -> list[int]:\n  return [1]\ndef chain() -> None:\n  mut left: list[int] = []\n  mut right: Option[list[int]] = None\n  left = right = value()\n";
    let module = build(source, &["m", "chained_original"])?;
    let body = body_named(&module, "chain")?;
    let left = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("left"))
        .ok_or("missing left")?
        .id;
    let right = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("right"))
        .ok_or("missing right")?
        .id;
    let mut reads: Vec<_> = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } if place.local_id() == Some(left) || place.local_id() == Some(right) => {
                Some((place.local_id(), read.place.local_id()))
            }
            _ => None,
        })
        .rev()
        .take(2)
        .collect();
    reads.reverse();
    assert_eq!(reads.len(), 2);
    assert_eq!(reads[0].0, Some(left));
    assert_eq!(reads[1].0, Some(right));
    assert_eq!(reads[0].1, reads[1].1);
    assert_ne!(reads[0].1, Some(left));
    assert_ne!(reads[0].1, Some(right));
    Ok(())
}

/// A chained value the checker recorded as written per target is evaluated once per target, left to right, at that
/// target's type, as legacy writes it: `n = f = 1` over an `int` and a `float` writes the integer `1` to `n` and the
/// float `1.0` to `f`, and a name the chain declares takes the literal as an `int`.
#[test]
fn chained_literal_written_per_target_takes_each_target_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def chain() -> None:\n  mut n: int = 0\n  mut f: float = 0.5\n  n = f = 1\n  x = n = f = -2\n";
    let module = build(source, &["m", "chained_per_target"])?;
    let body = body_named(&module, "chain")?;
    let local_named = |name: &str| {
        body.locals
            .iter()
            .find(|local| local.name.as_deref() == Some(name))
            .map(|local| local.id)
            .ok_or(format!("missing local `{name}`"))
    };
    let (n, f, x) = (local_named("n")?, local_named("f")?, local_named("x")?);
    let writes: Vec<(bir::LocalId, &bir::Constant)> = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Constant(constant)),
            } => place.local_id().map(|local| (local, constant)),
            _ => None,
        })
        .collect();
    // An `int` target keeps `-2` as a negation of `2`; only the `float` target folds it into a constant.
    let expected = [
        (n, bir::Constant::Int(0)),
        (f, bir::Constant::Float("0.5".into())),
        (n, bir::Constant::Int(1)),
        (f, bir::Constant::Float("1".into())),
        (f, bir::Constant::Float("-2".into())),
    ];
    assert_eq!(body.locals[x.index()].ty, IncanType::Primitive(IncanPrimitiveType::Int));
    let actual: Vec<(bir::LocalId, bir::Constant)> = writes
        .into_iter()
        .map(|(local, constant)| (local, constant.clone()))
        .collect();
    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn static_method_lowers_like_a_free_function_with_no_receiver_local() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Counter:\n  value: int\n\n  def zero() -> Counter:\n    return Counter(value=0)\n";
    let module = build(source, &["m", "static_method"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body zero decl:m::static_method::Counter::zero"));
    assert!(
        !snapshot.contains("[receiver"),
        "a static/associated method (receiver: None) must not declare a receiver local: {snapshot}"
    );

    Ok(())
}

#[test]
fn method_parameter_type_is_recorded_from_the_checked_callable_signature() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "model Counter:\n  value: int\n\n  def add(self, amount: int) -> int:\n    return self.value + amount\n";
    let module = build(source, &["m", "method_param"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body add decl:m::method_param::Counter::add"));
    assert!(
        snapshot.contains("local 1 amount : int [param]"),
        "an ordinary method parameter must declare with its checked resolved type, not Unknown: {snapshot}"
    );

    Ok(())
}

#[test]
fn top_level_defaults_lower_to_deferred_source_computations() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fallback() -> int:\n  return 2\n\ndef choose(limit: u8 = 7, value: int = fallback()) -> int:\n  return value\n";
    let module = build(source, &["m", "top_level_default"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let limit = choose.params.first().ok_or("expected choose's limit parameter")?;
    let value = choose.params.get(1).ok_or("expected choose's value parameter")?;

    assert_eq!(limit.local, bir::LocalId(0));
    assert_eq!(limit.name, "limit");
    assert_eq!(
        limit.ty,
        IncanType::Primitive(IncanPrimitiveType::Numeric(
            incan_lang::lang::types::numerics::NumericTypeId::U8
        ))
    );
    let bir::CallableParamDefault::Source(limit_default) = &limit.default else {
        return Err("a checked literal default must become a deferred Body-IR computation".into());
    };
    let limit_start = source.find("7,").ok_or("missing literal default source spelling")?;
    assert_eq!(limit_default.span, HirSourceSpan::new(limit_start, limit_start + 1));
    assert!(limit_default.stmts.is_empty());
    assert_eq!(
        limit_default.result,
        bir::Operand::Constant(bir::Constant::TypedNumeric(bir::TypedNumericConstant::Unsigned {
            kind: incan_lang::lang::types::numerics::NumericTypeId::U8,
            value: 7,
        }))
    );

    assert_eq!(value.local, bir::LocalId(1));
    assert_eq!(value.name, "value");
    let bir::CallableParamDefault::Source(value_default) = &value.default else {
        return Err("a checked function default call must become a deferred Body-IR computation".into());
    };
    let call_start = source.rfind("fallback()").ok_or("missing default source spelling")?;
    assert_eq!(
        value_default.span,
        HirSourceSpan::new(call_start, call_start + "fallback()".len()),
        "the direct consumer must receive the default expression's exact source span"
    );
    let [call] = value_default.stmts.as_slice() else {
        return Err("the deferred function default should contain one call statement".into());
    };
    let bir::StatementKind::Call {
        destination: Some(destination),
        callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
        args,
        may_panic,
    } = &call.kind
    else {
        return Err("the deferred default must retain a direct named call".into());
    };
    assert_eq!(target.name, "fallback");
    assert!(target.type_args.is_empty());
    assert!(args.is_empty());
    assert!(!may_panic);
    let bir::Operand::Place(result) = &value_default.result else {
        return Err("the deferred default call must return its computed temporary".into());
    };
    assert_eq!(&result.place, destination);
    assert!(
        !choose.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        )),
        "the default call must not be appended to the ordinary function body: {choose:?}"
    );
    assert!(
        !choose
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "a refused source default must not retain an implicit frontend lookup: {choose:?}"
    );

    Ok(())
}

#[test]
fn generic_method_defaults_use_the_shared_parameter_contract_after_self() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fallback() -> str:\n  return \"label\"\n\nmodel Shelf[T]:\n  def label[U](self, owner_items: list[T] = [], method_items: list[U] = [], suffix: str = \"\", fallback_label: str = fallback()) -> str:\n    return suffix\n";
    let module = build(source, &["m", "method_default"])?;
    let label = module
        .bodies
        .iter()
        .find(|body| body.name == "label")
        .ok_or("expected the label method Body IR")?;
    let self_param = label.params.first().ok_or("expected the self parameter")?;
    let owner_items = label.params.get(1).ok_or("expected the owner-generic parameter")?;
    let method_items = label.params.get(2).ok_or("expected the method-generic parameter")?;
    let suffix = label.params.get(3).ok_or("expected the literal default parameter")?;
    let fallback_label = label.params.get(4).ok_or("expected the call default parameter")?;

    assert_eq!(self_param.local, bir::LocalId(0));
    assert!(
        self_param.span.start < self_param.span.end,
        "the synthetic receiver must carry its documented declaration-span fallback"
    );
    assert!(matches!(&self_param.default, bir::CallableParamDefault::Required));
    assert!(matches!(&owner_items.default, bir::CallableParamDefault::Source(_)));
    assert!(matches!(&method_items.default, bir::CallableParamDefault::Source(_)));
    let bir::CallableParamDefault::Source(suffix_default) = &suffix.default else {
        return Err("a checked method literal default must become a deferred computation".into());
    };
    let literal_start = source.find("\"\"").ok_or("missing method literal default spelling")?;
    assert_eq!(
        suffix_default.span,
        HirSourceSpan::new(literal_start, literal_start + "\"\"".len())
    );
    assert!(suffix_default.stmts.is_empty());
    assert_eq!(
        suffix_default.result,
        bir::Operand::Constant(bir::Constant::Str(String::new()))
    );
    let bir::CallableParamDefault::Source(fallback_default) = &fallback_label.default else {
        return Err("a checked method call default must become a deferred computation".into());
    };
    let call_start = source
        .rfind("fallback()")
        .ok_or("missing method call default spelling")?;
    assert_eq!(
        fallback_default.span,
        HirSourceSpan::new(call_start, call_start + "fallback()".len())
    );
    assert_eq!(fallback_default.stmts.len(), 1);
    assert!(
        !label.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        )),
        "the method default call must not be appended to the ordinary method body: {label:?}"
    );
    assert_eq!(label.param_locals.len(), 5);

    Ok(())
}

#[test]
fn trait_method_default_uses_a_deferred_source_computation() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fallback() -> str:\n  return \"hello\"\n\ntrait Greeter:\n  def greet(self, greeting: str = fallback()) -> str:\n    return greeting\n";
    let module = build(source, &["m", "trait_method_default"])?;
    let greet = module
        .bodies
        .iter()
        .find(|body| body.name == "greet")
        .ok_or("expected the greet trait-method Body IR")?;
    let self_param = greet.params.first().ok_or("expected the self parameter")?;
    let greeting = greet.params.get(1).ok_or("expected the greeting parameter")?;

    assert!(matches!(&self_param.default, bir::CallableParamDefault::Required));
    let bir::CallableParamDefault::Source(default) = &greeting.default else {
        return Err("a checked trait-method default must become a deferred computation".into());
    };
    let default_start = source
        .rfind("fallback()")
        .ok_or("missing trait-method default source spelling")?;
    assert_eq!(
        default.span,
        HirSourceSpan::new(default_start, default_start + "fallback()".len())
    );
    let [call] = default.stmts.as_slice() else {
        return Err("the deferred trait-method default should contain one call statement".into());
    };
    assert!(matches!(
        &call.kind,
        bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            ..
        } if target.name == "fallback"
    ));
    assert!(
        !greet.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.name == "fallback"
        )),
        "the trait-method default call must not be appended to the ordinary method body: {greet:?}"
    );

    Ok(())
}

#[test]
fn byte_string_default_is_a_deferred_body_ir_constant_at_its_own_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def keep(payload: bytes = b\"x\") -> bytes:\n  return payload\n";
    let module = build(source, &["m", "unsupported_default"])?;
    let keep = module.bodies.first().ok_or("expected the keep function Body IR")?;
    let payload = keep.params.first().ok_or("expected the payload parameter")?;

    let bir::CallableParamDefault::Source(default) = &payload.default else {
        return Err("bytes defaults must retain their representable Body-IR constant".into());
    };
    let default_start = source.find("b\"x\"").ok_or("missing bytes default spelling")?;
    assert_eq!(
        default.span,
        HirSourceSpan::new(default_start, default_start + "b\"x\"".len()),
        "the deferred computation must retain the default expression's exact source span"
    );
    assert!(default.stmts.is_empty());
    assert_eq!(
        default.result,
        bir::Operand::Constant(bir::Constant::Bytes(b"x".to_vec()))
    );
    assert_eq!(
        keep.locals.len(),
        1,
        "a literal default must not allocate a speculative temporary or external local: {keep:?}"
    );
    assert!(
        !keep
            .block
            .stmts
            .iter()
            .any(|statement| matches!(&statement.kind, bir::StatementKind::Unsupported { .. })),
        "the deferred default belongs to parameter metadata, not the normal function body: {keep:?}"
    );

    Ok(())
}

#[test]
fn unsupported_race_arm_in_a_default_is_found_at_its_nested_source_span() {
    // A race remains a structured Body-IR node even when one arm has an unsupported construct. Callable
    // defaults have the stricter contract: the whole deferred computation must be executable, so the default
    // boundary must find that nested refusal and retain the nested construct's span for direct consumers.
    let unsupported_span = HirSourceSpan::new(24, 34);
    let statements = vec![bir::Statement {
        kind: bir::StatementKind::Race {
            destination: None,
            arms: vec![bir::RaceArm {
                awaitable: bir::Operand::Constant(bir::Constant::Int(1)),
                binding: bir::LocalId(0),
                body: bir::Block {
                    scope: bir::ScopeId(1),
                    stmts: vec![bir::Statement {
                        kind: bir::StatementKind::Unsupported {
                            description: "power operator".to_string(),
                        },
                        span: unsupported_span,
                    }],
                },
                result: bir::Operand::Constant(bir::Constant::Int(0)),
            }],
        },
        span: HirSourceSpan::new(10, 40),
    }];

    assert_eq!(
        first_unsupported_default_statement(&statements),
        Some((unsupported_span, "power operator".to_string())),
        "a direct consumer must refuse the nested construct rather than accept a partially unsupported default"
    );
}

/// Default validation descends into structured computations and preserves the unsupported statement's own span.
#[test]
fn unsupported_rvalue_bodies_in_a_default_are_found_at_their_nested_source_spans() {
    // A source default can construct a closure or generator, or evaluate a match, whose structured Body IR owns
    // more statements than the outer assignment exposes. Those statement sequences are still part of the direct
    // default contract: a consumer must receive their original refusal span instead of a misleading `Source`.
    let unsupported = |span: HirSourceSpan, description: &str| bir::Statement {
        kind: bir::StatementKind::Unsupported {
            description: description.to_string(),
        },
        span,
    };
    let assignment = |rvalue| bir::Statement {
        kind: bir::StatementKind::Assign {
            place: bir::Place::from_local(bir::LocalId(0)),
            rvalue,
        },
        span: HirSourceSpan::new(0, 80),
    };
    let result = bir::Operand::Constant(bir::Constant::Int(0));
    let closure_span = HirSourceSpan::new(10, 20);
    let generator_span = HirSourceSpan::new(21, 31);
    let guard_span = HirSourceSpan::new(32, 42);
    let body_span = HirSourceSpan::new(43, 53);
    let cases = vec![
        (
            vec![assignment(bir::Rvalue::Closure {
                params: Vec::new(),
                captured_operands: Vec::new(),
                body: Box::new(bir::ClosureBody {
                    original_callable: None,
                    function_item: false,
                    consumes_environment: false,
                    capture_locals: Vec::new(),
                    stmts: vec![unsupported(closure_span, "closure body")],
                    result: result.clone(),
                }),
            })],
            closure_span,
            "closure body",
        ),
        (
            vec![assignment(bir::Rvalue::Generator {
                source: bir::Operand::Constant(bir::Constant::Int(1)),
                captured_operands: Vec::new(),
                body: Box::new(bir::GeneratorBody {
                    source_local: bir::LocalId(1),
                    capture_locals: Vec::new(),
                    stmts: vec![unsupported(generator_span, "generator body")],
                }),
            })],
            generator_span,
            "generator body",
        ),
        (
            vec![assignment(bir::Rvalue::Match {
                scrutinee: bir::Operand::Constant(bir::Constant::Int(1)),
                arms: vec![bir::MatchArm {
                    pattern: bir::Pattern::Wildcard,
                    guard_stmts: vec![unsupported(guard_span, "match guard")],
                    guard: Some(bir::Operand::Constant(bir::Constant::Bool(true))),
                    body_stmts: Vec::new(),
                    result: result.clone(),
                }],
            })],
            guard_span,
            "match guard",
        ),
        (
            vec![assignment(bir::Rvalue::Match {
                scrutinee: bir::Operand::Constant(bir::Constant::Int(1)),
                arms: vec![bir::MatchArm {
                    pattern: bir::Pattern::Wildcard,
                    guard_stmts: Vec::new(),
                    guard: None,
                    body_stmts: vec![unsupported(body_span, "match body")],
                    result,
                }],
            })],
            body_span,
            "match body",
        ),
    ];

    for (statements, span, description) in cases {
        assert_eq!(
            first_unsupported_default_statement(&statements),
            Some((span, description.to_string())),
            "a nested {description} refusal must prevent an incomplete default computation from becoming Source"
        );
    }
}

#[test]
fn invalid_default_is_rejected_before_body_ir_is_built() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def choose(value: int = \"wrong\") -> int:\n  return value\n";
    let error = build(source, &["m", "invalid_default"])
        .err()
        .ok_or("a mismatched callable default must be rejected before Body IR construction")?;
    assert!(
        error.to_string().contains("Type mismatch: expected 'int', found 'str'"),
        "the source typechecker must reject the mismatched default before a Body-IR consumer sees it: {error}"
    );

    Ok(())
}

#[test]
fn refused_default_restores_ownership_state_before_local_ids_are_reused() -> Result<(), Box<dyn std::error::Error>> {
    // Lowering the partial moves one of its synthesized forwarding locals before the unsupported binary refuses.
    // The transaction must discard that move before `second` reuses the local id in the normal body, or the
    // required root-scope drop would silently disappear.
    let source = "def route(method: str) -> str:\n  return method\n\ndef choose(value: str = (partial route(method=\"GET\")) + missing) -> str:\n  first = \"first\"\n  second = \"second\"\n  return first\n";
    let (module, _diagnostics) = build_after_expected_typecheck_errors(source, &["m", "default_ownership_rollback"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let value = choose.params.first().ok_or("expected choose's value parameter")?;
    let bir::CallableParamDefault::Unsupported { description, .. } = &value.default else {
        return Err(format!("the unsupported default must remain a refusal: {:?}", value.default).into());
    };
    assert!(
        description.contains("binary operator Add"),
        "the refusal must name the unsupported default operation: {description}"
    );
    let second = choose
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("second"))
        .ok_or("expected second binding after refused default")?;
    assert!(
        choose.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Drop { local } if *local == second.id
        )),
        "a stale speculative move must not suppress second's required drop: {choose:?}"
    );

    Ok(())
}

#[test]
fn invalid_callable_defaults_remain_body_ir_refusals_without_implicit_captures()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "earlier parameter",
            "def choose(first: str, second: str = first) -> str:\n  return second\n",
            "first",
        ),
        (
            "receiver",
            "model Label:\n  text: str\n\n  def choose(self, value: str = self.text) -> str:\n    return value\n",
            "self.text",
        ),
        (
            "bare field",
            "model Label:\n  text: str\n\n  def choose(self, value: str = text) -> str:\n    return value\n",
            "text",
        ),
        (
            "bare property",
            "model Label:\n  text: str\n\n  property display -> str:\n    return self.text\n\n  def choose(self, value: str = display) -> str:\n    return value\n",
            "display",
        ),
    ];

    for (case, source, default_spelling) in cases {
        let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "default_capture"])?;
        let rejected_name = match default_spelling.split('.').next() {
            Some(name) => name,
            None => default_spelling,
        };
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.contains(rejected_name)),
            "the source checker must reject the {case} default before Body IR: {diagnostics:?}"
        );
        let choose = module
            .bodies
            .iter()
            .find(|body| body.name == "choose")
            .ok_or("expected the choose Body IR")?;
        let parameter = choose.params.last().ok_or("expected the defaulted parameter")?;
        let bir::CallableParamDefault::Unsupported { span, description } = &parameter.default else {
            return Err(format!("a {case} default must not fabricate a callable-frame or instance capture").into());
        };
        let default_start = source
            .rfind(default_spelling)
            .ok_or("missing invalid default source spelling")?;
        assert_eq!(
            *span,
            HirSourceSpan::new(default_start, default_start + default_spelling.len()),
            "the {case} refusal must preserve the whole default expression span"
        );
        assert!(description.contains(rejected_name));
        assert!(
            !choose
                .locals
                .iter()
                .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
            "a direct consumer must not need an implicit lexical lookup for the refused {case} default: {choose:?}"
        );
    }

    Ok(())
}

#[test]
fn validated_newtype_default_remains_a_visible_body_ir_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let source = "type Attempts = newtype int:\n  def from_underlying(n: int) -> Result[Attempts, ValidationError]:\n    return Ok(Attempts(n))\n\ndef choose(value: Attempts = 3) -> Attempts:\n  return value\n";
    let module = build(source, &["m", "newtype_default"])?;
    let choose = module
        .bodies
        .iter()
        .find(|body| body.name == "choose")
        .ok_or("expected the choose Body IR")?;
    let value = choose.params.first().ok_or("expected the newtype default parameter")?;
    let bir::CallableParamDefault::Unsupported { span, description } = &value.default else {
        return Err("a default requiring validated-newtype coercion must not become a raw source computation".into());
    };
    let default_start = source.rfind("3)").ok_or("missing newtype default spelling")?;
    assert_eq!(*span, HirSourceSpan::new(default_start, default_start + 1));
    assert_eq!(
        description,
        "default requires a validated-newtype coercion Body IR does not yet represent"
    );
    assert_eq!(choose.locals.len(), 1);
    assert!(
        !choose
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "the newtype refusal must not leave a hidden source lookup in the callable body: {choose:?}"
    );

    Ok(())
}

#[test]
fn aliased_method_parameter_type_retains_the_checked_callable_type() -> Result<(), Box<dyn std::error::Error>> {
    // `UserId` is a type alias for `int` (RFC-style `type X = Y`). A naive re-parse of the raw `id: UserId`
    // annotation inside Body IR (with no alias table of its own) could only produce `Named("UserId")`; the
    // checked callable type resolves the alias all the way through, so the local must show `int`.
    let source = "type UserId = int\n\nmodel Account:\n  balance: int\n\n  def credit(self, id: UserId, amount: int) -> int:\n    return self.balance + amount\n";
    let module = build(source, &["m", "aliased_param"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 id : int [param]"),
        "an aliased parameter type must resolve through the alias like any other checked expression, not stay \
             the raw `UserId` annotation spelling: {snapshot}"
    );

    Ok(())
}

#[test]
fn generic_method_parameter_type_retains_the_owner_type_variable() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class Box[T]:\n  value: T\n\n  def replace(mut self, other: T) -> None:\n    self.value = other\n\n  def wrap(mut self, items: list[T]) -> None:\n    pass\n";
    let module = build(source, &["m", "generic_param"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 other : T [param]"),
        "a bare owner type-variable parameter must retain the checked type variable: {snapshot}"
    );
    assert!(
        snapshot.contains("local 1 items : List[T] [param]"),
        "a generic collection parameter must retain its checked element type variable: {snapshot}"
    );

    Ok(())
}

#[test]
fn static_method_parameter_types_are_recorded_like_ordinary_methods() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "model Counter:\n  value: int\n\n  def from_value(amount: int) -> Counter:\n    return Counter(value=amount)\n";
    let module = build(source, &["m", "static_param"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("body from_value decl:m::static_param::Counter::from_value"));
    assert!(
        !snapshot.contains("[receiver"),
        "a static/associated method (receiver: None) must not declare a receiver local: {snapshot}"
    );
    assert!(
        snapshot.contains("local 0 amount : int [param]"),
        "a static method's ordinary parameters must resolve the same way an instance method's do: {snapshot}"
    );

    Ok(())
}

#[test]
fn overloaded_method_declarations_retain_distinct_parameter_types_by_declaration_span()
-> Result<(), Box<dyn std::error::Error>> {
    // Two `add` methods on the same owner, distinguished only by adopting two instantiations of the same
    // generic trait (RFC 042 multi-instantiation) -- the language surface's one legitimate way to declare
    // same-name, same-owner method overloads with genuinely different parameter types. If the checked binding
    // table were keyed by `(owner, method_name)` alone (like `decorated_method_bindings`), the second
    // declaration would silently overwrite the first and both bodies would report the same parameter type.
    let source = "trait Adder[T]:\n  def add(self, x: T) -> T: ...\n\nmodel Calc with Adder[int], Adder[str]:\n  count: int\n\n  def add(self, x: int) -> int:\n    return x\n\n  def add(self, x: str) -> str:\n    return x\n";
    let module = build(source, &["m", "overload_param"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 x : int [param]"),
        "the int-instantiated overload must keep its own checked parameter type: {snapshot}"
    );
    assert!(
        snapshot.contains("local 1 x : str [param]"),
        "the str-instantiated overload must keep its own distinct checked parameter type, not collide with the \
             int overload recorded under the same method name: {snapshot}"
    );

    Ok(())
}

#[test]
fn method_parameter_type_falls_back_to_unknown_only_when_the_typechecker_binding_is_absent()
-> Result<(), Box<dyn std::error::Error>> {
    // A successful typecheck always populates `method_bindings_by_span` for every method Body IR actually
    // lowers a body for (see `TypeChecker::check_method_with_self_ty`), so the only way to observe the
    // fallback honestly is to simulate the checked fact genuinely being absent -- exercising the same
    // defense-in-depth path `lower_method_body` falls back to, rather than asserting on a state ordinary
    // typechecking can never produce.
    let source =
        "model Counter:\n  value: int\n\n  def add(self, amount: int) -> int:\n    return self.value + amount\n";
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = vec!["m".to_string(), "fallback_param".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let mut type_info = checker.type_info().clone();
    type_info.declarations.method_bindings_by_span.clear();

    let module = build_body_ir_module_v0(&program, &module_path, &type_info);
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("local 1 amount : ? [param]"),
        "with no recorded checked binding for this declaration, the parameter must fall back to the explicit \
             Unknown type rather than guessing from the raw annotation: {snapshot}"
    );

    Ok(())
}

/// Newtype and enum methods lower like any other owner's methods.
///
/// The body count is asserted explicitly so a future regression shows up as a mismatch rather than a silent
/// absence, which is how this gap went unnoticed: nothing failed when these bodies were simply never produced.
#[test]
fn newtype_and_enum_methods_contribute_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Meters = newtype int:
    def value(self) -> int:
        return self.0

    def scale(mut self, factor: int) -> None:
        self.0 = self.0 * factor

enum Signal:
    Idle
    Active(int)

    def code(self) -> int:
        return 0

def run() -> int:
    return 1
"#;
    let module = build(source, &["app"])?;

    let names: Vec<&str> = module.bodies.iter().map(|body| body.name.as_str()).collect();
    assert!(
        names.contains(&"value"),
        "newtype method `value` must lower, got {names:?}"
    );
    assert!(
        names.contains(&"scale"),
        "newtype `mut self` method must lower, got {names:?}"
    );
    assert!(names.contains(&"code"), "enum method `code` must lower, got {names:?}");
    assert!(names.contains(&"run"));
    assert_eq!(
        module.bodies.len(),
        4,
        "one body per method plus the free function, and nothing else: {names:?}"
    );

    // Each method's identity is scoped under its owning declaration, so two owners may share a method name.
    for (owner, method) in [("Meters", "value"), ("Meters", "scale"), ("Signal", "code")] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == method)
            .ok_or_else(|| Box::<dyn std::error::Error>::from(format!("`{method}` body missing")))?;
        assert!(
            body.decl_id.path().ends_with(&format!("{owner}::{method}")),
            "`{method}` must be scoped under `{owner}`, got {}",
            body.decl_id.path()
        );
    }
    Ok(())
}

/// An enum method that dispatches on `self`, including a payload variant, lowers a real body rather than an empty or
/// placeholder one.
#[test]
fn an_enum_method_dispatching_on_self_lowers_its_match() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
    Idle
    Active(int)

    def level(self) -> int:
        match self:
            case Signal.Idle:
                return 0
            case Signal.Active(amount):
                return amount
"#;
    let module = build(source, &["app"])?;

    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "level")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("enum method `level` must lower"))?;

    assert!(!body.block.stmts.is_empty(), "the method must lower a real body");
    // A refused construct also produces statements, so an empty check proves nothing on its own. The snapshot
    // carrying no `unsupported(` node is what distinguishes a lowered match from a placeholder.
    let rendered = module.render_snapshot();
    assert!(
        !rendered.contains("unsupported("),
        "the match and its payload binding must lower, not refuse:\n{rendered}"
    );
    Ok(())
}

/// A newtype method reading its wrapped value lowers through the nominal receiver.
#[test]
fn a_newtype_method_reads_its_wrapped_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Meters = newtype int:
    def doubled(self) -> int:
        return self.0 * 2
"#;
    let module = build(source, &["app"])?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "doubled")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("newtype method `doubled` must lower"))?;
    assert!(!body.block.stmts.is_empty());
    assert!(
        !module.render_snapshot().contains("unsupported("),
        "reading the wrapped value must lower, not refuse"
    );
    assert!(
        body.decl_id.path().ends_with("Meters::doubled"),
        "identity is scoped under the newtype, got {}",
        body.decl_id.path()
    );
    Ok(())
}

/// A newtype method's receiver is the newtype itself, and `mut self` stays a mutable receiver.
#[test]
fn newtype_method_receivers_are_receiver_locals() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Meters = newtype int:
    def value(self) -> int:
        return self.0

    def scale(mut self, factor: int) -> None:
        self.0 = self.0 * factor
"#;
    let module = build(source, &["m", "newtype_receiver"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("local 0 self : Meters [receiver]"), "{snapshot}");
    assert!(snapshot.contains("local 0 self : Meters [receiver_mut]"), "{snapshot}");
    assert!(
        !snapshot.contains("unsupported("),
        "newtype receiver reads and mutation must lower without a placeholder: {snapshot}"
    );
    Ok(())
}

/// An enum method's receiver is the enum value it dispatches on.
#[test]
fn enum_method_receiver_is_a_receiver_local() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
    Idle
    Active(int)

    def code(self) -> int:
        return 0
"#;
    let module = build(source, &["m", "enum_receiver"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("local 0 self : Signal [receiver]"), "{snapshot}");
    Ok(())
}

/// A bodyless newtype or enum method never reaches lowering: the source checker rejects it first.
///
/// Only a trait declaration admits a bodyless method, so the "abstract method contributes nothing" boundary sits
/// upstream for these two kinds rather than in the lowering walk. Lowering still has to stay total for the rejected
/// program, which is what this pins — no body, and no `Unsupported` placeholder standing in for one.
#[test]
fn a_bodyless_newtype_or_enum_method_is_a_source_error_and_lowers_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Signal:
    Idle

    def label(self) -> str: ...
"#;
    let (module, errors) = build_after_expected_typecheck_errors(source, &["m", "bodyless"])?;

    assert!(
        errors
            .iter()
            .any(|error| error.contains("must have a body outside trait declarations")),
        "the source checker owns this refusal: {errors:?}"
    );
    assert!(
        module.bodies.is_empty(),
        "a rejected bodyless method must produce neither a body nor a placeholder: {:?}",
        module.bodies.iter().map(|body| body.name.as_str()).collect::<Vec<_>>()
    );
    Ok(())
}

/// Every declaration kind that carries a `methods` field lowers its bodies.
///
/// A skipped kind is the one coverage failure this module cannot make visible — it produces no `Body` at all rather
/// than an `Unsupported` marker — so the exhaustive set is pinned here rather than left to the walk's `_ => ` arm.
#[test]
fn every_declaration_kind_that_carries_methods_lowers_its_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
trait Describable:
    def describe(self) -> int:
        return 0

model Point:
    x: int

    def get_x(self) -> int:
        return self.x

class Counter:
    total: int

    def total_now(self) -> int:
        return self.total

type Meters = newtype int:
    def raw(self) -> int:
        return self.0

enum Signal:
    Idle

    def code(self) -> int:
        return 1
"#;
    let module = build(source, &["m", "all_owners"])?;
    let names: Vec<&str> = module.bodies.iter().map(|body| body.name.as_str()).collect();

    for method in ["describe", "get_x", "total_now", "raw", "code"] {
        assert!(names.contains(&method), "`{method}` must lower, got {names:?}");
    }
    assert_eq!(
        module.bodies.len(),
        5,
        "one body per methods-carrying declaration kind and nothing else: {names:?}"
    );
    Ok(())
}

#[test]
fn a_self_typed_parameter_in_a_types_own_method_reads_as_that_type() -> Result<(), Box<dyn std::error::Error>> {
    // `other: Self` in a model's own method is the model, so its fields have their declared types and a comparison of
    // them is a primitive operation, as on `self`. Before this the field read was untyped and the comparison refused.
    let source = "model P:\n  v: int\n\n  def same(self, other: Self) -> bool:\n    return self.v == other.v\n\ndef main() -> None:\n  println(P(v=1).same(P(v=1)))\n";
    let module = build(source, &["m", "self_param"])?;
    let same = body_named(&module, "same")?;
    let rendered = same.render_snapshot();
    assert!(!rendered.contains("unsupported("), "{rendered}");
    let other = same
        .params
        .iter()
        .find(|param| param.name == "other")
        .ok_or("`other` must be a parameter")?;
    assert_eq!(other.ty, IncanType::Named("P".to_string()), "{rendered}");
    Ok(())
}

/// Concrete owner methods resolve Self through nested return types, while trait defaults retain it for adopter
/// specialization.
#[test]
fn a_self_return_type_in_a_types_own_method_reads_as_that_type() -> Result<(), Box<dyn std::error::Error>> {
    // A declared type's own method returning `Self` returns that type, nested in a collection too, as the checker reads
    // it at every call site; only a trait default keeps `Self` open for specialization per adopter.
    let source = concat!(
        "model P:\n",
        "  v: int\n\n",
        "  def again(self) -> Self:\n",
        "    return P(v=self.v)\n\n",
        "  def both(self) -> list[Self]:\n",
        "    return [P(v=self.v)]\n\n",
        "trait T:\n",
        "  def me(self) -> Self:\n",
        "    return self\n",
    );
    let module = build(source, &["m", "self_return"])?;
    let again = body_named(&module, "again")?;
    assert_eq!(again.return_type, IncanType::Named("P".to_string()));
    let both = body_named(&module, "both")?;
    assert_eq!(
        both.return_type,
        IncanType::Generic {
            base: "List".to_string(),
            args: vec![IncanType::Named("P".to_string())],
        }
    );
    let me = body_named(&module, "me")?;
    assert_eq!(me.return_type, IncanType::SelfType, "a trait default keeps `Self` open");
    Ok(())
}

/// Canonical collection mutation records exclusive receiver facts while read-only members remain shared.
#[test]
fn collection_mutating_methods_retain_mutable_receiver_facts() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def update(mut values: list[int], mut unique: Set[int], mut mapping: Dict[str, int]) -> None:\n  values.append(3)\n  values.swap(0, 0)\n  count = values.count(3)\n  unique.add(3)\n  present = unique.contains(3)\n  mapping.insert(\"key\", 3)\n  keys = mapping.keys()\n  found = mapping.contains_key(\"key\")\n",
        &["m", "collection_receivers"],
    )?;
    let body = body_named(&module, "update")?;
    let mut facts = Vec::new();
    for statement in &body.block.stmts {
        if let bir::StatementKind::Call {
            callee: bir::Callee::Method(target),
            args,
            ..
        } = &statement.kind
        {
            let receiver = args
                .first()
                .and_then(bir::ArgumentElement::as_one)
                .ok_or("collection method lacks a receiver")?;
            let bir::Operand::Place(read) = receiver else {
                return Err("collection receiver must be a place".into());
            };
            facts.push((target.name.as_str(), read.fact));
        }
    }
    assert_eq!(
        facts,
        vec![
            ("append", bir::OwnershipFact::MutBorrow),
            ("swap", bir::OwnershipFact::MutBorrow),
            ("count", bir::OwnershipFact::Borrow),
            ("add", bir::OwnershipFact::MutBorrow),
            ("contains", bir::OwnershipFact::Borrow),
            ("insert", bir::OwnershipFact::MutBorrow),
            ("keys", bir::OwnershipFact::Borrow),
            ("contains_key", bir::OwnershipFact::Borrow)
        ]
    );
    Ok(())
}

/// Returning shared or mutable collection parameters clones caller storage rather than moving or dropping it.
#[test]
fn collection_parameter_last_reads_clone_without_dropping_caller_storage() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def shared(values: list[int]) -> list[int]:\n  return values\n\ndef mutable(mut values: list[int]) -> list[int]:\n  return values\n\ndef shared_set(values: Set[int]) -> Set[int]:\n  return values\n\ndef mutable_set(mut values: Set[int]) -> Set[int]:\n  return values\n\ndef shared_dict(values: Dict[str, int]) -> Dict[str, int]:\n  return values\n\ndef mutable_dict(mut values: Dict[str, int]) -> Dict[str, int]:\n  return values\n",
        &["m", "collection_parameter_reads"],
    )?;
    for name in [
        "shared",
        "mutable",
        "shared_set",
        "mutable_set",
        "shared_dict",
        "mutable_dict",
    ] {
        let body = body_named(&module, name)?;
        let local = body.params.first().ok_or("missing collection parameter")?.local;
        let mut returns = 0;
        for statement in &body.block.stmts {
            match &statement.kind {
                bir::StatementKind::Return {
                    value: Some(bir::Operand::Place(read)),
                } => {
                    assert_eq!(read.place.local_id(), Some(local));
                    assert_eq!(read.fact, bir::OwnershipFact::Clone);
                    assert!(!read.last_use);
                    returns += 1;
                }
                bir::StatementKind::Drop { local: dropped } => assert_ne!(*dropped, local),
                _ => {}
            }
        }
        assert_eq!(returns, 1);
    }
    Ok(())
}

/// Concrete adopters retain canonical trait slots and distinguish required implementations from defaults.
#[test]
fn concrete_trait_implementation_facts_preserve_default_and_required_targets() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"trait Reading:
  def get(self) -> int: ...
  def bump(mut self, amount: int) -> None: ...
  def advance(mut self) -> None:
    self.bump(1)
  def doubled(self) -> int:
    return self.get() + self.get()

class Counter with Reading:
  value: int
  def get(self) -> int:
    return self.value
  def bump(mut self, amount: int) -> None:
    self.value += amount

def main() -> None:
  mut counter = Counter(value=7)
  counter.advance()
  println(counter.get())
"#;
    let module = build(source, &["m", "concrete_traits"])?;
    assert_eq!(module.trait_declarations.len(), 1);
    assert_eq!(module.trait_implementations.len(), 4);
    for implementation in &module.trait_implementations {
        assert!(module.is_well_formed_trait_implementation(implementation));
        assert_eq!(implementation.owner.declaration_name, "Counter");
        assert_eq!(
            implementation.method == implementation.implementation,
            matches!(implementation.method.declaration_name.as_str(), "doubled" | "advance")
        );
        let mut tampered = implementation.clone();
        tampered.owner.declaration_span.start += 1;
        assert!(!module.is_well_formed_trait_implementation(&tampered));
    }
    for body in &module.bodies {
        assert!(module.body_has_canonical_direct_call_id(body));
    }
    let doubled = body_named(&module, "doubled")?;
    let required = module
        .trait_implementations
        .iter()
        .find(|implementation| implementation.method.declaration_name == "get")
        .ok_or("the required trait slot must be retained")?;
    let mut calls = 0;
    for statement in &doubled.block.stmts {
        if let bir::StatementKind::Call {
            callee: bir::Callee::Method(target),
            ..
        } = &statement.kind
        {
            assert_eq!(target.canonical.as_ref(), Some(&required.method));
            calls += 1;
        }
    }
    assert_eq!(calls, 2, "both calls in the default must retain the checked trait slot");
    let advance = body_named(&module, "advance")?;
    assert!(matches!(
        advance.locals[0].origin,
        bir::LocalOrigin::Receiver { mutable: true }
    ));
    let bump = module
        .trait_implementations
        .iter()
        .find(|implementation| implementation.method.declaration_name == "bump")
        .ok_or("the mutable required trait slot must be retained")?;
    let target = advance
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("the mutable default must retain its method call")?;
    assert_eq!(target.canonical.as_ref(), Some(&bump.method));
    Ok(())
}

/// Supertrait slots and defaults of a trait with type parameters retain the body that fills them per adopter.
#[test]
fn trait_implementation_facts_cover_supertrait_slots_and_checked_instantiations()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"trait Named:
  def name(self) -> str: ...

trait Tag with Named:
  def loud(self) -> str:
    return self.name() + "!"

trait Root:
  def label(self) -> str: ...

trait Child with Root:
  def label(self) -> str:
    return "child"

trait Holder[T]:
  def get(self) -> T: ...
  def pair(self) -> list[T]:
    return [self.get(), self.get()]

model Label with Tag:
  text: str
  def name(self) -> str:
    return self.text

model Item with Child:
  value: int

model Box with Holder[int]:
  value: int
  def get(self) -> int:
    return self.value

def main() -> None:
  println(Label(text="x").loud())
  println(Item(value=1).label())
  println(len(Box(value=4).pair()))
"#;
    let module = build(source, &["m", "trait_hierarchies"])?;
    for implementation in &module.trait_implementations {
        assert!(
            module.is_well_formed_trait_implementation(implementation),
            "{implementation:?}"
        );
    }
    let slot = |owner: &str, method: &str| {
        module
            .trait_implementations
            .iter()
            .filter(|implementation| {
                implementation.owner.declaration_name == owner && implementation.method.declaration_name == method
            })
            .collect::<Vec<_>>()
    };

    // ---- A supertrait slot the adopter implements itself ----
    let [name] = slot("Label", "name")[..] else {
        return Err("the supertrait slot must be retained once for its adopter".into());
    };
    assert_ne!(name.method, name.implementation);
    assert!(name.type_arguments.is_empty());

    // ---- A supertrait slot a subtrait default fills ----
    let filled = slot("Item", "label");
    assert_eq!(
        filled.len(),
        2,
        "both the supertrait and the subtrait slot are retained: {filled:?}"
    );
    let child_default = filled
        .iter()
        .find(|implementation| implementation.method == implementation.implementation)
        .ok_or("the subtrait's own slot keeps its default")?;
    assert!(
        filled
            .iter()
            .all(|implementation| implementation.implementation == child_default.implementation),
        "the supertrait slot is filled by the subtrait default: {filled:?}"
    );

    // ---- A default of a trait with type parameters carries its checked instantiation ----
    let [pair] = slot("Box", "pair")[..] else {
        return Err("the generic trait default must be retained for its adopter".into());
    };
    assert_eq!(pair.method, pair.implementation);
    assert_eq!(
        pair.type_arguments,
        vec![bir::TraitTypeArgument {
            parameter: "T".to_owned(),
            argument: IncanType::Primitive(IncanPrimitiveType::Int),
        }]
    );
    let [get] = slot("Box", "get")[..] else {
        return Err("the required generic trait slot must be retained".into());
    };
    assert!(get.type_arguments.is_empty());
    Ok(())
}
