//! The Rust spellings emission gives checked programs whose names or reads the frozen emitter used to spell wrongly:
//! identifiers that are Rust keywords (#1775), a program type named like a runtime surface type (#1770), `set()` over
//! a generator (#1744) and the reads of a `const` `FrozenDict` (#1757).

use crate::codegen::IrCodegen;
use incan_frontend::{lexer, parser};

/// Lex, parse, check and generate Rust for one program.
fn generate(source: &str) -> Result<String, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| format!("generation failed: {error:?}"))
}

/// Remove whitespace so assertions do not depend on the pretty-printer's line breaks.
fn compact(rust: &str) -> String {
    rust.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// #1775: an identifier that is a Rust keyword is spelled as a raw identifier in every position the generated Rust
/// names it (model and class fields, parameters, constructor arguments, enum variants, type parameters), while
/// reflection keeps the source spelling. Methods are emitted under their encoded identities, so their source names
/// never reach a Rust identifier position.
#[test]
fn host_keyword_identifiers_are_raw_in_every_position_issue1775() -> Result<(), String> {
    let rust = generate(
        r#"
model Slot:
    impl: int
    dyn: str = "d"

    def move(self, ref: int) -> int:
        return self.impl + ref


class Holder:
    use: int

    def where(self) -> int:
        return self.use


enum Mode:
    Loop
    unsafe


def pick[mod](value: mod) -> mod:
    return value


def main() -> None:
    slot = Slot(impl=1, dyn="x")
    println(slot.move(ref=2))
    println(Holder(use=5).where())
    match Mode.unsafe:
        case Mode.unsafe:
            println("unsafe")
        case Mode.Loop:
            println("loop")
    println(pick(6))
    for info in slot.__fields__():
        println(info.name)
"#,
    )?;
    let flat = compact(&rust);
    for spelling in [
        "r#impl:i64,r#dyn:String",
        "r#ref:i64",
        "self.r#impl+r#ref",
        "Slot{r#impl:1,r#dyn:",
        "Holder{r#use:5}",
        "self.r#use",
        "Loop,r#unsafe,",
        "Mode::r#unsafe=>",
        "(value:r#mod)->r#mod",
    ] {
        assert!(flat.contains(spelling), "missing `{spelling}` in:\n{rust}");
    }
    assert!(
        rust.contains("FrozenStr::new(\"impl\")"),
        "reflection reports the source spelling:\n{rust}"
    );
    assert!(
        !rust.contains("\"r#impl\""),
        "no raw spelling leaks into a string:\n{rust}"
    );
    Ok(())
}

/// #1770: a program's own `model FieldInfo` is constructed and named as that model, not as the runtime reflection
/// record, while the reflection records the program's `__fields__()` returns keep the runtime spelling.
#[test]
fn program_type_named_like_a_runtime_surface_type_keeps_its_own_spelling_issue1770() -> Result<(), String> {
    let rust = generate(
        r#"
model FieldInfo:
    label: str


def describe(info: FieldInfo) -> str:
    return info.label


def main() -> None:
    println(describe(FieldInfo(label="custom")))
"#,
    )?;
    let flat = compact(&rust);
    assert!(
        flat.contains("FieldInfo{label:"),
        "the constructor names the program's model:\n{rust}"
    );
    assert!(
        !flat.contains("incan_std_core::reflection::FieldInfo{label"),
        "the constructor must not name the runtime record:\n{rust}"
    );
    assert!(
        flat.contains("info:FieldInfo"),
        "a parameter annotation names the program's model:\n{rust}"
    );
    assert!(
        flat.contains("FrozenList<incan_std_core::reflection::FieldInfo>"),
        "the model's own reflection still returns runtime records:\n{rust}"
    );
    Ok(())
}

/// #1744: `set(generator)` collects through the `Iterator` trait, named in full, as `list(generator)` does.
#[test]
fn set_constructor_over_a_generator_collects_through_the_iterator_trait_issue1744() -> Result<(), String> {
    let rust = generate(
        r#"
def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value


def main() -> None:
    println(len(set(numbers(3))))
"#,
    )?;
    let flat = compact(&rust);
    assert!(
        flat.contains("::std::iter::Iterator::collect::<std::collections::HashSet<_>>("),
        "missing the trait-qualified collect:\n{rust}"
    );
    assert!(
        !flat.contains(".into_iter().collect::<std::collections::HashSet<_>>()"),
        "the generator must not reach the method-call collect:\n{rust}"
    );
    Ok(())
}

/// #1757: a `const` `FrozenDict` lookup goes through the raising frozen lookup with a `str` probe, membership is
/// `contains_key` with the same probe, and a comprehension over a `const` `FrozenList[str]` iterates owned strings.
#[test]
fn const_frozen_dict_reads_use_the_frozen_lookup_issue1757() -> Result<(), String> {
    let rust = generate(
        r#"
const TABLE: FrozenDict[str, FrozenList[str]] = {"names": ["alpha", "beta"], "empty": []}
const NAMES: FrozenList[str] = ["alpha", "beta"]


def main() -> None:
    key = "names"
    names = TABLE[key]
    println(len(names))
    println(TABLE.contains_key("names"))
    println(" ".join([name for name in NAMES]))
"#,
    )?;
    let flat = compact(&rust);
    assert!(
        flat.contains("incan_std_core::collections::frozen_dict_get(&TABLE,<_asAsRef<str>>::as_ref(&key)"),
        "missing the frozen lookup:\n{rust}"
    );
    assert!(
        flat.contains("TABLE.contains_key(<_asAsRef<str>>::as_ref(&\"names\"))"),
        "missing the keyed membership:\n{rust}"
    );
    assert!(
        flat.contains("(NAMES).iter().map(|__incan_item|__incan_item.to_string())"),
        "the comprehension source materializes owned strings:\n{rust}"
    );
    assert!(
        !flat.contains("TABLE[key]"),
        "no Rust indexing on the frozen dict:\n{rust}"
    );
    Ok(())
}
