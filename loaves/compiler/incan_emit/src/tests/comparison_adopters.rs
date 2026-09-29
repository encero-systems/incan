//! Types that adopt `std.derives.comparison.Eq` or `Ord` and define their dunders build and compare (#1561): each
//! comparison operator, `sorted(values)`, a set element with `@derive(Hash)` and the `T with Eq` and `T with Ord`
//! bounds reach the type's own `__eq__` and `__lt__`, and the defaults `Ord` supplies for the rest, or the type's own
//! override of one. A dunder called through a builtin bound is the operation it defines, `__ne__` reaches the `Eq`
//! default of an adopter that imports only `Ord`, a trait default passes `self` to a method that takes its own type by
//! value, and a trait default's call of a method its trait declares has the declared result.

use super::generated_programs::run_with_stdlib;

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
