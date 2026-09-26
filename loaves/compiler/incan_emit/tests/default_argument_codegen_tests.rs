//! Generated-Rust spelling of default arguments expanded outside their callable's module (#1771).
//!
//! Lowering spells each name a default reads through the module that declares the name, so these tests generate a
//! whole project and read the call sites the defaults are expanded at, with dependency items pruned to the reachable
//! ones as a run prunes them.

use std::collections::HashMap;

use incan_emit::IrCodegen;
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
    materialize_api_alias_projections, materialize_checked_api_public_namespaces,
};
use incan_frontend::library_exports::collect_checked_public_exports;
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Parse one source module.
fn parse(source: &str) -> Result<incan_frontend::ast::Program, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    Ok(parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?)
}

/// Remove whitespace so assertions do not depend on the pretty-printer's line breaks.
fn compact(code: &str) -> String {
    code.chars().filter(|character| !character.is_whitespace()).collect()
}

/// Generated Rust of a project: the crate root's code and each module's code by module path.
type GeneratedProject = (String, HashMap<Vec<String>, String>);

/// Generate `main` with the given top-level modules, pruning each module to the items `main` reaches.
fn generate(main: &str, modules: &[(&'static str, &str)]) -> Result<GeneratedProject, Box<dyn std::error::Error>> {
    let main = parse(main)?;
    let programs = modules
        .iter()
        .map(|(name, source)| Ok((*name, parse(source)?)))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(false);
    for (name, program) in &programs {
        codegen.add_module_with_path_segments(name, program, vec![(*name).to_string()]);
    }
    let paths = programs
        .iter()
        .map(|(name, _)| vec![(*name).to_string()])
        .collect::<Vec<_>>();
    Ok(codegen.try_generate_multi_file_nested(&main, &paths)?)
}

/// Return the generated Rust of one top-level module.
fn module_code<'a>(modules: &'a HashMap<Vec<String>, String>, name: &str) -> Result<&'a str, String> {
    modules
        .get(&vec![name.to_string()])
        .map(String::as_str)
        .ok_or_else(|| format!("module `{name}` was not generated"))
}

/// #1771: `a` and `b` each declare `const SIZE` and a function defaulting to it. A caller importing both expands each
/// default outside its module, where `SIZE` alone names neither; each default is spelled through the module that
/// declares its const, so `first()` passes `a`'s `SIZE` and `second()` passes `b`'s, and both consts are kept.
#[test]
fn same_named_default_consts_resolve_in_their_callables_modules_issue1771() -> TestResult {
    let (main_code, modules) = generate(
        "from a import first\nfrom b import second\n\ndef main() -> None:\n    println(first())\n    println(second())\n",
        &[
            (
                "a",
                "const SIZE: int = 1\n\npub def first(n: int = SIZE) -> int:\n    return n\n",
            ),
            (
                "b",
                "const SIZE: int = 2\n\npub def second(n: int = SIZE) -> int:\n    return n\n",
            ),
        ],
    )?;
    let code = compact(&main_code);
    assert!(
        code.contains("(crate::a::SIZE)"),
        "`first()` takes `a`'s SIZE:\n{main_code}"
    );
    assert!(
        code.contains("(crate::b::SIZE)"),
        "`second()` takes `b`'s SIZE:\n{main_code}"
    );
    for name in ["a", "b"] {
        assert!(
            compact(module_code(&modules, name)?).contains("constSIZE:i64"),
            "`{name}` keeps the SIZE a default reaches"
        );
    }
    Ok(())
}

/// #1771: `a` imports `SIZE` from `c` and defaults to it, while `b` declares its own `SIZE`. The imported name means
/// the const `c` declares, so `a_size()` passes `c`'s `SIZE` wherever it is called.
#[test]
fn imported_default_const_resolves_to_its_declaring_module_issue1771() -> TestResult {
    let (main_code, modules) = generate(
        "from a import a_size\nfrom b import b_size\n\ndef main() -> None:\n    println(a_size())\n    println(b_size())\n",
        &[
            ("c", "pub const SIZE: int = 3\n"),
            (
                "a",
                "from c import SIZE\n\npub def a_size(n: int = SIZE) -> int:\n    return n\n",
            ),
            (
                "b",
                "const SIZE: int = 2\n\npub def b_size(n: int = SIZE) -> int:\n    return n\n",
            ),
        ],
    )?;
    let code = compact(&main_code);
    assert!(
        code.contains("(crate::c::SIZE)"),
        "`a_size()` takes the SIZE `c` declares:\n{main_code}"
    );
    assert!(
        code.contains("(crate::b::SIZE)"),
        "`b_size()` takes `b`'s own SIZE:\n{main_code}"
    );
    assert!(
        compact(module_code(&modules, "c")?).contains("constSIZE:i64"),
        "`c` keeps the SIZE `a`'s default reaches"
    );
    Ok(())
}

/// #1771: `main` declares its own `SIZE` and defaults to it, while the SIZE `a`'s default reads is `c`'s. Each
/// default keeps the const its own module means: `a_size()` passes `c`'s, `local()` passes `main`'s from the crate
/// root.
#[test]
fn default_consts_of_the_calling_module_and_a_dependency_stay_apart_issue1771() -> TestResult {
    let (main_code, _) = generate(
        "from a import a_size\n\nconst SIZE: int = 9\n\ndef local(n: int = SIZE) -> int:\n    return n\n\n\
         def main() -> None:\n    println(a_size())\n    println(local())\n",
        &[
            ("c", "pub const SIZE: int = 3\n"),
            (
                "a",
                "from c import SIZE\n\npub def a_size(n: int = SIZE) -> int:\n    return n\n",
            ),
        ],
    )?;
    let code = compact(&main_code);
    assert!(
        code.contains("(crate::c::SIZE)"),
        "`a_size()` takes the SIZE `c` declares:\n{main_code}"
    );
    assert!(
        code.contains("(crate::SIZE)"),
        "`local()` takes `main`'s own SIZE:\n{main_code}"
    );
    Ok(())
}

/// #1771: a default in `b` constructs the public model `Card` that `a` declares with a private field, directly and
/// through a method partial's preset. `main` imports neither `Card` nor `a`, so the construction is spelled there as a
/// literal of every field through `a`'s path, and `a` makes the private field reachable within the crate, not beyond.
#[test]
fn default_constructing_another_modules_type_reaches_its_fields_within_the_crate_issue1771() -> TestResult {
    let (main_code, modules) = generate(
        "from b import Deck, show\n\ndef main() -> None:\n    println(show())\n    println(Deck().top())\n",
        &[
            (
                "a",
                "pub model Card:\n    size: int = 4\n    pub label: str = \"x\"\n\n    def area(self) -> int:\n        return self.size\n",
            ),
            (
                "b",
                r#"
from a import Card


pub def show(card: Card = Card(label="y")) -> int:
    return card.area()


pub model Deck:
    pub n: int = 1

    def deal(self, card: Card) -> int:
        return card.area() + self.n

    top = partial deal(card=Card(label="ace"))
"#,
            ),
        ],
    )?;
    let code = compact(&main_code);
    for literal in [
        "crate::a::Card{size:4,label:\"y\".to_string()}",
        "crate::a::Card{size:4,label:\"ace\".to_string()}",
    ] {
        assert!(
            code.contains(literal),
            "the construction is spelled through `a`:\n{main_code}"
        );
    }
    let a_code = compact(module_code(&modules, "a")?);
    assert!(
        a_code.contains("pub(crate)size:i64") && a_code.contains("publabel:String"),
        "the private field is reachable within the crate and the public one keeps its visibility:\n{a_code}"
    );
    Ok(())
}

/// #1771: consts that only the defaults of functions nothing calls or imports name are not kept, since those defaults
/// are never expanded.
#[test]
fn defaults_of_uncalled_functions_keep_nothing_issue1771() -> TestResult {
    let (_, modules) = generate(
        "from a import used\n\ndef main() -> None:\n    println(used())\n",
        &[(
            "a",
            "const SIZE: int = 1\nconst OTHER: int = 2\n\n\ndef unused_default(n: int = SIZE) -> int:\n    return n\n\n\n\
             pub def unused_pub(n: int = OTHER) -> int:\n    return n\n\n\npub def used() -> int:\n    return 3\n",
        )],
    )?;
    let a_code = compact(module_code(&modules, "a")?);
    for name in ["SIZE", "OTHER"] {
        assert!(
            !a_code.contains(&format!("const{name}:")),
            "`{name}` is named only by a default nothing expands:\n{a_code}"
        );
    }
    Ok(())
}

/// The dependency's library name, as a consumer imports it through `pub::`.
const LIBRARY: &str = "shapes";

/// Check a dependency's root module and publish its manifest the way a library build does.
fn provider_index(source: &str) -> Result<LibraryManifestIndex, Box<dyn std::error::Error>> {
    let program = parse(source)?;
    let module_path = vec!["lib".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("provider failed to check: {errors:?}"))?;
    let exports = collect_checked_public_exports(&program, &checker);
    let mut api_modules = vec![collect_checked_api_metadata(&program, &checker, module_path.clone())];
    materialize_api_alias_projections(&mut api_modules);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: api_modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api).map_err(|error| format!("{error:?}"))?;
    let mut manifest = LibraryManifest::from_checked_exports(LIBRARY, "0.1.0", &exports);
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(LIBRARY, &api, &[(module_path, exports.clone())])
        .map_err(|error| format!("{error:?}"))?;
    manifest.contract_metadata.api = Some(api);
    Ok(LibraryManifestIndex::from_entries(HashMap::from([(
        LIBRARY.to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                LIBRARY,
                LIBRARY,
                std::env::temp_dir().join("incan_default_argument_codegen_tests"),
            ),
        },
    )])))
}

/// Return the argument each `println` in generated Rust prints, whitespace removed.
fn printed_arguments(code: &str) -> Vec<String> {
    compact(code)
        .split("println!(\"{}\",")
        .skip(1)
        .filter_map(|rest| rest.split(");").next().map(str::to_string))
        .collect()
}

/// #1771: a dependency's default that constructs one of its public models or classes is carried to a consumer that
/// omits the argument, directly and through a partial over the function, including a model whose private field only
/// the dependency's constructor sets and a field argument naming a private `str` const. A partial's preset that
/// constructs one is carried the same way. The consumer imports neither type, so each construction reaches its type
/// through the dependency's path, and it is otherwise the construction the consumer would write itself.
#[test]
fn public_construction_default_reaches_a_consumer_through_the_dependency_issue1771() -> TestResult {
    let index = provider_index(
        r#"
pub model Card:
    size: int = 4
    pub label: str = "x"

    def area(self) -> int:
        return self.size


pub class Box:
    pub side: int = 3


pub def show(card: Card = Card(label="y")) -> int:
    return card.area()


pub def side(b: Box = Box()) -> int:
    return b.side


pub def stacked(count: int, b: Box = Box()) -> int:
    return count * b.side


pub stacked_twice = partial stacked(count=2)


const LABEL: str = "k"


pub def labeled(card: Card = Card(label=LABEL)) -> int:
    return card.area()


pub def tagged(tag: str, card: Card) -> str:
    return tag + card.label


pub tag_ace = partial tagged(card=Card(label="ace"))
"#,
    )?;
    let generate = |source: &str| -> Result<String, Box<dyn std::error::Error>> {
        let mut codegen = IrCodegen::new();
        codegen.set_library_manifest_index(index.clone());
        Ok(codegen.try_generate(&parse(source)?)?)
    };
    let omitted = generate(
        "from pub::shapes import labeled, show, side, stacked_twice, tag_ace\n\ndef main() -> None:\n    println(show())\n    println(side())\n    println(stacked_twice())\n    println(labeled())\n    println(tag_ace(tag=\"t:\"))\n",
    )?;
    let written = generate(
        "from pub::shapes import Box, Card, show, side, stacked_twice\n\ndef main() -> None:\n    println(show(Card(label=\"y\")))\n    println(side(Box()))\n    println(stacked_twice(b=Box()))\n",
    )?;
    let omitted_arguments = printed_arguments(&omitted);
    assert_eq!(
        omitted_arguments,
        vec![
            "shapes::show(shapes::Card(Some(\"y\".to_string())))".to_string(),
            "shapes::side(shapes::Box{side:3})".to_string(),
            "shapes::stacked_twice(2,shapes::Box{side:3})".to_string(),
            "shapes::labeled(shapes::Card(Some(shapes::LABEL.to_string())))".to_string(),
            "shapes::tag_ace(\"t:\".to_string(),shapes::Card(Some(\"ace\".to_string())))".to_string(),
        ],
        "each omitted default and preset constructs its type through the dependency:\n{omitted}"
    );
    let unqualified = omitted_arguments
        .iter()
        .take(3)
        .map(|argument| {
            argument
                .replace("(shapes::Card(", "(Card(")
                .replace("(shapes::Box{", "(Box{")
                .replace(",shapes::Box{", ",Box{")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        unqualified,
        printed_arguments(&written),
        "the construction is the one the consumer writes itself:\n{written}"
    );
    Ok(())
}
