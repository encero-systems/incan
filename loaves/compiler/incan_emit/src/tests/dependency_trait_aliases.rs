//! A `pub::` dependency's trait under another spelling (#1561): imported under an alias (`Tag as Tagged`) or exported
//! under one (`pub Tagging = Tag`), the trait is the dependency's own, so the dependency's adopters meet a bound on it
//! and a consumer's type adopts it with its own methods; so it is when the consumer names it through a module binding
//! (`t.Tag`), and a consumer's type adopting it under one spelling meets a bound on another. A default method of the
//! trait reaches no adopter in another package, so an adopter that relies on one is refused.

use super::generated_programs::{generate_consumer, run_with_dependency};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The `tags` dependency: a trait with a required method and a default, exported again under an alias, and a model
/// adopting it.
const TAGS: &str = r#"
pub trait Tag:
    def tag(self) -> str: ...

    def loud(self) -> str:
        return f"{self.tag()}!"


pub Tagging = Tag


pub model Label with Tag:
    pub name: str

    def tag(self) -> str:
        return self.name
"#;

/// The dependency's `Label` meets a bound on the trait imported under an alias and on the dependency's own alias of
/// it, and a consumer's model adopts the trait under either spelling, defining each of its methods.
#[test]
fn dependency_trait_under_another_spelling_bounds_and_adopts_issue1561() -> TestResult {
    let consumer = r#"
from pub::tags import Label, Tag as Tagged, Tagging


def shout[T with Tagged](value: T) -> str:
    return value.loud()


def name_of[T with Tagging](value: T) -> str:
    return value.tag()


model Mine with Tagged:
    id: int

    def tag(self) -> str:
        return "mine"

    def loud(self) -> str:
        return "MINE"


model Yours with Tagging:
    id: int

    def tag(self) -> str:
        return "yours"

    def loud(self) -> str:
        return "YOURS"


def main() -> None:
    println(shout(Label(name="dep")))
    println(name_of(Label(name="dep")))
    println(shout(Mine(id=1)))
    println(name_of(Yours(id=2)))
"#;
    assert_eq!(run_with_dependency("tags", TAGS, consumer)?, "dep!\ndep\nMINE\nyours\n");
    Ok(())
}

/// The dependency's trait named through a module binding (`import pub::tags as t`, then `t.Tag` and the package alias
/// `t.Tagging`) is the dependency's own trait: the dependency's `Label`, reached as `t.Label`, meets a bound on either
/// spelling, and a consumer's model adopts it under either with its own methods (#1561).
#[test]
fn dependency_trait_through_a_module_binding_bounds_and_adopts_issue1561() -> TestResult {
    let consumer = r#"
import pub::tags as t


def shout[T with t.Tag](value: T) -> str:
    return value.loud()


def name_of[T with t.Tagging](value: T) -> str:
    return value.tag()


model Mine with t.Tag:
    id: int

    def tag(self) -> str:
        return "mine"

    def loud(self) -> str:
        return "MINE"


model Yours with t.Tagging:
    id: int

    def tag(self) -> str:
        return "yours"

    def loud(self) -> str:
        return "YOURS"


def main() -> None:
    println(shout(t.Label(name="dep")))
    println(name_of(t.Label(name="dep")))
    println(shout(Mine(id=1)))
    println(name_of(Yours(id=2)))
"#;
    assert_eq!(run_with_dependency("tags", TAGS, consumer)?, "dep!\ndep\nMINE\nyours\n");
    Ok(())
}

/// A consumer's type that adopts the dependency's trait under one spelling meets a bound that names it under another:
/// the import alias `Tagged`, the dependency's alias `Tagging`, the trait itself and its module-bound `t.Tag` are one
/// trait (#1561).
#[test]
fn consumer_adopter_meets_a_bound_on_another_spelling_of_the_dependency_trait_issue1561() -> TestResult {
    let consumer = r#"
import pub::tags as t
from pub::tags import Tag, Tagging
from pub::tags import Tag as Tagged


def name_of[T with Tagging](value: T) -> str:
    return value.tag()


def shout[T with Tagged](value: T) -> str:
    return value.loud()


def quiet[T with t.Tag](value: T) -> str:
    return value.tag()


model Mine with Tagged:
    id: int

    def tag(self) -> str:
        return "mine"

    def loud(self) -> str:
        return "MINE"


model Yours with Tagging:
    id: int

    def tag(self) -> str:
        return "yours"

    def loud(self) -> str:
        return "YOURS"


model Ours with t.Tagging:
    id: int

    def tag(self) -> str:
        return "ours"

    def loud(self) -> str:
        return "OURS"


model Theirs with Tag:
    id: int

    def tag(self) -> str:
        return "theirs"

    def loud(self) -> str:
        return "THEIRS"


def main() -> None:
    println(name_of(Mine(id=1)))
    println(shout(Yours(id=2)))
    println(quiet(Mine(id=1)))
    println(shout(Ours(id=3)))
    println(name_of(Theirs(id=4)))
    println(quiet(Theirs(id=4)))
"#;
    assert_eq!(
        run_with_dependency("tags", TAGS, consumer)?,
        "mine\nYOURS\nmine\nOURS\ntheirs\ntheirs\n"
    );
    Ok(())
}

/// A consumer's model that adopts the dependency's trait without defining its default method is refused: the default
/// body is not published with the package.
#[test]
fn adopter_relying_on_a_dependency_trait_default_is_refused_issue1561() -> TestResult {
    let consumer = r#"
from pub::tags import Tag


model Mine with Tag:
    id: int

    def tag(self) -> str:
        return "mine"


def main() -> None:
    println(Mine(id=1).loud())
"#;
    let error = match generate_consumer("tags", TAGS, consumer) {
        Ok(_) => return Err("an adopter relying on a dependency trait default must be refused".into()),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("'Mine' adopts 'Tag' from another package and does not define its default method 'loud'"),
        "{error}"
    );
    Ok(())
}
