//! A union member names the declaration its spelling resolves to where the union is written, and every module that
//! reaches that union through a signature, an alias or a narrowing builds the same crate-root wrapper (#1796).

use crate::IrCodegen;
use crate::test_support::parse_program_result;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Return `code` without whitespace, so assertions do not depend on how the Rust is laid out.
fn compact(code: &str) -> String {
    code.chars().filter(|character| !character.is_whitespace()).collect()
}

/// One module declaring `Product`, `Answer = Product | int`, and functions that read and build an `Answer`.
fn product_module(label: &str, field_type: &str) -> String {
    format!(
        "pub model Product:\n    pub value: {field_type}\n\n\npub type Answer = Product | int\n\n\npub def label(answer: Answer) -> str:\n    match answer:\n        Product(product) => return f\"{label} product {{product.value}}\"\n        int(number) => return f\"{label} number {{number}}\"\n\n\npub def wrap(value: {field_type}) -> Answer:\n    return Product(value=value)\n"
    )
}

/// The generated Rust of a package: its crate root and each module, keyed by module path.
struct PackageCode {
    root: String,
    modules: std::collections::HashMap<Vec<String>, String>,
}

impl PackageCode {
    /// Return one generated module's Rust without whitespace.
    fn module(&self, name: &str) -> Result<String, Box<dyn std::error::Error>> {
        self.modules
            .get(&vec![name.to_string()])
            .map(|code| compact(code))
            .ok_or_else(|| format!("no generated module `{name}`").into())
    }

    /// Return the name of the one crate-root wrapper that declares `variant`, such as `V0(crate::first::Product)`.
    fn wrapper_with_variant(&self, variant: &str) -> Result<String, Box<dyn std::error::Error>> {
        let root = compact(&self.root);
        let at = root
            .find(variant)
            .ok_or_else(|| format!("no wrapper declares `{variant}`:\n{}", self.root))?;
        let start = root[..at]
            .rfind("pubenum")
            .ok_or_else(|| format!("`{variant}` has no enum header:\n{}", self.root))?
            + "pubenum".len();
        let end = root[start..]
            .find('{')
            .ok_or_else(|| format!("`{variant}` has no enum body:\n{}", self.root))?;
        Ok(root[start..start + end].to_string())
    }

    /// Fail when any crate-root wrapper payload spells `payload` bare, a spelling the crate root cannot resolve.
    fn assert_no_payload(&self, payload: &str) -> TestResult {
        let root = compact(&self.root);
        if root.contains(&format!("V0({payload}),")) || root.contains(&format!("V1({payload}),")) {
            return Err(format!("a wrapper payload is spelled `{payload}`:\n{}", self.root).into());
        }
        Ok(())
    }
}

/// Generate one package of the given `(module, source)` modules under a crate root that declares only a constant.
fn generate_modules(modules: &[(&str, String)]) -> Result<PackageCode, Box<dyn std::error::Error>> {
    let parsed = modules
        .iter()
        .map(|(name, source)| parse_program_result(source).map(|ast| (*name, ast)))
        .collect::<Result<Vec<_>, _>>()?;
    let root = parse_program_result("pub const PRODUCER: str = \"producer\"\n")?;
    let paths = parsed
        .iter()
        .map(|(name, _)| vec![(*name).to_string()])
        .collect::<Vec<_>>();
    let mut codegen = IrCodegen::new();
    for ((name, ast), path) in parsed.iter().zip(&paths) {
        codegen.add_module_with_path_segments(name, ast, path.clone());
    }
    let (root, modules) = codegen.try_generate_multi_file_nested(&root, &paths)?;
    Ok(PackageCode { root, modules })
}

/// Generate a package of `first` (an `int` `Product`), `second` (a `str` `Product`) and `extra` modules.
fn generate_package(extra: &[(&str, &str)]) -> Result<PackageCode, Box<dyn std::error::Error>> {
    let mut modules = vec![
        ("first", product_module("first", "int")),
        ("second", product_module("second", "str")),
    ];
    modules.extend(extra.iter().map(|(name, source)| (*name, (*source).to_string())));
    generate_modules(&modules)
}

/// #1796: `if isinstance(value, Item): return value.item_field` over a local union alias narrows through the union's
/// wrapper. The subject's declared type named the alias, so narrowing did not see a union and emitted a `matches!`
/// test followed by a field read on the wrapper itself, which rustc refuses (E0609).
#[test]
fn isinstance_over_a_local_union_alias_narrows_through_the_wrapper_issue1796() -> TestResult {
    let program = parse_program_result(
        "model Item:\n    value: int\n\n\ntype Answer = Item | int\n\n\npub def read(answer: Answer) -> int:\n    if isinstance(answer, Item):\n        return answer.value\n    return 0\n\n\ndef main() -> None:\n    println(read(Item(value=1)))\n",
    )?;
    let code = IrCodegen::new().try_generate(&program)?;
    let compacted = compact(&code);
    assert!(
        !compacted.contains("matches!(answer"),
        "narrowing must not test the wrapper:\n{code}"
    );
    assert!(
        compacted.contains("::V0(answer)=>{returnanswer.value;}"),
        "the narrowed branch reads the field from the member payload:\n{code}"
    );
    Ok(())
}

/// #1796: an `is not None` test over a local optional alias narrows the same way as one over `Item | None`.
#[test]
fn none_test_over_a_local_optional_alias_narrows_to_the_payload_issue1796() -> TestResult {
    let program = parse_program_result(
        "model Item:\n    value: int\n\n\ntype MaybeItem = Item | None\n\n\npub def read(item: MaybeItem) -> int:\n    if item is not None:\n        return item.value\n    return 0\n\n\ndef main() -> None:\n    println(read(Item(value=1)))\n",
    )?;
    let code = IrCodegen::new().try_generate(&program)?;
    let compacted = compact(&code);
    assert!(
        compacted.contains("Some(item)=>{returnitem.value;}"),
        "the narrowed branch reads the field from the payload:\n{code}"
    );
    Ok(())
}

/// #1796: a module that imports both `first` and `second` and reaches their unions only through their functions uses
/// each module's own wrapper, including where its own `Product` binding names the other module's declaration.
///
/// Its view of the functions spelled the union members bare, so lowering built a third wrapper over a `Product` it
/// could not place, which the functions do not accept and whose payload does not resolve at the crate root.
#[test]
fn a_module_reaching_both_unions_through_their_functions_uses_each_modules_wrapper_issue1796() -> TestResult {
    let package = generate_package(&[(
        "user",
        "import first\nimport second\nfrom second import Product\n\n\npub def go(value: int) -> str:\n    return first.label(first.wrap(value))\n\n\npub def pick(product: Product) -> str:\n    return second.label(product)\n",
    )])?;
    package.assert_no_payload("Product")?;
    let second_wrapper = package.wrapper_with_variant("V0(crate::second::Product)")?;
    let user = package.module("user")?;
    assert!(
        user.contains(&format!("crate::{second_wrapper}::V0(product)")),
        "`pick` builds `second`'s wrapper:\n{user}"
    );
    Ok(())
}

/// #1796: a module that calls `first.label(2)` through `from first import label`, with no binding of its own for
/// `Product`, builds `first`'s wrapper, the one `label` accepts.
///
/// Its view of `label` spelled the union member bare and the module binds no `Product`, so lowering built a wrapper
/// over a bare `Product` that `label` does not accept and whose payload names neither module's declaration.
#[test]
fn a_union_reached_through_an_imported_function_uses_the_declaring_modules_wrapper_issue1796() -> TestResult {
    let package = generate_package(&[(
        "user",
        "from first import label\n\n\npub def go() -> str:\n    return label(2)\n",
    )])?;
    package.assert_no_payload("Product")?;
    let first_wrapper = package.wrapper_with_variant("V0(crate::first::Product)")?;
    let user = package.module("user")?;
    assert!(
        user.contains(&format!("crate::{first_wrapper}::V1(2)")),
        "`go` builds `first`'s wrapper `{first_wrapper}`:\n{user}"
    );
    Ok(())
}

/// #1796: `Holder[int] | str` written in two modules that each declare `Holder[T]` is one wrapper per module, each
/// carrying its own module's `Holder`.
///
/// A generic member kept its bare spelling, so both modules shared one wrapper whose `Holder<i64>` payload names
/// neither declaration at the crate root.
#[test]
fn a_union_over_a_generic_type_two_modules_declare_gets_one_wrapper_each_issue1796() -> TestResult {
    let holder = "pub model Holder[T]:\n    pub item: T\n\n\npub type Held = Holder[int] | str\n\n\npub def hold() -> Held:\n    return Holder(item=1)\n";
    let package = generate_modules(&[("third", holder.to_string()), ("fourth", holder.to_string())])?;
    package.assert_no_payload("Holder<i64>")?;
    let third_wrapper = package.wrapper_with_variant("V1(crate::third::Holder<i64>)")?;
    let fourth_wrapper = package.wrapper_with_variant("V1(crate::fourth::Holder<i64>)")?;
    assert_ne!(
        third_wrapper, fourth_wrapper,
        "each module's union needs its own wrapper"
    );
    for (module, own) in [("third", &third_wrapper), ("fourth", &fourth_wrapper)] {
        let code = package.module(module)?;
        assert!(
            code.contains(&format!("crate::{own}::V1(Holder{{item:1}})")),
            "`{module}` builds its own wrapper `{own}`:\n{code}"
        );
    }
    Ok(())
}

/// #1796: `return 3` into a union alias imported from `first`, by name or module-qualified, builds `first`'s
/// wrapper while another module of the package also declares an `Answer`.
///
/// The emitter places an imported alias by its name, which the other module's `Answer` makes ambiguous, and never
/// places a module-qualified one, so the return value was emitted without its wrapper.
#[test]
fn a_return_into_an_imported_union_alias_builds_its_wrapper_issue1796() -> TestResult {
    let package = generate_package(&[
        (
            "user",
            "import first\nfrom first import Answer\n\n\npub def make() -> Answer:\n    return 3\n\n\npub def make_qualified() -> first.Answer:\n    return 4\n",
        ),
        ("fifth", "pub type Answer = int | str\n"),
    ])?;
    let first_wrapper = package.wrapper_with_variant("V0(crate::first::Product)")?;
    let user = package.module("user")?;
    for value in [3, 4] {
        assert!(
            user.contains(&format!("crate::{first_wrapper}::V1({value})")),
            "`return {value}` builds `first`'s wrapper `{first_wrapper}`:\n{user}"
        );
    }
    Ok(())
}

/// The consumer module shared by the import-alias tests: a union over `Product` imported as `FirstProduct`.
const IMPORT_ALIAS_USER: &str = "from first import Product as FirstProduct\n\n\npub def pick(value: FirstProduct | str) -> str:\n    match value:\n        FirstProduct(product) => return f\"product {product.value}\"\n        str(text) => return text\n\n\npub def go() -> str:\n    return pick(FirstProduct(value=1))\n";

/// #1796: a union written with an import alias (`FirstProduct | str`) names the declaration the alias imports, so its
/// crate-root wrapper payload resolves there and the module still builds and matches it through the alias.
///
/// The wrapper payload was the alias, which only the importing module binds.
#[test]
fn a_union_over_an_import_alias_names_the_imported_declaration_issue1796() -> TestResult {
    let package = generate_package(&[("user", IMPORT_ALIAS_USER)])?;
    package.assert_no_payload("FirstProduct")?;
    let wrapper = package.wrapper_with_variant("V1(crate::first::Product)")?;
    let user = package.module("user")?;
    assert!(
        user.contains(&format!("crate::{wrapper}::V1(FirstProduct{{value:1}})")),
        "`go` builds the wrapper `{wrapper}` from the alias:\n{user}"
    );
    assert!(
        user.contains(&format!("crate::{wrapper}::V1(product)=>")),
        "`pick` matches the member through the wrapper `{wrapper}`:\n{user}"
    );
    Ok(())
}

/// #1796: the import alias names its declaration also where no other module declares that name, so the wrapper is
/// the one `Product | str` written with the declaration's own name uses, and its payload resolves at the crate root.
#[test]
fn a_union_over_an_import_alias_of_an_unshared_type_uses_the_declarations_wrapper_issue1796() -> TestResult {
    let package = generate_modules(&[
        ("first", product_module("first", "int")),
        ("user", IMPORT_ALIAS_USER.to_string()),
    ])?;
    package.assert_no_payload("FirstProduct")?;
    let wrapper = incan_ir::types::IrType::NamedGeneric(
        incan_ir::types::IR_UNION_TYPE_NAME.to_string(),
        vec![
            incan_ir::types::IrType::Struct("Product".to_string()),
            incan_ir::types::IrType::String,
        ],
    )
    .union_type_name()
    .ok_or("a union has a wrapper name")?;
    let user = package.module("user")?;
    assert!(
        user.contains(&format!("crate::{wrapper}::V0(FirstProduct{{value:1}})")),
        "`go` builds the wrapper `{wrapper}` of `Product | str` from the alias:\n{user}"
    );
    Ok(())
}
