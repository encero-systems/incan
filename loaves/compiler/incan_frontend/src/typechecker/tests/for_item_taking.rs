//! `for` loops that take the items of their list (#1844): which loops do, the fact recorded for lowering, and the
//! `INCAN-T0119` refusal of a list used again after, inside, or by a repeat of such a loop.

use super::*;

/// The imports and the `work` task every program here starts with.
const PRELUDE: &str = "from std.async import spawn, JoinHandle\n\nasync def work() -> int:\n    return 1\n\n";

/// Check `PRELUDE` followed by `body`, returning the checker so a test can read the recorded facts.
fn checked(body: &str) -> Result<(String, TypeChecker), Vec<CompileError>> {
    let source = format!("{PRELUDE}{body}");
    let tokens = lexer::lex(&source)?;
    let program = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&program)?;
    Ok((source, checker))
}

/// The span of the `nth` occurrence of `needle` in `source`.
fn span_of(source: &str, needle: &str, nth: usize) -> Result<Span, String> {
    let start = source
        .match_indices(needle)
        .nth(nth)
        .map(|(start, _)| start)
        .ok_or_else(|| format!("`{needle}` occurs fewer than {} times", nth + 1))?;
    Ok(Span::new(start, start + needle.len()))
}

/// The iterable span of the `for` loop whose header is `for <binding> in <list>:`, the `nth` such header.
fn loop_iterable(source: &str, header: &str, nth: usize) -> Result<Span, String> {
    let header_span = span_of(source, header, nth)?;
    let list = header
        .trim_end_matches(':')
        .rsplit(' ')
        .next()
        .ok_or_else(|| format!("`{header}` names no list"))?;
    let start = header_span.end - 1 - list.len();
    Ok(Span::new(start, start + list.len()))
}

/// The `INCAN-T0119` refusals among `errors`.
fn taken_list_refusals(errors: &[CompileError]) -> Vec<&CompileError> {
    errors
        .iter()
        .filter(|error| error.stable_code() == Some("INCAN-T0119"))
        .collect()
}

/// #1844: awaiting each handle of a list takes the handles out of the list, and so do returning a handle and passing
/// one to a call. The checker records each such loop for lowering.
#[test]
fn loop_handing_each_handle_on_takes_the_list_items_issue1844() -> Result<(), Box<dyn std::error::Error>> {
    let (source, checker) = checked(
        r#"def first(handles: list[JoinHandle[int]]) -> Option[JoinHandle[int]]:
    for handle in handles:
        return Some(handle)
    return None

async def wait_one(handle: JoinHandle[int]) -> int:
    match await handle:
        Ok(value) => return value
        Err(_) => return 0

async def main() -> None:
    handles = [spawn(work()), spawn(work())]
    for handle in handles:
        match await handle:
            Ok(value) => println(value)
            Err(_) => println("join failed")
    passed = [spawn(work())]
    mut total = 0
    for handle in passed:
        total += await wait_one(handle)
    println(total)
"#,
    )
    .map_err(|errors| format!("the program must check: {errors:?}"))?;
    let info = checker.type_info();
    assert!(info.for_loop_takes_items(loop_iterable(&source, "for handle in handles:", 0)?));
    assert!(info.for_loop_takes_items(loop_iterable(&source, "for handle in handles:", 1)?));
    assert!(info.for_loop_takes_items(loop_iterable(&source, "for handle in passed:", 0)?));
    Ok(())
}

/// #1844: a read of the list after the loop that took its items is refused, naming the list and pointing back at the
/// loop.
#[test]
fn list_read_after_the_loop_that_took_its_items_is_refused_issue1844() -> Result<(), Box<dyn std::error::Error>> {
    let body = r#"async def main() -> None:
    handles = [spawn(work()), spawn(work())]
    for handle in handles:
        match await handle:
            Ok(value) => println(value)
            Err(_) => println("join failed")
    println(len(handles))
"#;
    let source = format!("{PRELUDE}{body}");
    let errors = checked(body).err().ok_or("reading the emptied list must be refused")?;
    let refusals = taken_list_refusals(&errors);
    let [refusal] = refusals.as_slice() else {
        return Err(format!("expected one INCAN-T0119 refusal, got {errors:?}").into());
    };
    assert_eq!(
        refusal.message,
        "`handles` is used after the `for` loop that took its items"
    );
    assert_eq!(
        refusal.span,
        span_of(&source, "handles", 2)?,
        "the refusal points at the later read"
    );
    assert_eq!(
        refusal.related_spans().first().map(|related| related.span),
        Some(loop_iterable(&source, "for handle in handles:", 0)?),
        "the refusal points back at the loop"
    );
    Ok(())
}

/// #1844: a read of the list inside the loop that takes its items is refused too.
#[test]
fn list_read_inside_the_loop_that_takes_its_items_is_refused_issue1844() -> Result<(), Box<dyn std::error::Error>> {
    let errors = checked(
        r#"async def main() -> None:
    handles = [spawn(work()), spawn(work())]
    for handle in handles:
        println(len(handles))
        match await handle:
            Ok(value) => println(value)
            Err(_) => println("join failed")
"#,
    )
    .err()
    .ok_or("reading the list inside the loop must be refused")?;
    let refusals = taken_list_refusals(&errors);
    let [refusal] = refusals.as_slice() else {
        return Err(format!("expected one INCAN-T0119 refusal, got {errors:?}").into());
    };
    assert_eq!(
        refusal.message,
        "`handles` is used inside the `for` loop that takes its items"
    );
    Ok(())
}

/// #1844: an enclosing loop that repeats the loop over a list built outside it is refused, and a list built inside the
/// enclosing loop is iterated afresh on each pass.
#[test]
fn enclosing_loop_repeating_a_taking_loop_is_refused_issue1844() -> Result<(), Box<dyn std::error::Error>> {
    let errors = checked(
        r#"async def main() -> None:
    handles = [spawn(work())]
    mut rounds = 0
    while rounds < 2:
        for handle in handles:
            match await handle:
                Ok(value) => println(value)
                Err(_) => println("join failed")
        rounds += 1
"#,
    )
    .err()
    .ok_or("repeating the loop over the emptied list must be refused")?;
    let refusals = taken_list_refusals(&errors);
    let [refusal] = refusals.as_slice() else {
        return Err(format!("expected one INCAN-T0119 refusal, got {errors:?}").into());
    };
    assert_eq!(
        refusal.message,
        "the `for` loop that takes the items of `handles` runs again when the enclosing loop repeats"
    );

    checked(
        r#"async def main() -> None:
    mut rounds = 0
    while rounds < 2:
        handles = [spawn(work())]
        for handle in handles:
            match await handle:
                Ok(value) => println(value)
                Err(_) => println("join failed")
        rounds += 1
"#,
    )
    .map_err(|errors| format!("a list built inside the enclosing loop must check: {errors:?}"))?;
    Ok(())
}

/// #1844: copyable and cloneable items are copied out of their list, so the list stays readable after the loop and no
/// loop takes its items: strings, integers, models, a `std.fs` `Path` whose name a compiler-known type shares, and a
/// type parameter. A loop that does not hand the handle on leaves the handles in the list.
#[test]
fn copyable_and_cloneable_items_stay_in_their_list_issue1844() -> Result<(), Box<dyn std::error::Error>> {
    let (source, checker) = checked(
        r#"from std.fs import Path

model Item:
    name: str

def shout(text: str) -> str:
    return text.upper()

def describe(item: Item) -> str:
    return item.name

def show(path: Path) -> str:
    return path.name()

def first_of[T](items: list[T]) -> Option[T]:
    for item in items:
        return Some(item)
    return None

async def main() -> None:
    names = ["ada", "grace"]
    for name in names:
        println(shout(name))
    values = [1, 2]
    for value in values:
        println(value)
    items = [Item(name="a")]
    for item in items:
        println(describe(item))
    paths = [Path("a")]
    for path in paths:
        println(show(path))
    handles = [spawn(work())]
    for handle in handles:
        pass
    println(len(names) + len(values) + len(items) + len(paths) + len(handles))
"#,
    )
    .map_err(|errors| format!("the program must check: {errors:?}"))?;
    let info = checker.type_info();
    for header in [
        "for item in items:",
        "for name in names:",
        "for value in values:",
        "for path in paths:",
        "for handle in handles:",
    ] {
        let occurrences = source.matches(header).count();
        for nth in 0..occurrences {
            assert!(
                !info.for_loop_takes_items(loop_iterable(&source, header, nth)?),
                "`{header}` must leave the items in the list"
            );
        }
    }
    Ok(())
}
