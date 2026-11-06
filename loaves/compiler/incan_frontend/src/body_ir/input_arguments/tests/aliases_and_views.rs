//! Local aliases, borrowed parameter operations, call profiles, method frames and returned views.

use super::*;

/// An owned local transfer makes later source use invalid; shared inputs and local loans cannot be consumed.
#[test]
fn owned_local_source_conflicts_remain_typed() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def main() -> int:\n    original = \"payload\"\n    let held: Owned[str] = original\n    return len(original)\n",
        "def read(original: Borrowed[str]) -> int:\n    let held: Owned[str] = original\n    return len(held)\n",
        "def read(original: Owned[str]) -> str:\n    let view: Borrowed[str] = original\n    return view\n",
    ] {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("owned source conflict was accepted")?;
        assert!(
            errors.iter().any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{errors:?}"
        );
    }
    Ok(())
}

/// A non-Copy alias borrows its original owner for a direct call, then permits transfer after its final use.
#[test]
fn noncopy_local_alias_retains_owner_until_last_use() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "def length(value: Borrowed[str]) -> int:\n    return len(value)\ndef main() -> int:\n    original = \"payload\"\n    let view: Borrowed[str] = original\n    assert length(view) == 7\n    let held: Owned[str] = original\n    return len(held)\n",
        false,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no main body")?;
    assert!(
        body.first_representation_gap().is_none(),
        "{:?}",
        body.first_representation_gap()
    );
    assert!(!body.locals.iter().any(|local| local.name.as_deref() == Some("view")));
    Ok(())
}

/// An exclusive model alias projects nested fields into the original aggregate without a second owner.
#[test]
fn noncopy_model_alias_keeps_nested_storage() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "model Page:\n    title: str\n    count: int\ndef main() -> None:\n    mut page = Page(title=\"old\", count=1)\n    mut view: BorrowedMut[Page] = page\n    view.title = \"new\"\n    view.count += 1\n    assert view.title == \"new\"\n    assert page.count == 2\n",
        false,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no main body")?;
    assert!(
        body.first_representation_gap().is_none(),
        "{:?}",
        body.first_representation_gap()
    );
    assert!(!body.locals.iter().any(|local| local.name.as_deref() == Some("view")));
    Ok(())
}

/// Observers consume alias operands after evaluating the other operands, without a hidden owned snapshot.
#[test]
fn noncopy_alias_operation_retains_loan_through_later_arguments() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "print(view, replace(original))",
        "assert view == replace(original)",
        "print(f\"{view}{replace(original)}\")",
    ] {
        let source = format!(
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef main() -> None:\n    mut original = \"payload\"\n    let view: Borrowed[str] = original\n    {expression}\n"
        );
        let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("later mutation released alias too early")?;
        assert!(
            errors.iter().any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{expression}: {errors:?}"
        );
    }
    Ok(())
}

/// Disjoint mutation is permitted, and a fresh formatted value ends its source dependency before the next argument.
#[test]
fn noncopy_alias_observers_preserve_disjoint_and_completed_reads() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "print(view)",
        "print(view, replace(other))",
        "print(replace(other), view)",
        "assert view != replace(other)",
        "print(f\"{view}{replace(other)}\")",
        "print(f\"{view}\", replace(original))",
    ] {
        let source = format!(
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef main() -> None:\n    mut original = \"payload\"\n    mut other = \"other\"\n    let view: Borrowed[str] = original\n    {expression}\n"
        );
        let module = staged(&source, false)?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "main")
            .ok_or("missing main")?;
        assert!(
            body.first_representation_gap().is_none(),
            "{expression}: {:?}",
            body.first_representation_gap()
        );
    }
    Ok(())
}

/// Explicit borrowed parameters retain their original storage at a primitive observer, without a repair snapshot.
#[test]
fn borrowed_text_parameter_observers_keep_source_storage() -> Result<(), Box<dyn std::error::Error>> {
    for mode in ["Borrowed", "BorrowedMut"] {
        let module = admitted(&format!(
            "def read(value: {mode}[str]) -> int:\n    return len(value)\n"
        ))?;
        let body = module.bodies.first().ok_or("missing body")?;
        let arguments = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Call { args, .. } => Some(args),
                _ => None,
            })
            .ok_or("missing observer")?;
        let [bir::ArgumentElement::One(bir::Operand::Place(read))] = arguments.as_slice() else {
            return Err("missing observer operand".into());
        };
        assert_eq!(read.fact, bir::OwnershipFact::Borrow, "{mode}");
        assert_eq!(
            read.place.local_id(),
            body.params.first().map(|parameter| parameter.local),
            "{mode}"
        );
        assert!(!read.last_use);
    }
    Ok(())
}

/// A direct exclusive parameter's observed text remains shared until later operands finish evaluating.
#[test]
fn borrowed_parameter_operation_refuses_later_mutation() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "print(value, replace(value))",
        "assert value == replace(value)",
        "print(f\"{value}{replace(value)}\")",
    ] {
        let source = format!(
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str]) -> None:\n    {expression}\n"
        );
        let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("borrowed operand was silently snapshotted")?;
        assert!(
            errors.iter().any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{expression}: {errors:?}"
        );
    }
    Ok(())
}

/// An exclusive alias cannot use its parent capability to mutate a text operand still shared by an operation.
#[test]
fn exclusive_alias_operation_requires_a_shared_child() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str]) -> None:\n    mut view: BorrowedMut[str] = value\n    print(view, replace(view))\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("exclusive parent bypassed operation read")?;
    assert!(
        errors.iter().any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{errors:?}"
    );
    Ok(())
}

/// Disjoint parameter mutations and completed inner values remain usable; Copy snapshots retain ordinary behavior.
#[test]
fn borrowed_parameter_operations_keep_disjoint_and_copy_behavior() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "print(value, value)",
        "print(value, replace(other))",
        "print(replace(other), value)",
        "assert value != replace(other)",
        "print(f\"{value}{replace(other)}\")",
        "print(f\"{value}\", replace(value))",
    ] {
        let source = format!(
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str], mut other: BorrowedMut[str]) -> None:\n    {expression}\n"
        );
        let module = admitted(&source)?;
        assert!(
            module
                .bodies
                .iter()
                .all(|body| body.first_representation_gap().is_none()),
            "{expression}"
        );
    }
    let module = admitted(
        "def bump(mut value: BorrowedMut[int]) -> int:\n    value += 1\n    return value\ndef inspect(mut value: BorrowedMut[int]) -> None:\n    print(value, bump(value))\n",
    )?;
    assert!(
        module
            .bodies
            .iter()
            .all(|body| body.first_representation_gap().is_none())
    );
    Ok(())
}

/// A plain Copy helper keeps its inferred declaration ABI, while an explicit local still gives its caller a profile.
/// Native persistence of that local-only obligation is a separate representation acceptance gate.
#[test]
fn ordinary_copy_helper_retains_caller_profile_and_value_operand() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "def add(value: int) -> int:\n    return value + 1\ndef use(value: int) -> int:\n    let view: Borrowed[int] = value\n    return add(view)\n",
    )?;
    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "use")
        .ok_or("missing caller")?;
    let callee = module
        .bodies
        .iter()
        .find(|body| body.name == "add")
        .ok_or("missing callee")?;
    assert!(
        caller
            .params
            .iter()
            .all(|parameter| parameter.input_access.is_inferred())
    );
    assert!(
        callee
            .params
            .iter()
            .all(|parameter| parameter.input_access.is_inferred())
    );
    assert!(
        caller
            .checked_input_profile
            .as_ref()
            .is_some_and(|profile| profile.version == 1)
    );
    assert!(callee.checked_input_profile.is_none());
    let argument = caller
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call { args, .. } => args.first(),
            _ => None,
        })
        .ok_or("missing helper call")?;
    assert!(
        matches!(argument, bir::ArgumentElement::One(bir::Operand::Place(read)) if read.fact == bir::OwnershipFact::Copy)
    );
    Ok(())
}

/// An explicit local in an otherwise ordinary body keeps its obligation on the body: losing the checked profile
/// refuses the body and its call to an ordinary helper instead of leaving a body that no longer looks explicit.
#[test]
fn explicit_local_requires_profile_when_callee_is_ordinary() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "def add(value: int) -> int:\n    return value + 1\ndef use(value: int) -> int:\n    let view: Borrowed[int] = value\n    return add(view)\n",
    )?;
    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "use")
        .ok_or("missing caller")?;
    let callee = module
        .bodies
        .iter()
        .find(|body| body.name == "add")
        .ok_or("missing callee")?;
    assert!(
        caller
            .params
            .iter()
            .all(|parameter| parameter.input_access.is_inferred())
    );
    assert!(
        callee
            .params
            .iter()
            .all(|parameter| parameter.input_access.is_inferred())
    );
    assert!(caller.checked_input_profile.is_some());
    assert!(matches!(
        caller.explicit_input_bindings.as_slice(),
        [bir::ExplicitInputBinding {
            identity: Some(_),
            access: incan_semantics_core::IncanInputAccess::Shared,
            ..
        }]
    ));
    assert!(callee.explicit_input_bindings.is_empty());
    assert!(caller.input_profile_gap().is_none());
    assert!(caller.input_call_profile_gap(callee).is_none());
    let mut stripped = caller.clone();
    stripped.checked_input_profile = None;
    assert!(
        stripped.input_profile_gap().is_some(),
        "explicit local obligation was erased with the profile"
    );
    assert!(stripped.input_call_profile_gap(callee).is_some());
    assert!(stripped.first_representation_gap().is_some());
    Ok(())
}

/// A method body is a checked frame: its receiver and explicit inputs borrow the caller's existing storage, and the
/// caller materializes the receiver from the checked invocation rather than the ordinary receiver route.
#[test]
fn method_frames_borrow_receiver_and_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "class Counter:\n    pub value: int\n\n    def bump(mut self, mut amount: BorrowedMut[int]) -> None:\n        self.value = self.value + amount\n        amount = 0\n\ndef main() -> int:\n    mut counter = Counter(value=1)\n    mut step = 2\n    counter.bump(step)\n    return counter.value + step\n",
    )?;
    let method = module
        .bodies
        .iter()
        .find(|body| body.name == "bump")
        .ok_or("no method")?;
    let profile = method
        .checked_input_profile
        .as_ref()
        .ok_or("method has no checked profile")?;
    assert_eq!(profile.parameters.len(), 2);
    assert_eq!(
        profile.parameters[1].access,
        incan_semantics_core::IncanInputAccess::Exclusive
    );
    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no caller")?;
    assert!(caller.checked_input_profile.is_some());
    let mut facts = Vec::new();
    for statement in &caller.block.stmts {
        if let bir::StatementKind::Call {
            callee: bir::Callee::Method(_),
            args,
            ..
        } = &statement.kind
        {
            for argument in args {
                let bir::ArgumentElement::One(bir::Operand::Place(read)) = argument else {
                    return Err("method input is not a place".into());
                };
                let local = caller
                    .locals
                    .get(read.place.local_id().ok_or("missing source local")?.index())
                    .ok_or("missing local")?;
                facts.push((local.name.clone(), read.fact));
            }
        }
    }
    assert_eq!(
        facts,
        [
            (Some("counter".into()), bir::OwnershipFact::MutBorrow),
            (Some("step".into()), bir::OwnershipFact::MutBorrow),
        ]
    );
    Ok(())
}

/// A view body returns a borrow of its declared place, and a caller runs the view call for its effects and aliases
/// the substituted place: the later receiver is the original field, not a copy or a reference-valued local.
#[test]
fn returned_views_alias_the_substituted_place() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "class Address:\n    pub city: str\n\n    def set_city(mut self, city: str) -> Reference[Self] from self:\n        self.city = city\n        return self\n\nclass Customer:\n    pub name: str\n    pub address: Address\n\n    def edit_address(mut self) -> Reference[Address] from self.address:\n        return self.address\n\ndef main() -> None:\n    mut customer = Customer(name=\"Sam\", address=Address(city=\"Utrecht\"))\n    mut address = customer.edit_address()\n    customer.name = \"Samuel\"\n    address.set_city(\"Rotterdam\")\n    assert customer.address.city == \"Rotterdam\"\n",
    )?;
    let body = |name: &str| {
        module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or_else(|| format!("missing {name}"))
    };
    let edit = body("edit_address")?;
    let view = edit.returned_view.as_ref().ok_or("edit_address returns no view")?;
    assert_eq!(
        (view.slot, view.access),
        (0, incan_semantics_core::IncanInputAccess::Exclusive)
    );
    let returned = edit
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Return {
                value: Some(bir::Operand::Place(read)),
            } => Some(read),
            _ => None,
        })
        .ok_or("edit_address returns no place")?;
    assert_eq!(returned.fact, bir::OwnershipFact::MutBorrow);
    assert_eq!(returned.place.projection.len(), 1);
    // A view body returns its source's value type, here both `Address`; the returned view carries the access.
    let address = &body("set_city")?.return_type;
    assert!(
        matches!(address, incan_semantics_core::IncanType::Named(_)),
        "{address:?}"
    );
    assert_eq!(&edit.return_type, address);
    let caller = body("main")?;
    assert!(caller.checked_input_profile.is_some());
    let mut calls = Vec::new();
    for statement in &caller.block.stmts {
        if let bir::StatementKind::Call {
            destination,
            callee: bir::Callee::Method(target),
            args,
            ..
        } = &statement.kind
        {
            let Some(bir::ArgumentElement::One(bir::Operand::Place(receiver))) = args.first() else {
                return Err("method call has no place receiver".into());
            };
            let local = caller
                .locals
                .get(receiver.place.local_id().ok_or("receiver has no local")?.index())
                .ok_or("missing receiver local")?;
            calls.push((
                target.name.clone(),
                destination.is_none(),
                local.name.clone(),
                receiver.place.projection.len(),
            ));
        }
    }
    assert_eq!(
        calls,
        [
            ("edit_address".into(), true, Some("customer".into()), 0),
            ("set_city".into(), true, Some("customer".into()), 1),
        ]
    );
    Ok(())
}

/// A fluent chain continues on the inner view's substituted place: both calls receive the original nested field,
/// and a view body may return another view of exactly its declared source.
#[test]
fn chained_views_continue_on_the_original_field() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "class Address:\n    pub city: str\n    pub postcode: str\n\n    def set_city(mut self, city: str) -> Reference[Self] from self:\n        self.city = city\n        return self\n\n    def set_postcode(mut self, postcode: str) -> Reference[Self] from self:\n        self.postcode = postcode\n        return self\n\n    def relocate(mut self) -> Reference[Self] from self:\n        return self.set_city(\"Delft\")\n\nclass Customer:\n    pub name: str\n    pub address: Address\n\ndef main() -> None:\n    mut customer = Customer(name=\"Sam\", address=Address(city=\"Utrecht\", postcode=\"3511\"))\n    customer.address.set_city(\"Rotterdam\").set_postcode(\"3011\")\n    customer.address.relocate()\n    print(customer.address.city)\n",
    )?;
    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("missing main")?;
    let mut receivers = Vec::new();
    for statement in &caller.block.stmts {
        if let bir::StatementKind::Call {
            destination: None,
            callee: bir::Callee::Method(target),
            args,
            ..
        } = &statement.kind
            && let Some(bir::ArgumentElement::One(bir::Operand::Place(receiver))) = args.first()
        {
            receivers.push((target.name.clone(), receiver.place.projection.len(), receiver.fact));
        }
    }
    assert_eq!(
        receivers,
        [
            ("set_city".into(), 1, bir::OwnershipFact::MutBorrow),
            ("set_postcode".into(), 1, bir::OwnershipFact::MutBorrow),
            ("relocate".into(), 1, bir::OwnershipFact::MutBorrow),
        ]
    );
    // Discarded view calls run for their effects only; their substituted places are never read as values.
    assert!(!caller.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Expr { value: bir::Operand::Place(read) } if read.fact == bir::OwnershipFact::MutBorrow
    )));
    let relocate = module
        .bodies
        .iter()
        .find(|body| body.name == "relocate")
        .ok_or("missing relocate")?;
    assert!(relocate.returned_view.is_some());
    assert!(relocate.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Return { value: Some(bir::Operand::Place(read)) }
            if read.fact == bir::OwnershipFact::MutBorrow && read.place.projection.is_empty()
    )));
    Ok(())
}
