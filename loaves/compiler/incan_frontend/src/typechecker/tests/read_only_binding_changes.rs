//! Changes through a binding declared without `mut` (#1561): a call of a method that changes its receiver through a
//! local declared without `mut` or a parameter not marked `mut`, or through a field or element of one, and a change
//! through the variable of a `for` loop that reads the items of such a place in place, are refused with `INCAN-T0001`.
//! A change the checker cannot classify, and a change to a value only the call or the loop holds, are accepted.

use super::*;

/// Check a program and return the messages of its errors, each asserted to carry the typecheck code `INCAN-T0001`.
fn refusal_messages(source: &str) -> Vec<String> {
    check_str(source)
        .err()
        .unwrap_or_default()
        .into_iter()
        .map(|error| {
            assert_eq!(
                error.stable_code(),
                None,
                "the refusal takes the typecheck code INCAN-T0001: {error:?}"
            );
            error.message
        })
        .collect()
}

const COUNTER: &str = r#"
class Counter:
    pub count: int

    def bump(mut self) -> None:
        self.count += 1

    def read(self) -> int:
        return self.count


class Holder:
    pub counter: Counter
    pub rows: list[list[int]]
"#;

/// A `mut self` method, a changing `list`, `dict` or `set` method, through a local declared by `let` or a first plain
/// assignment, a field or an element of one, a dict value, a tuple element, or a parameter not marked `mut`, is
/// refused.
#[test]
fn changing_method_calls_through_read_only_bindings_are_refused_issue1561() {
    let source = format!(
        r#"{COUNTER}

def by_param(items: list[int], counter: Counter) -> None:
    items.append(1)
    counter.bump()


def main() -> None:
    counter = Counter(count=0)
    counter.bump()
    let holder = Holder(counter=Counter(count=0), rows=[[1]])
    holder.counter.bump()
    holder.rows[0].append(2)
    items: list[int] = []
    items.append(1)
    tags = {{"a"}}
    tags.add("b")
    table: dict[str, list[int]] = {{"a": [1]}}
    table["a"].append(2)
    table.insert("b", [])
    pair: tuple[Counter, int] = (Counter(count=0), 1)
    pair[0].bump()
"#
    );
    assert_eq!(
        refusal_messages(&source),
        [
            "Cannot mutate 'items' - variable is immutable",
            "Cannot mutate 'counter' - variable is immutable",
            "Cannot mutate 'counter' - variable is immutable",
            "Cannot mutate 'holder' - variable is immutable",
            "Cannot mutate 'holder' - variable is immutable",
            "Cannot mutate 'items' - variable is immutable",
            "Cannot mutate 'tags' - variable is immutable",
            "Cannot mutate 'table' - variable is immutable",
            "Cannot mutate 'table' - variable is immutable",
            "Cannot mutate 'pair' - variable is immutable",
        ]
    );
}

/// A `for` loop or a list comprehension whose body changes its items through its variable, over a place declared
/// without `mut` read in place
/// directly, through `enumerate`, `zip` or `values()`, through a field, a list element or a tuple element, over an
/// enclosing loop's variable, over a parameter not marked `mut`, over a static, and over `self` in a plain `self`
/// method, is refused.
#[test]
fn loops_changing_the_items_of_read_only_places_are_refused_issue1561() {
    let source = format!(
        r#"{COUNTER}

class Grid:
    pub rows: list[list[int]]

    def fill(self) -> None:
        for row in self.rows:
            row.append(1)


static ROWS: list[list[int]] = [[1]]


def by_param(rows: list[list[int]]) -> None:
    for row in rows:
        row.append(1)


def main() -> None:
    rows: list[list[int]] = [[1]]
    for row in rows:
        row.append(2)
    for i, row in enumerate(rows):
        row.append(i)
    extra: list[int] = [3]
    for row, n in zip(rows, extra):
        row.append(n)
    table: dict[str, list[int]] = {{"a": [1]}}
    for value in table.values():
        value.append(2)
    holder = Holder(counter=Counter(count=0), rows=[[1]])
    for row in holder.rows:
        row[0] = 5
    groups: list[list[list[int]]] = [[[1]]]
    for row in groups[0]:
        row.append(2)
    holders = [Holder(counter=Counter(count=0), rows=[[1]])]
    for row in holders[0].rows:
        row.append(2)
    for group in groups:
        for row in group:
            row.append(3)
    counters = [Counter(count=0)]
    for each in counters:
        each.bump()
    for row in ROWS:
        row.append(4)
    pair: tuple[list[list[int]], int] = ([[1]], 0)
    for row in pair[0]:
        row.append(5)
    popped = [row.pop() for row in rows]
"#
    );
    assert_eq!(
        refusal_messages(&source),
        [
            "Cannot change 'row' - it is bound from 'self', which this method takes as plain 'self'",
            "Cannot change 'row' - it is bound from the parameter 'rows', which is not declared 'mut'",
            "Cannot change 'row' - it is bound from 'rows', which is immutable",
            "Cannot change 'row' - it is bound from 'rows', which is immutable",
            "Cannot change 'row' - it is bound from 'rows', which is immutable",
            "Cannot change 'value' - it is bound from 'table', which is immutable",
            "Cannot change 'row' - it is bound from 'holder', which is immutable",
            "Cannot change 'row' - it is bound from 'groups', which is immutable",
            "Cannot change 'row' - it is bound from 'holders', which is immutable",
            "Cannot change 'row' - it is bound from 'groups', which is immutable",
            "Cannot change 'each' - it is bound from 'counters', which is immutable",
            "Cannot change 'row' - it is bound from the static 'ROWS', which is changed only through its own name",
            "Cannot change 'row' - it is bound from 'pair', which is immutable",
            "Cannot change 'row' - it is bound from 'rows', which is immutable",
        ]
    );
}

/// Only a known change through a place that does not permit it is refused. A `mut` local or parameter, a `mut` tuple
/// binding, a static and a local bound to one, a read-only method, a generator a local holds, a value only the call
/// holds, a loop over values of its own and a loop over a `mut` place through `enumerate`, `zip`, `values()` or an
/// element all accept a change.
#[test]
fn changes_that_the_place_permits_are_accepted_issue1561() {
    assert_check_ok(&format!(
        r#"{COUNTER}

static ITEMS: list[int] = []


def numbers() -> Generator[int]:
    yield 1


def fresh() -> list[list[int]]:
    return [[1]]


def by_param(mut items: list[int], mut rows: list[list[int]], counter: Counter) -> int:
    items.append(1)
    for row in rows:
        row.append(2)
    return counter.read()


def main() -> None:
    mut counter = Counter(count=0)
    counter.bump()
    fixed = Counter(count=0)
    println(fixed.read())
    mut items: list[int] = []
    items.append(1)
    println(items.count(1))
    ITEMS.append(1)
    live = ITEMS
    live.append(2)
    generated = numbers()
    println(len(generated.collect()))
    looped = numbers()
    for n in looped:
        println(n)
    fresh().append([2])
    for row in fresh():
        row.append(3)
    for row in [[1], [2]]:
        row.append(4)
    mut rows: list[list[int]] = [[1]]
    for i, row in enumerate(rows):
        row.append(i)
    for row, n in zip(rows, [1]):
        row.append(n)
    mut table: dict[str, list[int]] = {{"a": [1]}}
    for value in table.values():
        value.append(2)
    table["a"].append(3)
    mut groups: list[list[list[int]]] = [[[1]]]
    for row in groups[0]:
        row.append(4)
    mut pair: tuple[Counter, list[list[int]]] = (Counter(count=0), [[1]])
    pair[0].bump()
    for row in pair[1]:
        row.append(5)
    pair = (Counter(count=1), [])
    popped = [row.pop() for row in rows]
    mut holders = [Holder(counter=Counter(count=0), rows=[[1]])]
    for row in holders[0].rows:
        row.append(6)
    println(by_param(items, rows, fixed))
"#
    ));
}
