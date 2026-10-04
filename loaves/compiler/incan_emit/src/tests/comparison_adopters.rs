//! Types that adopt `std.derives.comparison.Eq` or `Ord` and define their dunders build and compare (#1561): each
//! comparison operator, `sorted(values)`, a set element with `@derive(Hash)` and the `T with Eq` and `T with Ord`
//! bounds reach the type's own `__eq__` and `__lt__`, and the defaults `Ord` supplies for the rest, or the type's own
//! override of one. A dunder called through a builtin bound is the operation it defines, `__ne__` reaches the `Eq`
//! default of an adopter that imports only `Ord`, a trait default passes `self` to a method that takes its own type by
//! value, and a trait default's call of a method its trait declares has the declared result. An adopter declared in
//! another module, and a type that derives `Ord`, get the ordering dunders as their operators.

use super::generated_programs::{run_modules_with_stdlib, run_project_module_with_stdlib, run_with_stdlib};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// An `Eq` adopter, two `Ord` adopters (one reversing its order and defining its own `__le__`), and an adopter whose
/// dunders take `other` as the declaring type, compared by every operator, sorted, held in a set and passed to
/// `Eq`- and `Ord`-bounded functions.
#[test]
fn eq_and_ord_adopters_build_and_compare_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
from std.derives.comparison import Eq, Ord


@derive(Hash)
model UserKey with Eq:
    id: int

    def __eq__(self, other: UserKey) -> bool:
        return self.id == other.id


model Score with Ord:
    points: int

    def __eq__(self, other: Self) -> bool:
        return self.points == other.points

    def __lt__(self, other: Self) -> bool:
        return self.points < other.points


model Reversed with Ord:
    value: int

    def __eq__(self, other: Reversed) -> bool:
        return self.value == other.value

    def __lt__(self, other: Reversed) -> bool:
        return self.value > other.value

    def __le__(self, other: Reversed) -> bool:
        return true


def same[T with Eq](left: T, right: T) -> bool:
    return left == right


def before[T with Ord](left: T, right: T) -> bool:
    return left < right


def lowest[T with (Ord, Clone)](items: list[T]) -> T:
    return sorted(items)[0]


def main() -> None:
    keys: set[UserKey] = {UserKey(id=1), UserKey(id=1), UserKey(id=2)}
    println(len(keys))
    println(same(UserKey(id=3), UserKey(id=3)))
    println(UserKey(id=3) != UserKey(id=4))
    println(before(Score(points=1), Score(points=2)))
    println(Score(points=3) > Score(points=2))
    println(Score(points=2) >= Score(points=2))
    println(Score(points=2) <= Score(points=1))
    println(Score(points=2).__ge__(Score(points=3)))
    println(lowest([Score(points=3), Score(points=1), Score(points=2)]).points)
    println(sorted([Reversed(value=1), Reversed(value=3)])[0].value)
    println(Reversed(value=1) <= Reversed(value=0))
"#,
    )?;
    assert_eq!(stdout, "2\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse\nfalse\n1\n3\ntrue\n");
    Ok(())
}

/// Through a type parameter's builtin bound, `a.__eq__(b)`, `a.__ne__(b)` and the ordering dunders are the comparison
/// operators, which reach an adopter's own dunders, and `value.__str__()` is the value's display text.
#[test]
fn dunders_through_builtin_bounds_are_their_operations_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
from std.derives.comparison import Eq, Ord


model Reversed with Ord:
    value: int

    def __eq__(self, other: Self) -> bool:
        return self.value == other.value

    def __lt__(self, other: Self) -> bool:
        return self.value > other.value


@derive(Display)
model Point:
    x: int


def differ[T with Eq](a: T, b: T) -> bool:
    return a.__ne__(b)


def equal[T with Eq](a: T, b: T) -> bool:
    return a.__eq__(b)


def less[T with Ord](a: T, b: T) -> bool:
    return a.__lt__(b)


def at_most[T with Ord](a: T, b: T) -> bool:
    return a.__le__(b)


def text[T with Display](value: T) -> str:
    return value.__str__()


def main() -> None:
    println(differ(1, 2))
    println(equal("a", "a"))
    println(less(1, 2))
    println(at_most(2, 2))
    println(less(Reversed(value=2), Reversed(value=1)))
    println(text(3))
    println(text(Point(x=1)))
"#,
    )?;
    assert_eq!(stdout, "true\ntrue\ntrue\ntrue\ntrue\n3\nPoint { x: 1 }\n");
    Ok(())
}

/// In a module checked under its module path, as every module of a project build is, the dunders an `Eq` or `Ord`
/// adopter gets from the trait are callable under every spelling of the trait (an import alias, its module) and every
/// kind of adopter, and a bound on a builtin trait the module never imports is the builtin: `value.__str__()` through
/// `T with Display` is the display text and the comparison dunders through `T with Eq` and `T with Ord` are the
/// operators. The adopter of `comparison.Ord` had no projection of the defaults it calls, and the builtin bound took
/// the checked module as the trait's owner, so the call recorded no dispatch; both failed E0599 in rustc.
#[test]
fn comparison_dunders_through_every_spelling_build_in_a_project_module_issue1561() -> TestResult {
    let stdout = run_project_module_with_stdlib(
        r#"
from std.derives import comparison
from std.derives.comparison import Ord as Ordered


model Aliased with Ordered:
    v: int

    def __eq__(self, other: Self) -> bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> bool:
        return self.v < other.v


model Qualified with comparison.Ord:
    v: int

    def __eq__(self, other: Self) -> bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> bool:
        return self.v < other.v


enum Level with Ordered:
    Low
    High

    def rank(self) -> int:
        match self:
            Level.Low => return 0
            Level.High => return 1

    def __eq__(self, other: Self) -> bool:
        return self.rank() == other.rank()

    def __lt__(self, other: Self) -> bool:
        return self.rank() < other.rank()


type Rank = newtype int with comparison.Ord:
    def __eq__(self, other: Self) -> bool:
        return self.0 == other.0

    def __lt__(self, other: Self) -> bool:
        return self.0 < other.0


class Account with comparison.Eq:
    pub id: int

    def __eq__(self, other: Self) -> bool:
        return self.id == other.id


@derive(Display)
model Point:
    x: int


def text[T with Display](value: T) -> str:
    return value.__str__()


def differ[T with Eq](a: T, b: T) -> bool:
    return a.__ne__(b)


def less[T with Ord](a: T, b: T) -> bool:
    return a.__lt__(b)


def main() -> None:
    println(Aliased(v=2).__ge__(Aliased(v=1)))
    println(Qualified(v=2).__le__(Qualified(v=1)))
    println(Level.High.__gt__(Level.Low))
    println(Rank(1).__ge__(Rank(2)))
    println(Account(id=1).__ne__(Account(id=2)))
    println(text(3))
    println(text(Point(x=1)))
    println(differ(1, 2))
    println(less("a", "b"))
"#,
    )?;
    assert_eq!(
        stdout,
        "true\nfalse\ntrue\nfalse\ntrue\n3\nPoint { x: 1 }\ntrue\ntrue\n"
    );
    Ok(())
}

/// A trait default that passes `self` to a method taking the trait's `Self`, and a method reading a `Self` parameter's
/// field, build: inside the adopter's impl, `Self` is the adopter.
#[test]
fn trait_default_passes_self_to_a_method_taking_self_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
trait Ranked:
    def less(self, other: Self) -> bool: ...

    def more(self, other: Self) -> bool:
        return other.less(self)


model Level with Ranked:
    rank: int

    def less(self, other: Self) -> bool:
        return self.rank < other.rank


def main() -> None:
    println(Level(rank=2).more(Level(rank=1)))
    println(Level(rank=1).more(Level(rank=2)))
"#,
    )?;
    assert_eq!(stdout, "true\nfalse\n");
    Ok(())
}

/// Enum, newtype, class and generic adopters, the trait imported under an alias or named through its module, a
/// user trait that declares `__eq__`, and a dunder that calls a method on its `Self` parameter all build and compare.
#[test]
fn comparison_adopters_of_every_kind_and_spelling_build_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
from std.derives import comparison
from std.derives.comparison import Eq, Ord
from std.derives.comparison import Ord as Ordered


enum Level with Ord:
    Low
    High

    def rank(self) -> int:
        match self:
            Level.Low => return 0
            Level.High => return 1

    def __eq__(self, other: Self) -> bool:
        return self.rank() == other.rank()

    def __lt__(self, other: Self) -> bool:
        return self.rank() < other.rank()


type Rank = newtype int with Ord:
    def __eq__(self, other: Self) -> bool:
        return self.0 == other.0

    def __lt__(self, other: Self) -> bool:
        return self.0 < other.0


class Account with Eq:
    pub id: int
    pub note: str

    def __eq__(self, other: Self) -> bool:
        return self.id == other.id


model Boxed[T with (Ord, Clone)] with Ord:
    value: T

    def __eq__(self, other: Self) -> bool:
        return self.value == other.value

    def __lt__(self, other: Self) -> bool:
        return self.value < other.value


model Aliased with Ordered:
    points: int

    def __eq__(self, other: Self) -> bool:
        return self.points == other.points

    def __lt__(self, other: Self) -> bool:
        return self.points < other.points


model Qualified with comparison.Ord:
    points: int

    def __eq__(self, other: Self) -> bool:
        return self.points == other.points

    def __lt__(self, other: Self) -> bool:
        return self.points < other.points


trait Same:
    def __eq__(self, other: Self) -> bool: ...

    def differs(self, other: Self) -> bool:
        return not self.__eq__(other)


model Tag with Same:
    name: str

    def __eq__(self, other: Self) -> bool:
        return self.name == other.name


def same[T with Eq](a: T, b: T) -> bool:
    return a == b


def top[T with Ordered](a: T, b: T) -> bool:
    return a > b


def main() -> None:
    println(Level.Low < Level.High)
    println(sorted([Level.High, Level.Low])[0].rank())
    println(Rank(1) < Rank(2))
    println(sorted([Rank(5), Rank(3)])[0].0)
    println(same(Account(id=1, note="a"), Account(id=1, note="b")))
    println(Boxed(value=1) < Boxed(value=2))
    println(top(Aliased(points=3), Aliased(points=2)))
    println(sorted([Qualified(points=4), Qualified(points=2)])[0].points)
    println(Tag(name="a").differs(Tag(name="b")))
"#,
    )?;
    assert_eq!(stdout, "true\n0\ntrue\n3\ntrue\ntrue\ntrue\n2\ntrue\n");
    Ok(())
}

/// An `Ord` adopter that imports only `Ord`, spelled as imported or through its module, and an enum adopter reach the
/// `__ne__` default of the `Eq` supertrait, which is `not __eq__`; so does a type parameter bounded by `Ord` (#1561).
#[test]
fn ne_reaches_the_eq_default_of_an_ord_only_adopter_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
from std.derives.comparison import Ord
from std.derives import comparison


model Ordered with Ord:
    v: int

    def __eq__(self, other: Self) -> bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> bool:
        return self.v < other.v


model Qualified with comparison.Ord:
    v: int

    def __eq__(self, other: Self) -> bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> bool:
        return self.v < other.v


enum Level with Ord:
    Low
    High

    def __eq__(self, other: Self) -> bool:
        return self.rank() == other.rank()

    def __lt__(self, other: Self) -> bool:
        return self.rank() < other.rank()

    def rank(self) -> int:
        match self:
            Level.Low => return 0
            Level.High => return 1


def differ[T with Ord](a: T, b: T) -> bool:
    return a.__ne__(b)


def main() -> None:
    println(Ordered(v=1).__ne__(Ordered(v=2)))
    println(Ordered(v=1).__ne__(Ordered(v=1)))
    println(Qualified(v=1).__ne__(Qualified(v=1)))
    println(Level.Low.__ne__(Level.High))
    println(differ(Ordered(v=3), Ordered(v=3)))
"#,
    )?;
    assert_eq!(stdout, "true\nfalse\nfalse\ntrue\nfalse\n");
    Ok(())
}

/// A trait's default method that calls a method the trait or its supertrait declares, on `self` or on another `Self`
/// value, uses its declared result: `self.tag() + "!"`, `len(self.tag())`, a method chained on the result, and a
/// generic trait's method returning its type parameter build and run (#1561).
#[test]
fn trait_default_calls_on_self_take_the_declared_signature_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
trait Named:
    def name(self) -> str: ...


trait Tag with Named:
    def tag(self) -> str: ...

    def loud(self) -> str:
        return self.tag() + "!"

    def size(self) -> int:
        return len(self.tag()) + len(self.name())

    def greet(self, other: Self) -> str:
        return "hi " + other.name().upper()


trait Holder[T]:
    def get(self) -> T: ...

    def pair(self) -> list[T]:
        return [self.get(), self.get()]


model Label with Tag:
    text: str

    def name(self) -> str:
        return "label"

    def tag(self) -> str:
        return self.text


model Box with Holder[int]:
    value: int

    def get(self) -> int:
        return self.value


def main() -> None:
    label = Label(text="x")
    println(label.loud())
    println(label.size())
    println(label.greet(Label(text="y")))
    println(Box(value=4).pair())
"#,
    )?;
    assert_eq!(stdout, "x!\n6\nhi LABEL\n[4, 4]\n");
    Ok(())
}

/// A type imported from another module that adopts `std.derives.comparison.Ord` or `Eq`, under any spelling, gets the
/// dunders of the trait in the importing module, which need not import the trait: `__ge__`, `__le__`, `__gt__` and
/// `__ne__` compare as its operators do, and an adopter's own `__ge__` override is the one `__ge__` reaches. A type
/// imported from another module that derives `Ord` compares the same way.
#[test]
fn comparison_dunders_of_an_adopter_from_another_module_build_issue1561() -> TestResult {
    let stdout = run_modules_with_stdlib(
        &[(
            "scores",
            r#"
from std.derives import comparison
from std.derives.comparison import Eq, Ord
from std.derives.comparison import Ord as Ordered


pub model Score with Ord:
    pub points: int

    def __eq__(self, other: Self) -> bool:
        return self.points == other.points

    def __lt__(self, other: Self) -> bool:
        return self.points < other.points


pub enum Level with Ordered:
    Low
    High

    def rank(self) -> int:
        match self:
            Level.Low => return 0
            Level.High => return 1

    def __eq__(self, other: Self) -> bool:
        return self.rank() == other.rank()

    def __lt__(self, other: Self) -> bool:
        return self.rank() < other.rank()


pub model Qualified with comparison.Ord:
    pub v: int

    def __eq__(self, other: Self) -> bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> bool:
        return self.v < other.v


pub model Reversed with Ord:
    pub v: int

    def __eq__(self, other: Self) -> bool:
        return self.v == other.v

    def __lt__(self, other: Self) -> bool:
        return self.v > other.v

    def __ge__(self, other: Self) -> bool:
        return false


pub model Key with Eq:
    pub id: int

    def __eq__(self, other: Self) -> bool:
        return self.id == other.id


@derive(Eq, Ord)
pub model Derived:
    pub n: int
"#,
        )],
        r#"
from scores import Score, Level, Qualified, Reversed, Key, Derived


def main() -> None:
    println(Score(points=2).__ge__(Score(points=3)))
    println(Score(points=2).__le__(Score(points=3)))
    println(Score(points=2).__gt__(Score(points=3)))
    println(Score(points=2).__ne__(Score(points=3)))
    println(Level.High.__gt__(Level.Low))
    println(Qualified(v=2).__le__(Qualified(v=1)))
    println(Reversed(v=1).__ge__(Reversed(v=0)))
    println(Reversed(v=1).__le__(Reversed(v=0)))
    println(Key(id=1).__ne__(Key(id=2)))
    println(Derived(n=1).__ge__(Derived(n=2)))
"#,
    )?;
    assert_eq!(
        stdout,
        "false\ntrue\nfalse\ntrue\ntrue\nfalse\nfalse\ntrue\ntrue\nfalse\n"
    );
    Ok(())
}

/// A type that derives `Ord` implements Rust's ordering traits from the derive, and has no source `Ord` impl, so each
/// ordering dunder called on it is the operator it defines: `a.__ge__(b)` is `a >= b`, for a model, an enum, a newtype
/// and a generic model, on a value and on `self` in a method. `__ne__` and `__eq__` are `!=` and `==`.
#[test]
fn dunders_of_a_derived_ord_are_its_operators_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
@derive(Eq, Ord)
model Score:
    points: int

    def at_least(self, other: Score) -> bool:
        return self.__ge__(other)


@derive(Eq, Ord)
enum Level:
    Low
    High


@derive(Eq, Ord)
model Pair[T]:
    left: T
    right: T


@derive(Eq, Ord)
type Rank = newtype int


def main() -> None:
    a = Score(points=2)
    b = Score(points=3)
    println(a.__ge__(b))
    println(a.__le__(b))
    println(a.__gt__(b))
    println(a.__lt__(b))
    println(a.__ne__(b))
    println(a.__eq__(b))
    println(a.__ge__(b) == (a >= b))
    println(b.at_least(a))
    println(Level.High.__gt__(Level.Low))
    println(Pair(left=1, right=2).__lt__(Pair(left=1, right=3)))
    println(Rank(1).__ge__(Rank(2)))
"#,
    )?;
    assert_eq!(
        stdout,
        "false\ntrue\nfalse\ntrue\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue\nfalse\n"
    );
    Ok(())
}
