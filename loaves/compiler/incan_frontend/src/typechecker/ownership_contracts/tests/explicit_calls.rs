//! Explicit ownership contracts across methods, headers, call arguments, call replay and call-site facts.

use super::*;

/// A selected method closes Self in its ownership contract using the same receiver as its parameter types; a
/// shared receiver and a shared argument may borrow the same owner.
#[test]
fn method_input_payload_retains_checked_self_substitution() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "class Reader:\n    def view(self, other: Borrowed[Self]) -> None:\n        pass\ndef run() -> None:\n    reader = Reader()\n    reader.view(reader)\n",
    )?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert_eq!(checker.type_info.protocols.explicit_input_calls.len(), 1);
    Ok(())
}

/// Methods retain their own receiver and parameter identities; local loans must not disappear merely because
/// their callable lives inside a model rather than at module scope.
#[test]
fn method_flow_preserves_receiver_permissions() -> Result<(), Box<dyn std::error::Error>> {
    for (source, conflict) in [
        (
            "model Counter:\n    value: int\n    def change(mut self, amount: Borrowed[int]) -> int:\n        mut view: BorrowedMut[int] = self.value\n        view += amount\n        return self.value\n",
            false,
        ),
        (
            "model Counter:\n    value: int\n    def change(self, amount: Borrowed[int]) -> int:\n        mut view: BorrowedMut[int] = self.value\n        view += amount\n        return self.value\n",
            true,
        ),
        (
            "model Counter:\n    value: int\n    def read(self) -> int:\n        let view: Borrowed[int] = self.value\n        return view\n",
            false,
        ),
    ] {
        let checker = checked(source)?;
        assert_eq!(checker.type_info.protocols.explicit_input_flow.len(), 1, "{source}");
        let output = checker
            .type_info
            .protocols
            .explicit_input_flow
            .values()
            .next()
            .ok_or("missing method flow")?;
        let InputFlow::Checked { analysis, .. } = output else {
            return Err(format!("method flow unavailable: {output:?}\n{source}").into());
        };
        assert_eq!(!analysis.conflicts.is_empty(), conflict, "{source}\n{analysis:?}");
        assert_eq!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            conflict
        );
        // A method frame without conflicts is admitted; a conflicting one has no executable proof.
        assert_eq!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            conflict,
            "{source}: {:?}",
            checker.errors
        );
    }
    let checker = checked(
        "model First:\n    value: int\n    def read(self) -> int:\n        let view: Borrowed[int] = self.value\n        return view\nmodel Second:\n    value: int\n    def read(self) -> int:\n        let view: Borrowed[int] = self.value\n        return view\n",
    )?;
    assert_eq!(checker.type_info.protocols.explicit_input_flow.len(), 2);
    let mut receivers = std::collections::BTreeSet::new();
    for output in checker.type_info.protocols.explicit_input_flow.values() {
        let InputFlow::Checked { graph, analysis } = output else {
            return Err(format!("missing distinct owner flow: {output:?}").into());
        };
        assert!(analysis.conflicts.is_empty());
        receivers.extend(
                graph
                    .permissions
                    .keys()
                    .filter(|root| matches!(root, super::flow::Root::Binding(identity) if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Receiver)),
            );
    }
    assert_eq!(
        receivers.len(),
        2,
        "same-spelled methods must retain distinct receivers"
    );
    Ok(())
}

/// Conflicts are source diagnostics with the actual operation and the dependency explaining its lifetime;
/// stable secondary anchors must survive independently of message wording or native compilation.
#[test]
fn explicit_conflicts_report_origin_and_dependent_use() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def conflicting(mut value: Owned[int]) -> int:\n    let view: Borrowed[int] = value\n    value = 2\n    return view\n";
    let checker = checked(source)?;
    let conflict = checker
        .errors
        .iter()
        .find(|error| error.stable_code() == Some("INCAN-T0126"))
        .ok_or("missing source ownership conflict")?;
    assert_eq!(source.get(conflict.span.start..conflict.span.end), Some("value"));
    let related = conflict.related_spans();
    assert_eq!(related.len(), 2, "{conflict:?}");
    assert_eq!(source.get(related[0].span.start..related[0].span.end), Some("value"));
    assert_eq!(source.get(related[1].span.start..related[1].span.end), Some("view"));
    assert!(related[0].span.start < conflict.span.start);
    assert!(related[1].span.start > conflict.span.start);
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0123"))
    );

    let source = "def transfer(value: Owned[str]) -> str:\n    first = value\n    return value\n";
    let checker = checked(source)?;
    let conflict = checker
        .errors
        .iter()
        .find(|error| error.stable_code() == Some("INCAN-T0126"))
        .ok_or("missing moved source conflict")?;
    assert_eq!(source.get(conflict.span.start..conflict.span.end), Some("value"));
    assert_eq!(conflict.related_spans().len(), 1);
    assert!(conflict.related_spans()[0].span.start < conflict.span.start);

    let source = "def readonly(value: Borrowed[int]) -> None:\n    value = 2\n";
    let checker = checked(source)?;
    let conflict = checker
        .errors
        .iter()
        .find(|error| error.stable_code() == Some("INCAN-T0126"))
        .ok_or("missing permission conflict")?;
    assert!(conflict.related_spans().is_empty());
    assert_eq!(conflict.related_declarations().len(), 1);
    assert_eq!(conflict.related_declarations()[0].identity.declaration_name, "value");

    let source = "def branching(mut value: Owned[int], flag: bool) -> int:\n    let view: Borrowed[int] = value\n    if flag:\n        value = 2\n        return view\n    return view\n";
    let checker = checked(source)?;
    let conflict = checker
        .errors
        .iter()
        .find(|error| error.stable_code() == Some("INCAN-T0126"))
        .ok_or("missing branch conflict")?;
    assert_eq!(conflict.related_spans().len(), 2);
    let witness = conflict.related_spans()[1].span;
    assert_eq!(
        witness.start,
        source.find("return view").ok_or("missing branch use")? + 7
    );
    Ok(())
}

/// Later callees and overloads expose their exact obligations before any body executes; malformed sources
/// remain unavailable and are diagnosed by their declaration-owned check, without duplicate source errors.
#[test]
fn function_headers_supply_ownership_before_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def caller() -> None:\n    pass\ndef view(value: Borrowed[Page]) -> Borrowed[str] from value.title:\n    return value.title\ndef view(prefix: str, value: Borrowed[str]) -> Borrowed[str] from value:\n    return value\ndef invalid(value: Borrowed[str]) -> Borrowed[str] from absent:\n    return value\nmodel Page:\n    title: str\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.reset_program_check(&program, false);
    checker.collect_program_check_declarations(&program);
    assert_eq!(checker.check_work.function_declarations, 0);
    let contracts = &checker.type_info.declarations.ownership_contracts;
    assert_eq!(contracts.len(), 2, "{contracts:?}");
    for (index, declaration) in program.declarations[1..3].iter().enumerate() {
        let binding = checker
            .type_info
            .declarations
            .function_bindings_by_span
            .get(&declaration.span)
            .ok_or("missing overload header")?;
        let identity = binding.identity.as_ref().ok_or("missing exact overload identity")?;
        let contract = contracts.get(identity).ok_or("missing overload ownership")?;
        let dependency = contract.result_source.as_ref().ok_or("missing source relationship")?;
        assert_eq!(dependency.slot, crate::ownership_metadata::SourceSlot::Parameter(index));
        assert_eq!(dependency.fields.len(), usize::from(index == 0));
    }
    assert!(!checker.errors.iter().any(|error| error.message.contains("absent")));
    let checked = checked(source)?;
    assert_eq!(
        checked
            .errors
            .iter()
            .filter(|error| error.message.contains("Result source must name"))
            .count(),
        1,
        "{:?}",
        checked.errors,
    );
    Ok(())
}

/// Direct input annotations constrain values and admit only the supported body profile. Alias declarations
/// retain their broader stage refusal, and shared access never permits source mutation.
#[test]
fn explicit_inputs_have_checked_value_and_body_profiles() -> Result<(), Box<dyn std::error::Error>> {
    for (source, admitted) in [
        ("def add(value: Borrowed[int]) -> int:\n    return value + 1\n", true),
        (
            "type View = Borrowed[int]\ndef add(value: View) -> int:\n    return value + 1\n",
            false,
        ),
        (
            "model Page:\n    title: str\ndef read(page: Borrowed[Page]) -> int:\n    return len(page.title)\n",
            true,
        ),
        (
            "def increment(mut value: BorrowedMut[int]) -> None:\n    value = value + 1\n",
            true,
        ),
        ("def owned(value: Owned[int]) -> int:\n    return value + 1\n", true),
        (
            "def add(Borrowed: int, value: Borrowed[int]) -> int:\n    return value + Borrowed\n",
            true,
        ),
    ] {
        let checker = checked(source)?;
        assert_eq!(checker.errors.is_empty(), admitted, "{source}\n{:?}", checker.errors);
        assert!(
            checker
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0123")),
            "unexpected value typing failure: {source}\n{:?}",
            checker.errors
        );
    }
    let checker = checked("def overwrite(mut value: Borrowed[int]) -> None:\n    value = 2\n")?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() != Some("INCAN-T0123")),
        "shared input mutation was not refused: {:?}",
        checker.errors
    );
    Ok(())
}

/// Argument type compatibility sees the contracted value type while retaining the selected declaration's
/// ownership request. Different overload slots cannot borrow another candidate's mode or generic binders.
#[test]
fn explicit_call_arguments_use_selected_value_types() -> Result<(), Box<dyn std::error::Error>> {
    for (source, admitted) in [
        (
            "def use(value: int) -> int:\n    return read(value)\ndef read(value: Borrowed[int]) -> int:\n    return value\n",
            true,
        ),
        (
            "def use(value: int) -> int:\n    return read(value=value)\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef read(prefix: str, value: Borrowed[int]) -> int:\n    return value\n",
            true,
        ),
        (
            "def use(value: str) -> str:\n    return take(value)\ndef take[T](value: Owned[T]) -> T:\n    return value\n",
            true,
        ),
        (
            "type View = Borrowed[int]\ndef use(value: int) -> int:\n    return read(value)\ndef read(value: View) -> int:\n    return value\n",
            false,
        ),
    ] {
        let checker = checked(source)?;
        assert_eq!(checker.errors.is_empty(), admitted, "{source}\n{:?}", checker.errors);
        assert!(
            checker
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{source}\n{:?}",
            checker.errors
        );
    }
    let checker = checked(
        "def use(value: int) -> int:\n    return read(value)\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef read(value: Owned[int]) -> int:\n    return value\n",
    )?;
    assert!(checker.errors.iter().any(|error| error.message.contains("ambiguous")));
    assert!(
        checker.type_info.protocols.explicit_input_calls.is_empty(),
        "failed overload selection must discard candidate ownership facts"
    );
    Ok(())
}

/// Plain helpers participate only in a body that already owes an input proof. Failed overload candidates
/// cannot manufacture that obligation or leave a selected ordinary signature behind.
#[test]
fn ordinary_helpers_are_scoped_to_required_input_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let ordinary = checked(
        "def add(left: int, right: int) -> int:\n    return left + right\ndef use(value: int) -> int:\n    return add(right=2, left=value)\n",
    )?;
    assert!(ordinary.errors.is_empty(), "{:?}", ordinary.errors);
    assert!(ordinary.type_info.protocols.explicit_input_calls.is_empty());
    assert!(ordinary.type_info.protocols.explicit_input_flow.is_empty());
    for source in [
        "def add(left: int, right: int) -> int:\n    return left + right\ndef use(value: Borrowed[int]) -> int:\n    return add(right=2, left=value)\n",
        "def add(left: int, right: int) -> int:\n    return left + right\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef use(value: int) -> int:\n    first = add(value, 2)\n    return read(first)\n",
    ] {
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{source}\n{:?}", checker.errors);
        assert!(
            checker
                .type_info
                .protocols
                .explicit_input_calls
                .values()
                .any(|call| call.callee.declaration_name == "add")
        );
    }
    for (source, accepted) in [
        (
            "def identity[T](value: T) -> T:\n    return value\ndef use(value: Borrowed[int]) -> int:\n    return identity(value)\n",
            false,
        ),
        (
            "def phantom[T](value: int) -> int:\n    return value\ndef use(value: Borrowed[int]) -> int:\n    return phantom[int](value)\n",
            true,
        ),
        (
            "def absent[T](value: int) -> Option[T]:\n    return None\ndef use(value: Borrowed[int]) -> Option[int]:\n    return absent[int](value)\n",
            true,
        ),
    ] {
        let checker = checked(source)?;
        assert_eq!(checker.errors.is_empty(), accepted, "{source}\n{:?}", checker.errors);
        assert!(
            checker
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{source}\n{:?}",
            checker.errors
        );
    }
    let failed = checked(
        "def choose(value: int) -> int:\n    return value\ndef choose(other: int) -> int:\n    return other\ndef use(value: Borrowed[int]) -> int:\n    return choose(value)\n",
    )?;
    assert!(
        failed.errors.iter().any(|error| error.message.contains("ambiguous")),
        "{:?}",
        failed.errors
    );
    assert!(failed.type_info.protocols.explicit_input_calls.is_empty());
    Ok(())
}

/// Declaration classification is authenticated independently of restored access events. Neither replacing an
/// explicit contract nor borrowing a same-shaped ordinary header may authorize a forged invocation.
#[test]
fn ordinary_call_replay_requires_its_exact_header() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(left: int, right: int) -> int:\n    return left + right\ndef use(value: Borrowed[int]) -> int:\n    return add(right=2, left=value)\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let (span, call) = checker
        .type_info
        .protocols
        .explicit_input_calls
        .iter()
        .next()
        .ok_or("missing ordinary call")?;
    assert_eq!(call.kind, InputCallKind::Ordinary);
    let owner = call.owner.as_ref().ok_or("missing caller")?;
    assert!(checker.type_info.checked_input_body(owner)?.is_some());
    for corruption in 0..7 {
        let mut facts = checker.type_info.clone();
        let changed = facts
            .protocols
            .explicit_input_calls
            .get_mut(span)
            .ok_or("missing call")?;
        match corruption {
            0 => changed.kind = InputCallKind::Explicit,
            1 => changed.parameters[0].is_mut = true,
            2 => changed.owner = Some(call.callee.clone()),
            3 => {
                facts
                    .declarations
                    .function_bindings_by_span
                    .retain(|_, binding| binding.identity.as_ref() != Some(&call.callee));
                facts
                    .declarations
                    .function_bindings
                    .retain(|_, binding| binding.identity.as_ref() != Some(&call.callee));
            }
            4 => {
                changed.parameters[0].is_mut = true;
                for binding in facts
                    .declarations
                    .function_bindings_by_span
                    .values_mut()
                    .chain(facts.declarations.function_bindings.values_mut())
                {
                    if binding.identity.as_ref() == Some(&call.callee) {
                        binding.params[0].is_mut = true;
                    }
                }
            }
            5 => {
                let mut conflict = facts
                    .declarations
                    .function_bindings_by_span
                    .get(&call.callee.declaration_span)
                    .ok_or("missing header")?
                    .clone();
                conflict.params[0].is_mut = true;
                facts
                    .declarations
                    .function_bindings
                    .insert("other_alias".into(), conflict);
            }
            _ => {
                facts.protocols.explicit_input_calls.remove(span);
            }
        }
        assert!(
            facts.checked_input_body(owner).is_err(),
            "accepted ordinary call corruption {corruption}"
        );
    }
    let mut explicit = checked(
        "def read(value: Borrowed[int]) -> int:\n    return value\ndef use(value: int) -> int:\n    return read(value)\n",
    )?;
    let call = explicit
        .type_info
        .protocols
        .explicit_input_calls
        .values_mut()
        .next()
        .ok_or("missing explicit call")?;
    call.kind = InputCallKind::Ordinary;
    let owner = call.owner.clone().ok_or("missing caller")?;
    assert!(explicit.type_info.checked_input_body(&owner).is_err());
    Ok(())
}

/// A selected overload retains its own instantiated parameter slots without rewriting either declaration.
/// The ordinary caller owns its checked access profile even without an explicit input annotation of its own.
#[test]
fn explicit_call_facts_preserve_canonical_slots() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "def use(value: str) -> str:\n    return read(1, value)\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef read(prefix: int, value: Owned[str]) -> str:\n    return value\n",
    )?;
    assert!(
        checker
            .errors
            .iter()
            .all(|error| error.stable_code() == Some("INCAN-T0123")),
        "{:?}",
        checker.errors
    );
    assert_eq!(checker.type_info.protocols.explicit_input_calls.len(), 1);
    let call = checker
        .type_info
        .protocols
        .explicit_input_calls
        .values()
        .next()
        .ok_or("missing explicit call")?;
    assert_eq!(call.parameters.len(), 2);
    assert_eq!(call.parameters[1].ty, ResolvedType::Str);
    let contract = checker
        .type_info
        .declarations
        .ownership_contracts
        .get(&call.callee)
        .ok_or("selected contract missing")?;
    assert_eq!(contract.modes.len(), 1);
    assert_eq!(contract.modes[0].component.parameter, Some(1));
    assert_eq!(contract.modes[0].mode, OwnershipMode::Owned);
    let declaration = checker
        .type_info
        .declarations
        .function_bindings_by_span
        .get(&call.callee.declaration_span)
        .ok_or("selected header missing")?;
    assert!(
        matches!(declaration.params[1].ty, ResolvedType::Generic(_, _)),
        "publication must retain the mode"
    );
    let owner = call.owner.as_ref().ok_or("callable owner missing")?;
    assert_eq!(owner.declaration_name, "use");
    assert!(matches!(
        checker.type_info.protocols.explicit_input_flow.get(owner),
        Some(InputFlow::Checked { .. })
    ));
    Ok(())
}

/// Call-site requests apply even to read-only bodies and ordinary callers, including named argument order.
#[test]
fn explicit_calls_check_source_access_and_transfer() -> Result<(), Box<dyn std::error::Error>> {
    for (source, conflicts) in [
        (
            "def use(mut value: int) -> int:\n    set(value)\n    return value\ndef set(mut value: BorrowedMut[int]) -> None:\n    value = 2\n",
            false,
        ),
        (
            "def use(value: str) -> None:\n    read(value, value)\ndef read(first: Borrowed[str], second: Borrowed[str]) -> None:\n    pass\n",
            false,
        ),
        (
            "def use(value: str) -> str:\n    take(value)\n    return value\ndef take(value: Owned[str]) -> None:\n    pass\n",
            true,
        ),
        (
            "def use(value: int) -> int:\n    take(value)\n    return value\ndef take(value: Owned[int]) -> None:\n    pass\n",
            false,
        ),
        (
            "def use(value: int) -> None:\n    read(value)\ndef read(value: BorrowedMut[int]) -> None:\n    pass\n",
            true,
        ),
        (
            "def use(mut value: int) -> None:\n    read(value, value)\ndef read(first: BorrowedMut[int], second: BorrowedMut[int]) -> None:\n    pass\n",
            true,
        ),
        (
            "def use(value: str) -> None:\n    read(value, value)\ndef read(first: Borrowed[str], second: Owned[str]) -> None:\n    pass\n",
            true,
        ),
        (
            "def use(value: str) -> None:\n    read(second=take(value), first=value)\ndef read(first: Borrowed[str], second: int) -> None:\n    pass\ndef take(value: Owned[str]) -> int:\n    return 1\n",
            true,
        ),
        (
            "def use(value: str) -> None:\n    read(first=value, second=take(value))\ndef read(first: Borrowed[str], second: int) -> None:\n    pass\ndef take(value: Owned[str]) -> int:\n    return 1\n",
            true,
        ),
        (
            "def use(mut value: Pair) -> None:\n    read(value.left, value.right)\ndef read(first: BorrowedMut[int], second: BorrowedMut[int]) -> None:\n    pass\nmodel Pair:\n    left: int\n    right: int\n",
            false,
        ),
        (
            "def use(mut value: Pair) -> None:\n    read(value, value.left)\ndef read(first: BorrowedMut[Pair], second: Borrowed[int]) -> None:\n    pass\nmodel Pair:\n    left: int\n    right: int\n",
            true,
        ),
        (
            "def use(mut value: BorrowedMut[int]) -> int:\n    read(value)\n    value = 3\n    return value\ndef read(value: Borrowed[int]) -> None:\n    pass\n",
            false,
        ),
        (
            "def use(value: Borrowed[str]) -> None:\n    take(value)\ndef take(value: Owned[str]) -> None:\n    pass\n",
            true,
        ),
        (
            "def use(value: str) -> int:\n    return choose(second=value == \"x\", first=take(value))\ndef choose(first: Borrowed[int], second: bool) -> int:\n    return first\ndef take(value: Owned[str]) -> int:\n    return 1\n",
            true,
        ),
        (
            "def use(reader: Reader, value: str) -> int:\n    return reader.choose(second=value == \"x\", first=take(value))\ndef take(value: Owned[str]) -> int:\n    return 1\nmodel Reader:\n    marker: int\n    def choose(self, first: Borrowed[int], second: bool) -> int:\n        return first\n",
            false,
        ),
    ] {
        let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
        let analysis = flow::analyze(&graph).map_err(|error| format!("{error:?}"))?;
        assert_eq!(!analysis.conflicts.is_empty(), conflicts, "{source}\n{analysis:?}");
    }
    Ok(())
}

/// A synthetic protocol lookup at an existing expression span must not steal or invent the source call's
/// explicit ownership contract when its temporary lookup artifacts are removed.
#[test]
fn explicit_calls_follow_call_site_save_and_restore() -> Result<(), Box<dyn std::error::Error>> {
    let mut checker = checked(
        "def use(value: int) -> int:\n    return read(value)\ndef read(value: Borrowed[int]) -> int:\n    return value\n",
    )?;
    let (span, original) = checker
        .type_info
        .protocols
        .explicit_input_calls
        .iter()
        .next()
        .map(|(span, call)| (*span, call.clone()))
        .ok_or("missing call")?;
    let facts = checker.type_info.take_call_site_facts(span);
    assert!(checker.type_info.protocols.explicit_input_calls.is_empty());
    checker
        .type_info
        .protocols
        .explicit_input_calls
        .insert(span, original.clone());
    checker.type_info.clear_body_ir_call_site_facts(span);
    assert!(checker.type_info.protocols.explicit_input_calls.is_empty());
    checker.type_info.restore_call_site_facts(span, facts);
    assert_eq!(
        checker.type_info.protocols.explicit_input_calls.get(&span),
        Some(&original)
    );
    let empty_span = crate::ast::Span::new(0, 0);
    let empty = checker.type_info.take_call_site_facts(empty_span);
    checker
        .type_info
        .protocols
        .explicit_input_calls
        .insert(empty_span, original);
    checker.type_info.restore_call_site_facts(empty_span, empty);
    assert!(
        !checker
            .type_info
            .protocols
            .explicit_input_calls
            .contains_key(&empty_span)
    );
    Ok(())
}

/// Explicit method parameters retain the exact selected overload, and their receiver contributes a loan in
/// source evaluation order rather than gaining access from the method's observed implementation.
#[test]
fn explicit_method_calls_retain_receiver_access() -> Result<(), Box<dyn std::error::Error>> {
    for (source, conflict) in [
        (
            "def use(reader: Reader, value: str) -> None:\n    reader.read(value)\nclass Reader:\n    def read(self, value: Borrowed[str]) -> None:\n        pass\n",
            false,
        ),
        (
            "def use(mut reader: Reader, value: str) -> None:\n    reader.read(value)\nclass Reader:\n    def read(mut self, value: Borrowed[str]) -> None:\n        pass\n",
            false,
        ),
        (
            "def use(reader: Reader, value: str) -> None:\n    reader.read(value)\nclass Reader:\n    def read(mut self, value: Borrowed[str]) -> None:\n        pass\n",
            true,
        ),
        (
            "def use(mut reader: Reader) -> None:\n    reader.read(reader.value)\nclass Reader:\n    pub value: str\n    def read(mut self, value: Borrowed[str]) -> None:\n        pass\n",
            true,
        ),
        (
            "def use(reader: Reader, value: str) -> str:\n    reader.read(1, value)\n    return value\nclass Reader:\n    def read(self, value: Borrowed[int]) -> None:\n        pass\n    def read(self, prefix: int, value: Owned[str]) -> None:\n        pass\n",
            true,
        ),
    ] {
        let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
        let analysis = flow::analyze(&graph).map_err(|error| format!("{error:?}"))?;
        assert_eq!(!analysis.conflicts.is_empty(), conflict, "{source}\n{analysis:?}");
    }
    Ok(())
}

/// A method examined only for an argument diagnostic is not a selected ownership target.
#[test]
fn rejected_method_does_not_retain_input_access() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "def use(reader: Reader, value: str) -> None:\n    reader.read(value)\nclass Reader:\n    def read(self, value: Borrowed[int]) -> None:\n        pass\n",
    )?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() != Some("INCAN-T0123"))
    );
    assert!(
        checker.type_info.protocols.explicit_input_calls.is_empty(),
        "diagnostic fallback must not retain a rejected method's ownership contract"
    );
    Ok(())
}

/// A fresh computed argument owns bounded storage for its invocation, while parentheses around a place retain
/// that place's permission. Computing a temporary must retain transfers made by nested argument evaluation.
#[test]
fn borrowed_arguments_can_use_bounded_temporary_owners() -> Result<(), Box<dyn std::error::Error>> {
    for (source, conflict) in [
        (
            "def use() -> None:\n    read(\"hello\")\ndef read(value: Borrowed[str]) -> None:\n    pass\n",
            false,
        ),
        (
            "def use() -> None:\n    read(1 + 2)\ndef read(value: Borrowed[int]) -> None:\n    pass\n",
            false,
        ),
        (
            "def use() -> None:\n    change(1)\ndef change(mut value: BorrowedMut[int]) -> None:\n    value = 2\n",
            false,
        ),
        (
            "def use(value: int) -> None:\n    change((value))\ndef change(mut value: BorrowedMut[int]) -> None:\n    value = 2\n",
            true,
        ),
        (
            "def use(value: str) -> str:\n    read(take(value))\n    return value\ndef read(value: Borrowed[str]) -> None:\n    pass\ndef take(value: Owned[str]) -> str:\n    return value\n",
            true,
        ),
    ] {
        let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
        let analysis = flow::analyze(&graph).map_err(|error| format!("{error:?}"))?;
        assert_eq!(!analysis.conflicts.is_empty(), conflict, "{source}\n{analysis:?}");
    }
    Ok(())
}
