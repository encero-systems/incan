//! The derive contract at check time: a set element hashed only through the `Hash` derive (#1822), automatic and
//! implied derives the checker sees (#1870), derive requirements and dunder signatures (#1871), dunders beside their
//! matching derives (#1872), `sorted()` over ordered types (#1881), and static methods without a receiver (#1882).

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Check `source` and return its errors, or fail when it is accepted.
fn refused(source: &str) -> Result<Vec<CompileError>, Box<dyn std::error::Error>> {
    match check_str(source) {
        Ok(()) => Err(format!("expected a refusal, but the program was accepted:\n{source}").into()),
        Err(errors) => Ok(errors),
    }
}

/// Check `source` and fail with its errors when it is refused.
fn accepted(source: &str) -> TestResult {
    check_str(source).map_err(|errors| {
        let messages = errors.iter().map(|error| error.message.as_str()).collect::<Vec<_>>();
        format!("expected the program to be accepted, got {messages:#?}\n{source}").into()
    })
}

/// Fail unless one of `errors` contains every one of `needles`.
fn assert_refusal(errors: &[CompileError], needles: &[&str]) -> TestResult {
    if errors
        .iter()
        .any(|error| needles.iter().all(|needle| error.message.contains(needle)))
    {
        return Ok(());
    }
    let messages = errors.iter().map(|error| error.message.as_str()).collect::<Vec<_>>();
    Err(format!("no error contains {needles:?}: {messages:#?}").into())
}

// ---- #1882: a static method takes no receiver ----

#[test]
fn issue1882_staticmethod_with_a_self_receiver_is_refused() -> TestResult {
    for (owner, receiver) in [
        ("class Counter:\n    count: int\n", "self"),
        ("model Counter:\n    count: int\n", "mut self"),
        ("enum Counter:\n    Zero\n", "self"),
        ("type Counter = newtype int:\n", "self"),
    ] {
        let source = format!("{owner}\n    @staticmethod\n    def make({receiver}) -> int:\n        return 1\n");
        let errors = refused(&source)?;
        assert_refusal(
            &errors,
            &["'make' is a @staticmethod", &format!("no '{receiver}' receiver")],
        )?;
    }
    Ok(())
}

#[test]
fn issue1882_static_and_class_methods_without_a_receiver_stay_accepted() -> TestResult {
    accepted(
        r#"
class Counter:
    pub count: int

    @staticmethod
    def make(n: int) -> Counter:
        return Counter(count=n)

    @classmethod
    def build(cls, n: int) -> Self:
        return cls(count=n)

def main() -> None:
    println(Counter.make(2).count + Counter.build(3).count)
"#,
    )
}

// ---- #1870: the checker sees automatic and implied derives ----

#[test]
fn issue1870_automatic_clone_satisfies_bounds_and_direct_calls() -> TestResult {
    accepted(
        r#"
model Point:
    x: int

class Box:
    pub size: int

enum Color:
    Red
    Blue

type Name = newtype str

def dup[T with Clone](value: T) -> T:
    return value.clone()

def main() -> None:
    p = dup(Point(x=1))
    q = p.clone()
    b = Box(size=1).clone()
    c = Color.Red.clone()
    n = Name("a").clone()
    println(q.x + b.size)
"#,
    )
}

#[test]
fn issue1870_implied_and_automatic_derives_satisfy_bounds() -> TestResult {
    accepted(
        r#"
@derive(Ord)
model Score:
    x: int

enum Level:
    Low(int)
    High

type Meters = newtype int
type Pair = newtype tuple[int, float]

def same[T with Eq](a: T, b: T) -> bool:
    return a == b

def partially_same[T with PartialEq](a: T, b: T) -> bool:
    return a == b

def less[T with PartialOrd](a: T, b: T) -> bool:
    return a < b

def copied[T with Copy](value: T) -> T:
    return value

def main() -> None:
    println(same(Score(x=1), Score(x=1)))
    println(less(Score(x=1), Score(x=2)))
    println(partially_same(Level.High, Level.Low(1)))
    m = copied(Meters(3))
    pair = copied(Pair((1, 2.0)))
"#,
    )
}

#[test]
fn issue1870_equality_without_eq_is_refused() -> TestResult {
    for (declaration, left, right) in [
        ("model Point:\n    x: int\n", "Point(x=1)", "Point(x=1)"),
        ("class Point:\n    x: int\n", "Point(x=1)", "Point(x=1)"),
        ("model Point[T]:\n    x: T\n", "Point(x=1)", "Point(x=1)"),
        ("type Point = newtype int\n", "Point(1)", "Point(1)"),
        (
            "model Inner:\n    x: int\n\nenum Point:\n    Dot(Inner)\n    Empty\n",
            "Point.Empty",
            "Point.Empty",
        ),
        ("model Point:\n    x: int\n", "[Point(x=1)]", "[Point(x=1)]"),
        ("model Point:\n    x: int\n", "(Point(x=1), 1)", "(Point(x=1), 1)"),
    ] {
        for operator in ["==", "!="] {
            let source = format!("{declaration}\ndef main() -> None:\n    println({left} {operator} {right})\n");
            let errors = refused(&source)?;
            assert_refusal(&errors, &[&format!("does not support '{operator}'")])?;
        }
    }
    Ok(())
}

#[test]
fn issue1870_list_membership_without_eq_is_refused() -> TestResult {
    for operator in ["in", "not in"] {
        let source = format!(
            "model Point:\n    x: int\n\ndef main() -> None:\n    items = [Point(x=1)]\n    println(Point(x=1) {operator} items)\n"
        );
        let errors = refused(&source)?;
        assert_refusal(&errors, &[&format!("'Point' does not support '{operator}'"), "__eq__"])?;
    }
    accepted(
        "@derive(Eq)\nmodel Point:\n    x: int\n\ndef main() -> None:\n    items = [Point(x=1)]\n    println(Point(x=1) in items)\n",
    )
}

#[test]
fn issue1870_ordering_of_an_unordered_builtin_is_refused() -> TestResult {
    let errors = refused(
        r#"
def main() -> None:
    a = {"a": 1}
    b = {"a": 2}
    println(a < b)
"#,
    )?;
    assert_refusal(&errors, &["does not support '<'"])
}

#[test]
fn issue1870_equality_through_derives_and_dunders_stays_accepted() -> TestResult {
    accepted(
        r#"
@derive(Eq)
model A:
    x: int

@derive(Ord)
class B:
    x: int

enum C:
    Dot(int)
    Empty

@rust.derive(PartialEq)
model D:
    x: int

model E:
    x: int

    def __eq__(self, other: E) -> bool:
        return self.x == other.x

@derive(PartialEq)
type F = newtype int

def main() -> None:
    println(A(x=1) == A(x=1))
    println(B(x=1) != B(x=2))
    println(C.Dot(1) == C.Empty)
    println(D(x=1) != D(x=1))
    println(E(x=1) == E(x=1))
    println(F(1) == F(1))
    println([A(x=1)] == [A(x=1)])
    println(1.5 < 2.5)
    println([1, 2] < [1, 3])
"#,
    )
}

// ---- #1871: derive requirements, applicability and dunder signatures ----

#[test]
fn issue1871_derive_whose_trait_a_member_lacks_is_refused() -> TestResult {
    for (source, needles) in [
        (
            "@derive(Copy)\nmodel Label:\n    text: str\n",
            vec!["@derive(Copy) on model 'Label'", "field 'text' has type 'str'"],
        ),
        (
            "@derive(Copy)\ntype Name = newtype str\n",
            vec!["@derive(Copy) on newtype 'Name' needs Copy of its underlying type 'str'"],
        ),
        (
            "model Plain:\n    x: int\n\n@derive(Copy)\nmodel Bad:\n    plain: Plain\n",
            vec!["@derive(Copy) on model 'Bad'", "field 'plain'"],
        ),
        (
            "@derive(Copy)\nenum Color:\n    Red\n    Named(str)\n",
            vec!["@derive(Copy) on enum 'Color'", "variant 'Named'"],
        ),
        (
            "@derive(Eq)\nmodel V:\n    x: float\n",
            vec!["@derive(Eq) on model 'V'", "does not implement Eq"],
        ),
        (
            "@derive(Hash)\nclass V:\n    x: list[float]\n",
            vec!["@derive(Hash) on class 'V'", "whose 'float' does not implement Hash"],
        ),
        (
            "@derive(Ord)\nmodel V:\n    x: float\n",
            vec!["@derive(Ord) on model 'V'", "does not implement Ord and Eq"],
        ),
        (
            "model Plain:\n    x: int\n\n@derive(Eq)\nmodel V:\n    p: Plain\n",
            vec!["@derive(Eq) on model 'V'", "does not implement Eq and PartialEq"],
        ),
        (
            "model Inner:\n    x: int\n\n@derive(Default)\nmodel V:\n    inner: Inner\n",
            vec![
                "@derive(Default) on model 'V'",
                "field 'inner'",
                "does not implement Default",
            ],
        ),
    ] {
        let errors = refused(source)?;
        assert_refusal(&errors, &needles)?;
    }
    Ok(())
}

#[test]
fn issue1871_partial_order_without_equality_and_inapplicable_derives_are_refused() -> TestResult {
    let errors = refused("@derive(PartialOrd)\nmodel Score:\n    x: int\n")?;
    assert_refusal(&errors, &["@derive(PartialOrd) on model 'Score' needs PartialEq"])?;
    let errors = refused("@derive(Default)\nenum Color:\n    Red\n    Blue\n")?;
    assert_refusal(
        &errors,
        &[
            "@derive(Default) applies to a model, class or newtype",
            "'Color' is enum",
        ],
    )?;
    let errors = refused(
        "@derive(Validate)\nclass C:\n    x: int\n\n    def validate(self) -> Result[C, str]:\n        return Ok(self)\n",
    )?;
    assert_refusal(&errors, &["@derive(Validate) applies to a model", "'C' is class"])
}

#[test]
fn issue1871_dunder_with_the_wrong_signature_is_refused() -> TestResult {
    for (method, dunder, expected) in [
        (
            "def __eq__(self, other: int) -> bool:\n        return true",
            "__eq__",
            "(self, other: Self) -> bool",
        ),
        (
            "def __eq__(self, other: User) -> int:\n        return 1",
            "__eq__",
            "(self, other: Self) -> bool",
        ),
        (
            "def __lt__(self, other: str) -> bool:\n        return true",
            "__lt__",
            "(self, other: Self) -> bool",
        ),
        (
            "def __ne__(mut self, other: User) -> bool:\n        return true",
            "__ne__",
            "(self, other: Self) -> bool",
        ),
        (
            "def __str__(self, extra: int) -> str:\n        return \"u\"",
            "__str__",
            "(self) -> str",
        ),
        (
            "def __str__(self) -> int:\n        return 1",
            "__str__",
            "(self) -> str",
        ),
    ] {
        let source = format!("model User:\n    id: int\n\n    {method}\n");
        let errors = refused(&source)?;
        assert_refusal(
            &errors,
            &[&format!("'User.{dunder}' must have the signature {expected}")],
        )?;
    }
    Ok(())
}

#[test]
fn issue1871_derives_and_dunders_that_meet_the_contract_stay_accepted() -> TestResult {
    accepted(
        r#"
@derive(Copy)
model Point:
    x: int
    y: float
    on: bool

@derive(Copy)
model Outer:
    inner: Point

@derive(Copy)
model Pair[T]:
    a: T

@derive(Eq, Hash, Ord)
model Key:
    id: int
    name: str
    tags: list[str]

@derive(PartialEq, PartialOrd)
model Reading:
    value: float

@derive(Default)
model Settings:
    theme: str = "dark"
    retries: int
    inner: Point = Point(x=0, y=0.0, on=false)

model Custom:
    x: int

    def __eq__(self, other: Custom) -> bool:
        return self.x == other.x

@derive(PartialOrd)
model Ordered:
    x: int

    def __eq__(self, other: Self) -> bool:
        return self.x == other.x

model Box[T]:
    v: T

    def __eq__(self, other: Box[T]) -> bool:
        return true

    def __str__(self) -> str:
        return "box"

@derive(Copy, Eq, Hash, Ord, Default)
type Rank = newtype int

@derive(PartialOrd)
enum Level:
    Low(int)
    High
"#,
    )
}

// ---- #1872: a dunder beside its matching derive ----

#[test]
fn issue1872_dunder_beside_its_matching_derive_is_refused() -> TestResult {
    for (decorator, owner, dunder, derive) in [
        (
            "@derive(Eq)",
            "model User:\n    id: int\n",
            "def __eq__(self, other: Self) -> bool:\n        return true",
            "Eq",
        ),
        (
            "@derive(PartialEq)",
            "class User:\n    id: int\n",
            "def __eq__(self, other: Self) -> bool:\n        return true",
            "PartialEq",
        ),
        (
            "@derive(Ord)",
            "model User:\n    id: int\n",
            "def __eq__(self, other: Self) -> bool:\n        return true",
            "Ord",
        ),
        (
            "@derive(Eq)",
            "model User:\n    id: int\n",
            "def __ne__(self, other: Self) -> bool:\n        return true",
            "Eq",
        ),
        (
            "@derive(Ord)",
            "model User:\n    id: int\n",
            "def __lt__(self, other: Self) -> bool:\n        return true",
            "Ord",
        ),
        (
            "@derive(Ord)",
            "model User:\n    id: int\n",
            "def __le__(self, other: Self) -> bool:\n        return true",
            "Ord",
        ),
        (
            "@derive(PartialEq, PartialOrd)",
            "model User:\n    id: int\n",
            "def __gt__(self, other: Self) -> bool:\n        return true",
            "PartialOrd",
        ),
        (
            "@derive(Display)",
            "model User:\n    id: int\n",
            "def __str__(self) -> str:\n        return \"u\"",
            "Display",
        ),
        (
            "@rust.derive(PartialEq)",
            "model User:\n    id: int\n",
            "def __eq__(self, other: Self) -> bool:\n        return true",
            "PartialEq",
        ),
        (
            "@derive(Eq)",
            "enum User:\n    A\n    B\n",
            "def __eq__(self, other: Self) -> bool:\n        return true",
            "Eq",
        ),
        (
            "@derive(Eq)",
            "type User = newtype int:\n",
            "def __eq__(self, other: Self) -> bool:\n        return true",
            "Eq",
        ),
    ] {
        let source = format!("{decorator}\n{owner}\n    {method}\n", method = dunder);
        let errors = refused(&source)?;
        let name = dunder.trim_start_matches("def ").split('(').next().unwrap_or_default();
        assert_refusal(&errors, &[&format!("'User' defines {name} and derives {derive}")])?;
    }
    Ok(())
}

/// A `__str__` inherited from an extended class or supplied by an adopted trait gives the type its display as a
/// declared one does, so `@derive(Display)` beside it is refused too.
#[test]
fn issue1872_derived_display_beside_an_inherited_or_adopted_str_is_refused() -> TestResult {
    let inherited = refused(
        r#"
class Base:
    pub id: int

    def __str__(self) -> str:
        return "base"

@derive(Display)
class Child extends Base:
    pub extra: int
"#,
    )?;
    assert_refusal(&inherited, &["'Child' defines __str__ and derives Display"])?;
    let adopted = refused(
        r#"
trait Named:
    def __str__(self) -> str:
        return "named"

@derive(Display)
model Thing with Named:
    id: int
"#,
    )?;
    assert_refusal(&adopted, &["'Thing' defines __str__ and derives Display"])
}

#[test]
fn issue1872_dunders_without_their_matching_derive_stay_accepted() -> TestResult {
    accepted(
        r#"
@derive(Hash, Clone)
model Key:
    id: int

    def __hash__(self) -> int:
        return self.id

@derive(Display)
model Shown:
    id: int

@derive(PartialOrd)
model Ordered:
    id: int

    def __eq__(self, other: Self) -> bool:
        return self.id == other.id

enum Color:
    Red
    Blue

    def __eq__(self, other: Self) -> bool:
        return true

def main() -> None:
    println(Color.Red == Color.Blue)
    println(Ordered(id=1) < Ordered(id=2))
"#,
    )
}

// ---- #1881: sorted() over an ordered element type ----

#[test]
fn issue1881_sorted_accepts_ordered_element_types() -> TestResult {
    accepted(
        r#"
@derive(Ord)
model Task:
    priority: int

@derive(Ord)
enum Level:
    Low
    High

@derive(Ord)
type Rank = newtype int

def main() -> None:
    tasks = sorted([Task(priority=2), Task(priority=1)])
    levels = sorted([Level.High, Level.Low])
    ranks = sorted([Rank(3), Rank(1)])
    pairs = sorted([(2, "b"), (1, "a")])
    nested = sorted([[2], [1]])
    floats = sorted([2.5, 1.5])
    println(tasks[0].priority)
"#,
    )
}

#[test]
fn issue1881_sorted_refuses_element_types_without_a_total_order() -> TestResult {
    for (declaration, values, needle) in [
        (
            "model Task:\n    priority: int\n",
            "[Task(priority=2)]",
            "'Task' does not implement Ord",
        ),
        (
            "@derive(PartialEq, PartialOrd)\nmodel Task:\n    priority: float\n",
            "[Task(priority=2.0)]",
            "'Task' does not implement Ord",
        ),
        (
            "model Task:\n    priority: int\n",
            "[(1, Task(priority=2))]",
            "its 'Task' does not implement Ord",
        ),
        ("", "[{1: 2}]", "does not implement Ord"),
    ] {
        let source = format!("{declaration}\ndef main() -> None:\n    values = sorted({values})\n");
        let errors = refused(&source)?;
        assert_refusal(&errors, &["sorted() needs list elements with a total order", needle])?;
    }
    Ok(())
}

// ---- #1822: `__hash__` is an ordinary method ----

#[test]
fn issue1822_a_hash_method_does_not_make_a_set_element() -> TestResult {
    for declaration in [
        "model Key:\n    id: int\n\n    def __eq__(self, other: Key) -> bool:\n        return self.id == other.id\n\n    def __hash__(self) -> int:\n        return self.id\n",
        "from std.derives.comparison import Hash\n\n@derive(Eq)\nmodel Key with Hash:\n    id: int\n\n    def __hash__(self) -> int:\n        return self.id\n",
    ] {
        let source = format!("{declaration}\ndef main() -> None:\n    seen: set[Key] = {{Key(id=1), Key(id=1)}}\n");
        let errors = refused(&source)?;
        assert_refusal(&errors, &["'Key' cannot be a set element", "Hash"])?;
    }
    Ok(())
}

#[test]
fn issue1822_a_hash_method_beside_the_derive_stays_an_ordinary_method() -> TestResult {
    accepted(
        r#"
@derive(Eq, Hash)
model Key:
    id: int

    def __hash__(self) -> int:
        return self.id

def main() -> None:
    seen: set[Key] = {Key(id=1), Key(id=1)}
    println(len(seen))
    println(Key(id=3).__hash__())
"#,
    )
}

/// A `Hash` bound is met only by the derive, which gives no `__hash__`, so calling it through the bound is refused
/// under either spelling of the trait; the method stays callable on a type that defines it (#1561).
#[test]
fn hash_method_through_a_hash_bound_is_refused_issue1561() -> TestResult {
    for import in [
        "from std.derives.comparison import Hash",
        "from std.derives.comparison import Hash as Hashable",
    ] {
        let bound = import.rsplit(' ').next().unwrap_or("Hash");
        let source =
            format!("{import}\n\ndef hash_of[T with {bound}](value: T) -> int:\n    return value.__hash__()\n");
        let errors = refused(&source)?;
        assert_refusal(
            &errors,
            &["'__hash__' cannot be called on 'T' through its 'Hash' bound"],
        )?;
    }
    accepted(
        r#"
from std.derives.comparison import Hash

model Hashed with Hash:
    value: int

    def __hash__(self) -> int:
        return self.value

def main() -> None:
    println(Hashed(value=5).__hash__())
"#,
    )
}

/// In a type's own dunder, a method called on its `Self` parameter resolves on the type, so a missing one is refused;
/// and an `Ord` adoption named through its module (`comparison.Ord`) provides `sorted()` (#1561).
#[test]
fn comparison_adopters_resolve_self_parameters_and_module_qualified_adoptions_issue1561() -> TestResult {
    let errors = refused(
        r#"
from std.derives.comparison import Eq

model Key with Eq:
    id: int

    def __eq__(self, other: Self) -> bool:
        return other.missing() == self.id
"#,
    )?;
    assert_refusal(&errors, &["missing"])?;
    accepted(
        r#"
from std.derives import comparison

model Score with comparison.Ord:
    points: int

    def __eq__(self, other: Self) -> bool:
        return self.points == other.points

    def __lt__(self, other: Self) -> bool:
        return self.points < other.points

def main() -> None:
    println(sorted([Score(points=2), Score(points=1)])[0].points)
"#,
    )
}

/// A newtype is `Copy` without a derive when its underlying type is a builtin `Copy` value (a number, `bool`, or a
/// tuple, `Option` or `Result` of those); over another newtype or a model that derives `Copy` it is `Copy` only by
/// deriving it.
#[test]
fn newtype_copy_follows_a_builtin_underlying_type_or_its_own_derive() -> TestResult {
    accepted(
        r#"
type Meters = newtype int
type Span = newtype tuple[int, float]
type Maybe = newtype Option[bool]

@derive(Copy)
type Distance = newtype Meters

@derive(Copy)
model Leg:
    meters: Meters
    span: Span
    maybe: Maybe
    distance: Distance
"#,
    )?;
    for (declarations, holder) in [
        (
            "type Meters = newtype int\ntype Distance = newtype Meters\n",
            "Distance",
        ),
        (
            "@derive(Copy)\nmodel Point:\n    x: int\n\ntype Place = newtype Point\n",
            "Place",
        ),
    ] {
        let source = format!("{declarations}\n@derive(Copy)\nmodel Leg:\n    value: {holder}\n");
        let errors = refused(&source)?;
        assert_refusal(&errors, &["@derive(Copy) on model 'Leg'", holder])?;
    }
    Ok(())
}

/// RFC 000 `@derive(Display)` displays a value as its `Debug` structure, so on a newtype it needs `Debug` of the
/// underlying type, and a newtype over a type without it is refused.
#[test]
fn derived_display_on_a_newtype_needs_debug_of_its_underlying_type() -> TestResult {
    let errors = refused(
        "import std.async\nfrom std.async.task import JoinHandle\n\n@derive(Display)\ntype Handle = newtype JoinHandle[int]\n",
    )?;
    assert_refusal(
        &errors,
        &[
            "@derive(Display) on newtype 'Handle'",
            "underlying type 'JoinHandle[int]'",
        ],
    )?;
    accepted("@derive(Display)\ntype UserId = newtype int\n\ndef main() -> None:\n    println(UserId(1))\n")
}

/// A newtype lacks the derives its underlying type lacks, and deriving one of them on the newtype is refused.
#[test]
fn newtype_derive_its_underlying_type_lacks_is_refused() -> TestResult {
    let errors = refused(
        "import std.async\nfrom std.async.task import JoinHandle\n\n@derive(Clone)\ntype Handle = newtype JoinHandle[int]\n",
    )?;
    assert_refusal(
        &errors,
        &[
            "@derive(Clone) on newtype 'Handle'",
            "underlying type 'JoinHandle[int]'",
        ],
    )
}
