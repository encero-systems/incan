//! A `pub::` dependency's trait under another spelling (#1561): imported under an alias (`Tag as Tagged`) or exported
//! under one (`pub Tagging = Tag`), the trait is the dependency's own, so the dependency's adopters meet a bound on it
//! and a consumer's type adopts it with its own methods. A default method of the trait reaches no adopter in another
//! package, so an adopter that relies on one is refused.

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
