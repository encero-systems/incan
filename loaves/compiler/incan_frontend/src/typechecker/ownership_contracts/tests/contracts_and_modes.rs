//! Explicit loops and bodies, carried views, ownership modes, receiver contracts and field views.

use super::*;

/// Loops over explicit inputs execute under the conditional profile: a loop reads each item of a borrowed input,
/// and an owned input or local may be transferred inside a loop whose break leaves it before a second iteration.
#[test]
fn explicit_loops_execute_under_the_conditional_profile() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def total(values: Borrowed[list[int]]) -> int:\n    mut sum = 0\n    for value in values:\n        sum += value\n    return sum\n",
        "def consume(text: Owned[str]) -> None:\n    pass\ndef maybe(text: Owned[str], rounds: Borrowed[list[int]]) -> None:\n    for _round in rounds:\n        consume(text)\n        break\n",
        "def consume(value: Owned[str]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    value = \"owner\"\n    if flag:\n        while flag:\n            consume(value)\n            break\n",
    ] {
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{source}: {:?}", checker.errors);
    }
    Ok(())
}

/// `?` continues with the success payload and leaves on the error, like a `return`; `Ok`, `Err` and `Some`
/// construct a fresh carrier from an owned payload; a `match` on a fresh carrier starts every arm from the
/// evaluated scrutinee and binds its payload as a fresh owner.
#[test]
fn explicit_bodies_propagate_construct_and_match_results() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "def size(text: Owned[str]) -> Result[int, str]:\n    if len(text) == 0:\n        return Err(\"empty\")\n    return Ok(len(text))\n";
    for body in [
        "def both(text: Owned[str], other: Owned[str]) -> Result[int, str]:\n    first = size(text)?\n    second = size(other)?\n    return Ok(first + second)\n",
        "def describe(text: Owned[str]) -> str:\n    match size(text):\n        Ok(count) => return f\"{count}\"\n        Err(message) => return message\n",
        "def keep(value: Owned[str]) -> None:\n    pass\ndef route(text: Owned[str], other: Owned[str]) -> None:\n    match size(text):\n        Ok(_) => keep(other)\n        Err(_) => pass\n",
        "def maybe(text: Owned[str], keep: bool) -> Option[str]:\n    if keep:\n        return Some(f\"{text}!\")\n    return None\n",
    ] {
        let checker = checked(&format!("{declarations}{body}"))?;
        assert!(
            checker.errors.is_empty(),
            "{body}: {:?} {:?}",
            checker.errors,
            flow_summary(&checker)
        );
    }
    let refused = checked(&format!(
        "{declarations}def twice(text: Owned[str]) -> Result[int, str]:\n    first = size(text)?\n    second = size(text)?\n    return Ok(first + second)\n"
    ))?;
    assert!(
        refused
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        refused.errors
    );
    // A carrier consumes an owned payload through the same checked move as an explicit owned call argument.
    let accepted = checked("def wrap(text: Owned[str]) -> Option[str]:\n    return Some(text)\n")?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    let refused = checked("def wrap(text: Borrowed[str]) -> Option[str]:\n    return Some(text)\n")?;
    assert!(
        refused
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        refused.errors
    );
    Ok(())
}

/// RFC 164's fallible buffer: a view carried in a `Result` comes out of its carrier with `?`, chains into the next
/// call, propagates its error through every caller, and leaves the owner usable for recovery once the failed
/// operation's borrows end. A carried view must still carry its declared source place.
#[test]
fn carried_views_propagate_and_recover() -> Result<(), Box<dyn std::error::Error>> {
    let shared =
        checked("def view(value: Borrowed[str]) -> Result[Borrowed[str], str] from value:\n    return Ok(value)\n")?;
    assert!(
        shared.errors.is_empty(),
        "{:?} {:?}",
        shared.errors,
        flow_summary(&shared)
    );
    let buffer = "class SampleBuffer:\n    pub capacity: int\n    pub samples: list[int]\n\n    pub def append(mut self, sample: int) -> Result[Reference[Self], str] from self:\n        if len(self.samples) >= self.capacity:\n            return Err(\"full\")\n        self.samples.append(sample)\n        return Ok(self)\n\n    pub def append_all(mut self, samples: Borrowed[list[int]]) -> Result[Reference[Self], str] from self:\n        for sample in samples:\n            self.append(sample)?\n        return Ok(self)\n\n";
    let accepted = checked(&format!(
        "{buffer}def fill(mut buffer: BorrowedMut[SampleBuffer]) -> Result[None, str]:\n    (buffer.append(10)?).append_all([20, 30])?\n    return Ok(None)\n\ndef recover() -> Result[None, str]:\n    mut buffer = SampleBuffer(capacity=2, samples=[])\n    match fill(buffer):\n        Ok(_) => println(\"All samples added\")\n        Err(error) =>\n            println(error)\n            assert len(buffer.samples) == 2\n            buffer.capacity = 3\n            buffer.append(30)?\n    assert len(buffer.samples) == 3\n    return Ok(None)\n"
    ))?;
    assert!(
        accepted.errors.is_empty(),
        "{:?} {:?}",
        accepted.errors,
        flow_summary(&accepted)
    );
    // Every call and `?` sees the projected value type, in the checked expression types and in every call-result
    // record Body IR reads: no type still carries the written mode.
    let leaked = accepted
        .type_info
        .expressions
        .expr_types
        .iter()
        .chain(&accepted.type_info.expressions.body_ir_expr_types)
        .chain(&accepted.type_info.calls.body_ir_call_result_types)
        .filter(|(_, ty)| format!("{ty:?}").contains("Reference"))
        .map(|(span, ty)| format!("{span:?}: {ty:?}"))
        .collect::<Vec<_>>();
    assert!(leaked.is_empty(), "{leaked:?}");
    let refused = checked(&format!(
        "{buffer}def other(mut buffer: BorrowedMut[SampleBuffer], mut spare: BorrowedMut[SampleBuffer]) -> Result[Reference[SampleBuffer], str] from buffer:\n    return Ok(spare)\n"
    ))?;
    assert!(
        refused
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0124")),
        "{:?}",
        refused.errors
    );
    Ok(())
}

/// A builtin list method changes or reads its receiver place in place: an exclusive receiver's field may grow,
/// while a shared input cannot be changed through its borrow.
#[test]
fn explicit_bodies_call_builtin_list_methods_on_places() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "class SampleBuffer:\n    pub capacity: int\n    pub samples: list[int]\n\n    pub def add(mut self, sample: Borrowed[int]) -> bool:\n        self.samples.append(sample)\n        return self.samples.contains(sample)\n\ndef main() -> None:\n    mut buffer = SampleBuffer(capacity=2, samples=[])\n    buffer.add(10)\n    println(len(buffer.samples))\n",
    )?;
    assert!(
        accepted.errors.is_empty(),
        "{:?} {:?}",
        accepted.errors,
        flow_summary(&accepted)
    );
    let refused = checked("def grow(values: Borrowed[list[int]]) -> None:\n    values.append(1)\n")?;
    assert!(
        refused
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?} {:?}",
        refused.errors,
        flow_summary(&refused)
    );
    Ok(())
}

/// A plain helper without arguments is an ordinary helper call like one with Copy arguments: a checked body may
/// read its scalar result.
#[test]
fn checked_bodies_call_plain_helpers_without_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "def seven() -> int:\n    return 7\ndef take(text: Owned[str]) -> None:\n    pass\ndef main() -> None:\n    before = seven()\n    take(\"x\")\n    println(before)\n",
    )?;
    assert!(
        checker.errors.is_empty(),
        "{:?} {:?}",
        checker.errors,
        flow_summary(&checker)
    );
    Ok(())
}

/// Printing a list of primitives reaches no nominal display hook, so a checked caller may print the list an
/// exclusive input replaced. A list of models may display through derived or user hooks and stays refused.
#[test]
fn checked_callers_print_lists_of_primitives() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "def collapse(mut values: BorrowedMut[list[int]]) -> None:\n    values = [9]\ndef main() -> None:\n    mut values = [1, 2, 3]\n    collapse(values)\n    println(values)\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    let refused = checked(
        "class Tag:\n    pub name: str\ndef keep(mut tags: BorrowedMut[list[Tag]]) -> None:\n    pass\ndef main() -> None:\n    mut tags = [Tag(name=\"a\")]\n    keep(tags)\n    println(tags)\n",
    )?;
    assert!(
        refused
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0123")),
        "{:?}",
        refused.errors
    );
    Ok(())
}

/// A transfer inside a loop that can reach another iteration uses an owner the first iteration already moved:
/// the back edge from the body's end, and from a `continue`, carries the transfer to the next one.
#[test]
fn explicit_loops_refuse_a_transfer_another_iteration_repeats() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "def consume(text: Owned[str]) -> None:\n    pass\n";
    for body in [
        "def again(text: Owned[str], rounds: Borrowed[list[int]]) -> None:\n    for _round in rounds:\n        consume(text)\n",
        "def again(text: Owned[str], flag: bool) -> None:\n    while flag:\n        consume(text)\n",
        "def again(text: Owned[str], flag: bool) -> None:\n    while flag:\n        consume(text)\n        if flag:\n            continue\n        break\n",
    ] {
        let refused = checked(&format!("{declarations}{body}"))?;
        assert!(
            refused
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{body}: {:?}",
            refused.errors
        );
    }
    Ok(())
}

/// Reannotation remains outside admission. A method body's alias, an alias returned as the declared view source
/// and a conditional owned local (under the conditional profile) are admitted.
#[test]
fn local_alias_admission_preserves_other_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "class Counter:\n    value: int\n    pub def read(self) -> int:\n        let local: Borrowed[int] = self.value\n        return local\n",
        "def read(value: Borrowed[str]) -> Borrowed[str] from value:\n    let local: Borrowed[str] = value\n    return local\n",
        "def read(value: Owned[int], choose: bool) -> int:\n    if choose:\n        let local: Owned[int] = value\n        return local\n    return value\n",
    ] {
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{source}: {:?}", checker.errors);
    }
    for source in ["def wrong() -> int:\n    mut value = 1\n    value: BorrowedMut[int] = 2\n    return value\n"] {
        let checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "{source}: {:?}",
            checker.errors
        );
    }
    Ok(())
}

/// A source declaration shadowing a prelude spelling remains an ordinary nominal type.
#[test]
fn shadowed_mode_is_not_an_ownership_contract() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "model Borrowed[T]:\n    value: T\ndef identity(value: Borrowed[int]) -> Borrowed[int]:\n    return value\n",
    )?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

/// Every explicit form is refused before executable storage can be selected, also when nested.
#[test]
fn modes_are_refused_at_every_annotation_depth() -> Result<(), Box<dyn std::error::Error>> {
    for mode in ["Borrowed", "BorrowedMut", "Owned", "Reference"] {
        let checker = checked(&format!("def read(value: Option[{mode}[str]]) -> None:\n    pass\n"))?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "{mode}: {:?}",
            checker.errors
        );
    }
    Ok(())
}

/// Even a single-source signature needs the explicit relationship, and globals cannot fulfill it.
#[test]
fn borrowed_result_source_is_mandatory_and_local_to_signature() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def view(value: Borrowed[str]) -> Borrowed[str]:\n    return value\n",
        "def view(value: Borrowed[str]) -> Result[Borrowed[str], str]:\n    return Ok(value)\n",
        "def view(value: Borrowed[str]) -> FrozenList[Borrowed[str]]:\n    return []\n",
        "const other: str = \"outside\"\ndef view(value: str) -> str from other:\n    return value\n",
    ] {
        let checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0124")),
            "{:?}",
            checker.errors
        );
    }
    Ok(())
}

/// A valid written source is still refused rather than authorizing old copied-result behavior.
#[test]
fn source_relationship_does_not_enable_execution() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked("def view(value: str) -> str from value:\n    return value\n")?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0123")),
        "{:?}",
        checker.errors
    );
    Ok(())
}

/// A mutable local that copies a stored string warns without rejecting or changing the program.
#[test]
fn mutable_stored_string_copy_warns() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "model Page:\n    title: str\ndef main() -> None:\n    page = Page(title=\"old\")\n    mut title = page.title\n    title = \"new\"\n",
    )?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert!(
        checker
            .warnings
            .iter()
            .any(|warning| warning.message.contains("copies a place that a place binding borrows")),
        "{:?}",
        checker.warnings
    );
    Ok(())
}

/// Compatible annotations, scalar fields and read-only locals do not accidentally request independent copies.
#[test]
fn place_migration_includes_annotations_scalars_and_readonly() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "model Page:\n    count: int\ndef main() -> None:\n    page = Page(count=1)\n    mut count: int = page.count\n    let saved = page.count\n    copy = int(2)\n    fresh = Page(count=2)\n",
    )?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let warnings: Vec<_> = checker
        .warnings
        .iter()
        .filter(|warning| warning.stable_code() == Some("INCAN-T0125"))
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    Ok(())
}

/// Abstract and concrete receiver sources share checked slots; concrete field paths retain their declaration.
/// Both returned views are executable.
#[test]
fn receiver_contracts_cover_abstract_and_concrete_methods() -> Result<(), Box<dyn std::error::Error>> {
    for (source, fields) in [
        ("pub trait Reader:\n    def view(self) -> Borrowed[Self] from self\n", 0),
        (
            "pub class Page:\n    title: str\n    pub def view(self) -> Borrowed[str] from self.title:\n        return self.title\n",
            1,
        ),
    ] {
        let checker = checked(source)?;
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
        let methods = match &exports.first().ok_or("missing public owner")?.kind {
            crate::library_exports::CheckedExportKind::Class(owner) => &owner.methods,
            crate::library_exports::CheckedExportKind::Trait(owner) => &owner.methods,
            _ => return Err("wrong owner kind".into()),
        };
        assert!(methods.first().ok_or("missing public method")?.ownership.is_some());
        let contract = checker
            .type_info
            .declarations
            .ownership_contracts
            .values()
            .next()
            .ok_or("missing method contract")?;
        let dependency = contract.result_source.as_ref().ok_or("missing receiver source")?;
        assert_eq!(dependency.slot, crate::ownership_metadata::SourceSlot::Receiver);
        assert_eq!(dependency.fields.len(), fields);
        assert!(checker.errors.is_empty(), "{source}: {:?}", checker.errors);
    }
    Ok(())
}

/// Annotation constructors and aliases cannot silently opt into ordinary generic value behavior.
#[test]
fn modes_are_not_wrappers_or_alias_escape_hatches() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def main() -> None:\n    value = Owned(1)\n",
        "type View = Borrowed[str]\ndef view(value: View) -> None:\n    pass\n",
        "def read(value: Owned[int, str]) -> None:\n    pass\n",
    ] {
        let checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "{:?}",
            checker.errors
        );
    }
    Ok(())
}

/// Source relationships use slots and canonical fields, including substituted nested generic owners.
#[test]
fn checked_source_retains_nested_field_identities() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "model Address:\n    street: str\nmodel Customer[T]:\n    address: T\ndef view(customer: Borrowed[Customer[Address]]) -> Borrowed[str] from customer.address.street:\n    return customer.address.street\n",
    )?;
    let contract = checker
        .type_info
        .declarations
        .ownership_contracts
        .values()
        .next()
        .ok_or("missing ownership contract")?;
    let source = contract.result_source.as_ref().ok_or("missing checked source")?;
    assert_eq!(source.slot, crate::ownership_metadata::SourceSlot::Parameter(0));
    assert_eq!(
        source
            .fields
            .iter()
            .map(|field| field.declaration_name.as_str())
            .collect::<Vec<_>>(),
        ["address", "street"]
    );
    assert!(
        source
            .fields
            .iter()
            .all(|field| field.kind == incan_semantics_core::SemanticSourceTargetKind::Field)
    );
    assert_eq!(contract.modes.len(), 2);
    Ok(())
}

/// An alias-expanded borrowed result still requires a source, and impossible fields have source diagnostics.
#[test]
fn checked_alias_and_invalid_field_contracts() -> Result<(), Box<dyn std::error::Error>> {
    let missing = checked("type View[T] = Borrowed[T]\ndef view(value: View[str]) -> View[str]:\n    return value\n")?;
    assert!(
        missing
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0124"))
    );
    let invalid = checked(
        "model Page:\n    title: str\ndef view(page: Borrowed[Page]) -> Borrowed[str] from page.absent:\n    return \"\"\n",
    )?;
    assert!(
        invalid
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0124"))
    );
    assert!(invalid.type_info.declarations.ownership_contracts.is_empty());
    Ok(())
}

/// A method frame is checked like a function, and its caller's receiver loan conflicts with an overlapping
/// explicit argument while a disjoint field argument stays accepted.
#[test]
fn method_frames_check_receiver_and_argument_overlap() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "class Counter:\n    pub value: int\n    pub step: int\n\n    pub def bump(mut self, mut amount: BorrowedMut[int]) -> None:\n        self.value = self.value + amount\n";
    let accepted = checked(&format!(
        "{declarations}\ndef main() -> int:\n    mut counter = Counter(value=1, step=2)\n    mut step = 2\n    counter.bump(step)\n    return counter.value\n"
    ))?;
    assert!(
        accepted.errors.is_empty(),
        "{:?} {:?}",
        accepted.errors,
        accepted
            .type_info
            .protocols
            .explicit_input_flow
            .values()
            .filter_map(|flow| match flow {
                InputFlow::Unavailable(reason) => Some(reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
    let overlapping = checked(&format!(
        "{declarations}\ndef main() -> int:\n    mut counter = Counter(value=1, step=2)\n    counter.bump(counter.step)\n    return counter.value\n"
    ))?;
    assert!(
        overlapping
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        overlapping.errors
    );
    Ok(())
}

/// The RFC's returned field views: a method may hold the whole receiver while running, and its result retains
/// only the declared field, so a disjoint sibling edit is accepted while overlapping edits, owner replacement,
/// wrong sources, fresh values and manufactured exclusive access are refused.
#[test]
fn returned_field_views_retain_only_their_declared_source() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "class Address:\n    pub city: str\n\n    pub def set_city(mut self, city: str) -> Reference[Self] from self:\n        self.city = city\n        return self\n\nclass Customer:\n    pub name: str\n    pub address: Address\n\n    pub def edit_address(mut self) -> Reference[Address] from self.address:\n        return self.address\n";
    let accepted = checked(&format!(
        "{declarations}\ndef main() -> None:\n    mut customer = Customer(name=\"Sam\", address=Address(city=\"Utrecht\"))\n    mut address = customer.edit_address()\n    customer.name = \"Samuel\"\n    address.set_city(\"Rotterdam\")\n    assert customer.address.city == \"Rotterdam\"\n"
    ))?;
    assert!(
        accepted.errors.is_empty(),
        "{:?} {:?}",
        accepted.errors,
        flow_summary(&accepted)
    );
    for (body, code) in [
        (
            "    mut address = customer.edit_address()\n    customer.address.city = \"Amsterdam\"\n    address.set_city(\"Rotterdam\")\n",
            "INCAN-T0126",
        ),
        (
            "    mut address = customer.edit_address()\n    customer = Customer(name=\"Alex\", address=Address(city=\"Amsterdam\"))\n    address.set_city(\"Rotterdam\")\n",
            "INCAN-T0126",
        ),
    ] {
        let refused = checked(&format!(
            "{declarations}\ndef main() -> None:\n    mut customer = Customer(name=\"Sam\", address=Address(city=\"Utrecht\"))\n{body}"
        ))?;
        assert!(
            refused.errors.iter().any(|error| error.stable_code() == Some(code)),
            "{body}: {:?} {:?}",
            refused.errors,
            flow_summary(&refused)
        );
    }
    for source in [
        "class Address:\n    pub city: str\n    pub street: str\n    pub def view(mut self) -> Reference[str] from self.city:\n        return self.street\n",
        "class Address:\n    pub city: str\n    pub def view(mut self) -> Reference[Address] from self:\n        return Address(city=\"Utrecht\")\n",
        "class Address:\n    pub city: str\n    pub def view(self) -> Reference[Address] from self:\n        return self\n",
        "class Address:\n    pub city: str\ndef view(address: Owned[Address]) -> Borrowed[Address] from address:\n    return address\n",
    ] {
        let refused = checked(source)?;
        assert!(
            refused
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0124")),
            "{source}: {:?}",
            refused.errors
        );
    }
    Ok(())
}
