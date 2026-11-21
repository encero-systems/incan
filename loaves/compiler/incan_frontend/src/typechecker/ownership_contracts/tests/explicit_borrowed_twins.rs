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
