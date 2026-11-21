//! RFC 164 (Modes and inference): `Borrowed[T]` and `&T` are one type with one check, and so are `BorrowedMut[T]` and
//! `&mut T`. A top-level borrow on a parameter or a local binding fixes how the value is passed; the body follows that
//! mode's binding rules with no whole-body source-flow proof. Each shape here is written once with `{borrow:T}` and
//! `{borrow_mut:T}` markers and checked in both spellings, padded to the same width so every span agrees: the
//! diagnostics of the two spellings are identical, refusals included.

use super::*;

/// The native spellings of a shared and an exclusive borrow, padded to the width of the explicit ones.
const NATIVE: (&str, &str) = ("&         {}", "&mut         {}");
/// The explicit spellings of a shared and an exclusive borrow.
const EXPLICIT: (&str, &str) = ("Borrowed[{}]", "BorrowedMut[{}]");

/// Expand `{borrow:T}` and `{borrow_mut:T}` markers with one pair of spellings; other braces pass through.
fn spelled(template: &str, (shared, exclusive): (&str, &str)) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let marker = &rest[start + 1..start + end];
        match marker.split_once(':') {
            Some(("borrow", name)) => out.push_str(&shared.replace("{}", name)),
            Some(("borrow_mut", name)) => out.push_str(&exclusive.replace("{}", name)),
            _ => out.push_str(&rest[start..=start + end]),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// Every error and warning of one checked module, in full and sorted.
fn diagnostics(checker: &TypeChecker) -> Vec<String> {
    let mut all: Vec<String> = checker
        .errors
        .iter()
        .chain(&checker.warnings)
        .map(|diagnostic| format!("{diagnostic:?}"))
        .collect();
    all.sort();
    all
}

/// Check `template` in both spellings and require identical diagnostics; returns the native spelling's checker.
fn check_twins(template: &str) -> Result<TypeChecker, Box<dyn std::error::Error>> {
    let native = checked(&spelled(template, NATIVE))?;
    let explicit = checked(&spelled(template, EXPLICIT))?;
    assert_eq!(diagnostics(&explicit), diagnostics(&native), "{template}");
    Ok(explicit)
}

/// Error messages of one checker, for assertions on what a shape refuses.
fn messages(checker: &TypeChecker) -> Vec<String> {
    checker.errors.iter().map(|error| error.message.clone()).collect()
}

const ITEM: &str =
    "model Item:\n    name: str\n    count: int\n\n    def label(self) -> str:\n        return self.name\n\n";
const COUNTER: &str = "model Counter:\n    value: int\n\n    def bump(mut self) -> None:\n        self.value += 1\n\n    def read(self) -> int:\n        return self.value\n\n";

/// The platform trace writer's shapes: a borrowed text handed on through a call whose result the body reads, and a
/// borrowed record read by a method. Accepted in both spellings, with no ownership contract and no source-flow proof.
#[test]
fn trace_writer_shapes_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let template = "model TraceRecord:\n    name: str\n    size: int\n\ndef echo(text: str) -> str:\n    return text\n\ndef measure(text: {borrow:str}) -> int:\n    return len(echo(text))\n\nclass Writer:\n    pub rows: list[str]\n\n    pub def append(mut self, text: {borrow:str}) -> int:\n        self.rows.append(echo(text))\n        return len(self.rows)\n\n    pub def write_record(mut self, row: {borrow:TraceRecord}) -> int:\n        self.rows.append(echo(row.name))\n        return row.size + len(self.rows)\n\n    pub def record(mut self, row: {borrow:TraceRecord}) -> int:\n        return self.write_record(row)\n\ndef main() -> int:\n    mut writer = Writer(rows=[])\n    row = TraceRecord(name=\"a\", size=1)\n    return writer.append(\"x\") + writer.write_record(row) + writer.record(row) + measure(\"y\")\n";
    let checker = check_twins(template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let contracts: Vec<_> = checker
        .type_info
        .declarations
        .ownership_contracts
        .keys()
        .map(|identity| identity.declaration_name.clone())
        .collect();
    assert!(contracts.is_empty(), "{contracts:?}");
    assert!(
        checker.type_info.protocols.explicit_input_flow.is_empty(),
        "{:?}",
        flow_summary(&checker)
    );
    Ok(())
}

/// Reads through a shared borrow: fields, methods, measuring, indexing, iteration, membership and comparison.
#[test]
fn shared_reads_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let template = format!(
        "{ITEM}def reads(item: {{borrow:Item}}, items: {{borrow:list[Item]}}, names: {{borrow:dict[str, int]}}, text: {{borrow:str}}) -> int:\n    mut total = item.count + len(item.label()) + len(items) + len(text)\n    for entry in items:\n        total += entry.count\n    if items[0].name == item.name and text == \"a\":\n        total += 1\n    if \"a\" in names:\n        total += names[\"a\"]\n    return total\n\ndef main() -> int:\n    item = Item(name=\"a\", count=1)\n    return reads(item, [item], {{\"a\": 1}}, \"a\")\n"
    );
    let checker = check_twins(&template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

/// Owned uses of a shared borrow: returned as the value, handed to an owned parameter, stored in a list and in a
/// field. Each gets a clone; none is refused.
#[test]
fn owned_uses_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let template = format!(
        "{ITEM}def keep(item: Item) -> list[Item]:\n    return [item]\n\ndef give(item: {{borrow:Item}}) -> Item:\n    return item\n\ndef hand_on(item: {{borrow:Item}}) -> list[Item]:\n    return keep(item)\n\ndef store(item: {{borrow:Item}}) -> list[Item]:\n    mut out: list[Item] = []\n    out.append(item)\n    return out\n\nclass Shelf:\n    items: list[Item]\n\n    pub def put(mut self, item: {{borrow:Item}}) -> None:\n        self.items.append(item)\n"
    );
    let checker = check_twins(&template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

/// An exclusive borrow writes a field, calls a changing method and is handed on to another exclusive borrow; an
/// owned `mut` local is lent to it with no `&`.
#[test]
fn exclusive_writes_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let template = format!(
        "{COUNTER}def bump_twice(mut counter: {{borrow_mut:Counter}}) -> int:\n    counter.value = counter.value + 1\n    counter.bump()\n    return counter.read()\n\ndef forward(mut counter: {{borrow_mut:Counter}}) -> int:\n    return bump_twice(counter)\n\ndef peek(counter: {{borrow:Counter}}) -> int:\n    return counter.value\n\ndef lend(mut counter: {{borrow_mut:Counter}}) -> int:\n    return peek(counter)\n\ndef main() -> int:\n    mut counter = Counter(value=0)\n    forward(counter)\n    return lend(counter) + counter.value\n"
    );
    let checker = check_twins(&template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

/// A shared borrow cannot be written through, with or without `mut`: not a field, not a changing method, and it
/// cannot be handed to an exclusive borrow.
#[test]
fn shared_writes_are_refused_in_both_spellings() -> Result<(), Box<dyn std::error::Error>> {
    for body in [
        "def rename(counter: {borrow:Counter}) -> None:\n    counter.value = 1\n",
        "def rename(mut counter: {borrow:Counter}) -> None:\n    counter.value = 1\n",
        "def rename(counter: {borrow:Counter}) -> None:\n    counter.bump()\n",
        "def rename(mut counter: {borrow:Counter}) -> None:\n    counter.bump()\n",
        "def change(mut counter: {borrow_mut:Counter}) -> None:\n    counter.value = 1\n\ndef rename(counter: {borrow:Counter}) -> None:\n    change(counter)\n",
        "def rename(items: {borrow:list[int]}) -> None:\n    items.append(1)\n",
        "def rename(mut items: {borrow:list[int]}) -> None:\n    items.append(1)\n",
    ] {
        let checker = check_twins(&format!("{COUNTER}{body}"))?;
        assert!(!checker.errors.is_empty(), "{body}: accepted");
    }
    Ok(())
}

/// An exclusive borrow without `mut` is read-only in its body, as every parameter without `mut` is.
#[test]
fn exclusive_borrow_without_mut_is_refused_a_write_in_both_spellings() -> Result<(), Box<dyn std::error::Error>> {
    let checker = check_twins(&format!(
        "{COUNTER}def rename(counter: {{borrow_mut:Counter}}) -> None:\n    counter.value = 1\n"
    ))?;
    assert!(
        messages(&checker)
            .iter()
            .any(|message| message.contains("Cannot mutate")),
        "{:?}",
        checker.errors
    );
    Ok(())
}

/// A local binding annotated with a borrow reads its value as the parameter does, and refuses a write through it.
#[test]
fn local_borrows_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = check_twins(&format!(
        "{ITEM}def first(items: list[Item]) -> int:\n    head: {{borrow:Item}} = items[0]\n    return head.count + len(head.label())\n"
    ))?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    let refused = check_twins(&format!(
        "{ITEM}def first(items: list[Item]) -> None:\n    head: {{borrow:Item}} = items[0]\n    head.count = 2\n"
    ))?;
    assert!(
        !refused.errors.is_empty(),
        "a write through a local borrow was accepted"
    );
    Ok(())
}

/// A trait method declared with a borrow is implemented with the same borrow in either spelling, and called through
/// the trait and through the implementation.
#[test]
fn trait_methods_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let template = format!(
        "{ITEM}trait Sizer:\n    def size(self, item: {{borrow:Item}}) -> int: ...\n\nclass Measure with Sizer:\n    def size(self, item: {{borrow:Item}}) -> int:\n        return item.count\n\ndef through[S with Sizer](sizer: S, item: {{borrow:Item}}) -> int:\n    return sizer.size(item)\n\ndef main() -> int:\n    item = Item(name=\"a\", count=1)\n    measure = Measure()\n    return measure.size(item) + through(measure, item)\n"
    );
    let checker = check_twins(&template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    for (trait_spelling, implementation_spelling) in [(EXPLICIT, NATIVE), (NATIVE, EXPLICIT)] {
        let trait_part = spelled(
            "trait Sizer:\n    def size(self, item: {borrow:Item}) -> int: ...\n\n",
            trait_spelling,
        );
        let implementation = spelled(
            "class Measure with Sizer:\n    def size(self, item: {borrow:Item}) -> int:\n        return item.count\n",
            implementation_spelling,
        );
        let mixed = checked(&format!("{ITEM}{trait_part}{implementation}"))?;
        assert!(mixed.errors.is_empty(), "mixed spellings: {:?}", mixed.errors);
    }
    Ok(())
}

/// A returned view borrows from a shared borrow named by its `from` clause in either spelling of the input; an owned
/// input still cannot back one.
#[test]
fn returned_views_from_borrows_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = check_twins(&format!(
        "{ITEM}def table(items: {{borrow:list[Item]}}) -> Borrowed[list[Item]] from items:\n    return items\n\ndef main() -> int:\n    items = [Item(name=\"a\", count=1)]\n    view = table(items)\n    return len(view)\n"
    ))?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    let refused = checked(&format!(
        "{ITEM}def table(items: Owned[list[Item]]) -> Borrowed[list[Item]] from items:\n    return items\n"
    ))?;
    assert!(!refused.errors.is_empty(), "an owned input backed a returned view");
    Ok(())
}

/// Slice 06's chained-call relation with slice 09B's lifetime elision: a borrowed result `from` its only borrowed
/// input, reached from that input rather than being it, keeps native passing in both spellings. Its result is the
/// native reference, it has no ownership contract, and callers read through it.
#[test]
fn borrowed_results_reached_from_their_source_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>>
{
    let template = format!(
        "{ITEM}def first(values: {{borrow:list[Item]}}, position: int) -> {{borrow:Item}} from values:\n    item: {{borrow:Item}} = values[position]\n    return item\n\ndef first_label(values: {{borrow:list[Item]}}) -> str:\n    return first(values, 0).label()\n\ndef main() -> int:\n    items = [Item(name=\"a\", count=1)]\n    chosen = first(items, 0)\n    return len(first_label(items)) + chosen.count\n"
    );
    let checker = check_twins(&template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let contracts: Vec<_> = checker
        .type_info
        .declarations
        .ownership_contracts
        .keys()
        .map(|identity| identity.declaration_name.clone())
        .collect();
    assert!(contracts.is_empty(), "{contracts:?}");
    let Some(crate::symbols::SymbolKind::Function(first)) = checker.lookup_symbol("first").map(|symbol| &symbol.kind)
    else {
        return Err("missing function first".into());
    };
    assert!(
        matches!(first.return_type, ResolvedType::Ref(_)),
        "{:?}",
        first.return_type
    );
    Ok(())
}

/// The `from` clause of such a result names the input Rust's lifetime elision ties it to: the receiver, or else the
/// only borrowed input. Naming another input, or a signature with two borrowed inputs, is refused at the clause with
/// INCAN-T0124 in both spellings, with a hint naming the input elision chose where there is one.
#[test]
fn borrowed_results_name_the_input_elision_ties_them_to() -> Result<(), Box<dyn std::error::Error>> {
    for (body, hint) in [
        (
            "def pick(a: {borrow:list[Item]}, b: {borrow:list[Item]}) -> {borrow:Item} from b:\n    item: {borrow:Item} = b[0]\n    return item\n",
            None,
        ),
        (
            "class Shelf:\n    pub items: list[Item]\n\n    pub def first(self, other: {borrow:list[Item]}) -> {borrow:Item} from other:\n        item: {borrow:Item} = other[0]\n        return item\n",
            Some("from self"),
        ),
    ] {
        let checker = check_twins(&format!("{ITEM}{body}"))?;
        let refusal = checker
            .errors
            .iter()
            .find(|error| error.stable_code() == Some("INCAN-T0124"))
            .ok_or_else(|| format!("{body}: {:?}", checker.errors))?;
        if let Some(hint) = hint {
            assert!(format!("{refusal:?}").contains(hint), "{refusal:?}");
        }
    }
    Ok(())
}

/// Such a result returns a borrow reached from its source; a value created in the body (a temporary list's element
/// field, a fresh local) is refused with one INCAN-T0124 at the returned value, in both spellings.
#[test]
fn borrowed_results_cannot_return_values_created_in_the_body() -> Result<(), Box<dyn std::error::Error>> {
    for body in [
        "def invalid(source: {borrow:list[Item]}) -> {borrow:str} from source:\n    temporary = [Item(name=\"temporary\", count=1)]\n    return temporary[0].name\n",
        "def invalid(source: {borrow:list[Item]}) -> {borrow:Item} from source:\n    fresh = Item(name=\"fresh\", count=1)\n    return fresh\n",
    ] {
        let checker = check_twins(&format!("{ITEM}{body}"))?;
        let codes: Vec<_> = checker.errors.iter().map(|error| error.stable_code()).collect();
        assert_eq!(codes, [Some("INCAN-T0124")], "{body}: {:?}", checker.errors);
    }
    Ok(())
}

/// A borrow-only signature takes the native route even when its borrowed result is exactly its source: no ownership
/// contract and no whole-body proof for it or for its caller. This is the guarded native callback's shape: a caller
/// matches on a lookup keyed through the returned borrow.
#[test]
fn exact_borrowed_results_of_borrow_only_signatures_take_the_native_route() -> Result<(), Box<dyn std::error::Error>> {
    let template = "model Key:\n    name: str\n\ndef borrowed_key(value: {borrow:Key}) -> {borrow:Key} from value:\n    return value\n\ndef lookup(entries: dict[str, int]) -> int:\n    key = Key(name=\"key\")\n    match entries.get(borrowed_key(key).name):\n        Some(value) =>\n            println(value)\n            return value\n        None => return 0\n";
    let checker = check_twins(template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    assert!(
        checker.type_info.declarations.ownership_contracts.is_empty(),
        "{:?}",
        checker.type_info.declarations.ownership_contracts.keys()
    );
    assert!(
        checker.type_info.protocols.explicit_input_flow.is_empty(),
        "{:?}",
        checker.type_info.protocols.explicit_input_flow.keys()
    );
    Ok(())
}

/// A caller holds such a result as a loan of the argument it borrows from, alive while the result is used: an edit of
/// that argument while the result is still read is refused with INCAN-T0126 in both spellings, and the same edit after
/// the last read is accepted.
#[test]
fn held_borrowed_results_are_loans_of_their_source() -> Result<(), Box<dyn std::error::Error>> {
    let table = "def table(items: {borrow:list[int]}) -> {borrow:list[int]} from items:\n    return items\n\n";
    let refused = check_twins(&format!(
        "{table}def edit(start: int) -> int:\n    mut items = [start]\n    view = table(items)\n    items.append(2)\n    return len(view)\n"
    ))?;
    assert!(
        refused
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        refused.errors
    );
    let accepted = check_twins(&format!(
        "{table}def edit(start: int) -> int:\n    mut items = [start]\n    view = table(items)\n    size = len(view)\n    items.append(2)\n    return size + len(items)\n"
    ))?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    Ok(())
}

/// A function with borrowed parameters is one callable type in both spellings, and an imported function with
/// borrowed parameters takes owned locals on both dependency routes.
#[test]
fn callable_and_imported_borrows_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let template = "def compare(left: {borrow:float}, right: {borrow:float}) -> int:\n    return 0\n\ndef apply(order: (&float, &float) -> int) -> int:\n    return 0\n\ndef main() -> int:\n    return apply(compare)\n";
    let checker = check_twins(template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let provider = "pub model Entry:\n    pub count: int\n\npub def total(values: {borrow:list[Entry]}) -> int:\n    return len(values)\n\npub def bump(mut counter: {borrow_mut:Entry}) -> None:\n    counter.count += 1\n";
    let consumer = "from views import Entry, bump, total\n\ndef main() -> int:\n    mut entry = Entry(count=1)\n    bump(entry)\n    return total([entry]) + entry.count\n";
    let native = super::one_type::consumer_checkers(&spelled(provider, NATIVE), consumer)?;
    let explicit = super::one_type::consumer_checkers(&spelled(provider, EXPLICIT), consumer)?;
    for ((route, native), (_, explicit)) in native.iter().zip(&explicit) {
        assert_eq!(diagnostics(explicit), diagnostics(native), "{route}");
        assert!(explicit.errors.is_empty(), "{route}: {:?}", explicit.errors);
    }
    Ok(())
}

/// A borrow nested in a callable type, in a Rust type's argument or inside a parameter or binding annotation is the
/// native reference in both spellings: a callback over a borrowed item, given a function and a closure; a Rust
/// generic over exclusive borrows, as a parameter and as a result; and a local list of borrows.
#[test]
fn nested_borrows_check_as_their_reference_twins() -> Result<(), Box<dyn std::error::Error>> {
    let callback = format!(
        "{ITEM}def apply(select: ({{borrow:Item}}, int) -> int, item: Item) -> int:\n    return select(item, 1)\n\ndef pick(item: {{borrow:Item}}, step: int) -> int:\n    return item.count + step\n\ndef main() -> int:\n    item = Item(name=\"a\", count=1)\n    return apply(pick, item) + apply((it, step) => len(it.label()) + step, item)\n"
    );
    let checker = check_twins(&callback)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let rust = "from rust::std::vec import Vec as Handle\n\ndef keep(mut items: Handle[tuple[{borrow_mut:int}, {borrow_mut:int}]]) -> None:\n    pass\n\ndef escaped() -> Handle[{borrow_mut:int}]:\n    return Handle.new()\n\ndef gather(value: {borrow:int}) -> int:\n    values: list[{borrow:int}] = [value]\n    return len(values)\n";
    let checker = check_twins(rust)?;
    assert!(
        !checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0123")),
        "{:?}",
        checker.errors
    );
    Ok(())
}

/// A borrow's loan conflicts are refused with INCAN-T0126 outside the whole-body proof too, in both spellings: a
/// call that reads a place while another operand lends it exclusively, the same through a local exclusive borrow, and
/// a closure holding a local borrow while its owner is edited.
#[test]
fn loan_conflicts_outside_the_proof_are_refused_in_both_spellings() -> Result<(), Box<dyn std::error::Error>> {
    let replace =
        "def replace(mut value: {borrow_mut:str}) -> int:\n    value = \"changed\"\n    return len(value)\n\n";
    for body in [
        "def invalid(mut value: {borrow_mut:str}) -> None:\n    println(value, replace(value))\n",
        "def invalid(mut value: {borrow_mut:str}) -> None:\n    mut view: {borrow_mut:str} = value\n    println(view, replace(view))\n",
        "def conflict(start: int) -> int:\n    mut owned = start + 1\n    view: {borrow:int} = owned\n    read: () -> int = () => view + 1\n    owned += 10\n    return read()\n",
        "class Counter:\n    pub value: int\n    pub name: str\n\ndef overlap(mut counter: {borrow_mut:Counter}) -> int:\n    name: {borrow:str} = counter.name\n    read: () -> int = () => len(name)\n    counter.name = \"changed\"\n    return read()\n",
    ] {
        let checker = check_twins(&format!("{replace}{body}"))?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{body}: {:?}",
            checker.errors
        );
    }
    Ok(())
}

/// Lends that do not overlap stay accepted outside the proof: one exclusive lend after another, a read after the
/// lend has ended, and a closure over a local borrow whose owner is edited only after the closure's last call.
#[test]
fn separate_loans_outside_the_proof_are_accepted() -> Result<(), Box<dyn std::error::Error>> {
    let template = format!(
        "{COUNTER}def bump(mut counter: {{borrow_mut:Counter}}) -> None:\n    counter.value += 1\n\ndef peek(counter: {{borrow:Counter}}) -> int:\n    return counter.value\n\ndef twice(mut counter: {{borrow_mut:Counter}}) -> int:\n    bump(counter)\n    bump(counter)\n    return peek(counter)\n\ndef main() -> int:\n    mut owned = 1\n    view: {{borrow:int}} = owned\n    read: () -> int = () => view + 1\n    total = read()\n    owned += 10\n    mut counter = Counter(value=0)\n    bump(counter)\n    return total + owned + twice(counter)\n"
    );
    let checker = check_twins(&template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

/// A body that still needs the whole-body proof (it calls a returned-view function) calls borrow-only callees in
/// both spellings: a generic one, an async one awaited, and an async method through a borrowed receiver.
#[test]
fn proof_bodies_call_borrow_only_callees() -> Result<(), Box<dyn std::error::Error>> {
    let template = "class Selection[R]:\n    pub reader: R\n    pub key: int\n\nclass Buffer:\n    pub items: list[int]\n\n    pub async def size_plus(self, extra: {borrow:int}) -> int:\n        return len(self.items) + extra\n\ndef select(reader: Borrowed[list[int]]) -> Selection[Borrowed[list[int]]] from reader:\n    return Selection[Borrowed[list[int]]](reader=reader, key=0)\n\ndef selected_key[R](selection: {borrow:Selection[R]}) -> int:\n    return selection.key\n\nasync def measure(buffer: {borrow:Buffer}) -> int:\n    return len(buffer.items)\n\nasync def total(mut buffer: {borrow_mut:Buffer}) -> int:\n    first = await measure(buffer)\n    return await buffer.size_plus(first)\n\nasync def use(mut values: {borrow_mut:list[int]}, mut buffer: {borrow_mut:Buffer}) -> int:\n    selection = select(values)\n    key = selected_key(selection)\n    return key + await total(buffer)\n";
    let checker = check_twins(template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

const TRACE_STATE: &str = "class State:\n    pub active: str\n    pub count: int\n    pub items: list[int]\n\ndef write_end(mut state: {borrow_mut:State}, identifier: str) -> int:\n    state.count += 1\n    return len(identifier)\n\ndef check(mut state: {borrow_mut:State}, flag: bool) -> int:\n    state.count += 1\n    return state.count\n\ndef bump(mut state: {borrow_mut:State}) -> str:\n    state.count += 1\n    return state.active\n\ndef take_first(mut items: {borrow_mut:list[int]}) -> str:\n    return \"first\"\n\nclass Counter:\n    pub value: int\n\n    pub def add(mut self, step: int) -> None:\n        self.value += step\n\n";

/// Rust makes the implicit reborrow of a `&mut` binding passed as an argument, or of a method's receiver, a two-phase
/// borrow: it is reserved while the later arguments are evaluated and activated at the call. So an exclusive borrow
/// (a parameter or a local) passed first and a shared read, clone or comparison of its field in a later argument
/// check in both spellings, and so does a `mut self` receiver whose argument reads it. This is the release trace's
/// `trace_write_end(state, state.active_phase)` shape. A body that needs the whole-body proof keeps its borrows at the
/// argument, as the explicit route always did.
#[test]
fn two_phase_borrows_allow_later_reads_of_the_borrowed_place() -> Result<(), Box<dyn std::error::Error>> {
    let template = format!(
        "{TRACE_STATE}def phase(mut state: {{borrow_mut:State}}) -> int:\n    mut total = write_end(state, state.active)\n    total += write_end(state, state.active.clone())\n    total += check(state, state.count == 1)\n    return total\n\ndef held(mut owned: State) -> int:\n    mut view: {{borrow_mut:State}} = owned\n    return write_end(view, view.active)\n\ndef grow(mut counter: {{borrow_mut:Counter}}) -> None:\n    counter.add(counter.value)\n"
    );
    let checker = check_twins(&template)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    Ok(())
}

/// The reservation is a shared borrow: a later argument that passes the same place exclusively again, or borrows its
/// field exclusively, is still refused with INCAN-T0126 in both spellings, as rustc refuses it. An owned place lent
/// exclusively is an explicit borrow, not a reborrow, so a later read of it stays refused too.
#[test]
fn two_phase_borrows_still_refuse_later_exclusive_uses() -> Result<(), Box<dyn std::error::Error>> {
    for body in [
        "def again(mut state: {borrow_mut:State}) -> int:\n    return write_end(state, bump(state))\n",
        "def field(mut state: {borrow_mut:State}) -> int:\n    return write_end(state, take_first(state.items))\n",
        "def lent(start: int) -> int:\n    mut owned = State(active=\"a\", count=start, items=[])\n    return write_end(owned, owned.active)\n",
    ] {
        let checker = check_twins(&format!("{TRACE_STATE}{body}"))?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{body}: {:?}",
            checker.errors
        );
    }
    Ok(())
}
