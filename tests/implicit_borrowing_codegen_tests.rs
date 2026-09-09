//! Regression coverage for reference shape and typed assignment ownership (#1455).

use incan::backend::ir::IrCodegen;
use incan::frontend::{lexer, parser};

#[allow(dead_code)]
#[path = "../src/oven/compiler_suite_env.rs"]
mod compiler_suite_env;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn private_enum_classifier_infers_shared_parameter() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::toml_edit import Item

def is_table(item: Item) -> bool:
    match item:
        Item.Table(_) => return true
        _ => return false

pub class Holder:
    item: Item

    def observe(self) -> bool:
        return is_table(self.item)
"#,
    )?;
    assert!(rust.contains("item:&Item"), "{rust}");
    assert!(!rust.contains("self.item.clone()"), "{rust}");
    Ok(())
}

#[test]
fn private_classifier_borrow_flows_through_a_helper() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::toml_edit import Item

def inspect(item: Item) -> bool:
    return classify(item)

def classify(item: Item) -> bool:
    match item:
        Item.Table(_) => return true
        _ => return false

pub def observe(item: Item) -> bool:
    return inspect(item)
"#,
    )?;
    assert_eq!(rust.matches("item:&Item").count(), 2, "{rust}");
    assert!(!rust.contains("item.clone()"), "{rust}");
    Ok(())
}

#[test]
fn escaping_classifier_keeps_its_callable_abi() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::toml_edit import Item

def classify(item: Item) -> bool:
    match item:
        Item.Table(_) => return true
        _ => return false

pub def callback() -> Callable[Item, bool]:
    return classify
"#,
    )?;
    assert!(!rust.contains("item:&Item"), "{rust}");
    Ok(())
}

#[test]
fn unknown_rust_receiver_does_not_authorize_borrowing() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::demo import Item

def inspect(item: Item) -> bool:
    return item.observe_value()

pub def observe(item: Item) -> bool:
    return inspect(item)
"#,
    )?;
    assert!(!rust.contains("item:&Item"), "{rust}");
    Ok(())
}

#[test]
fn overlapping_call_arguments_keep_the_owned_helper_abi() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::demo import Item

def classify(item: Item, other: Item) -> bool:
    match item:
        Item.Table(_) => return other.consume_value()
        _ => return false

pub def observe(item: Item) -> bool:
    return classify(item, item)
"#,
    )?;
    assert!(!rust.contains("item:&Item"), "{rust}");
    Ok(())
}

#[test]
fn escaping_cursor_keeps_its_owned_value() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::demo import Item

def select(item: Item) -> Item:
    mut current = item
    match current:
        Item.Table(_) => return current
        _ => return current

pub def observe(item: Item) -> Item:
    return select(item)
"#,
    )?;
    assert!(!rust.contains("item:&Item"), "{rust}");
    assert!(!rust.contains("current:&Item"), "{rust}");
    Ok(())
}

#[test]
fn mutating_pattern_payload_keeps_the_owned_parameter() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::demo import Item

def clear(item: Item) -> bool:
    match item:
        Item.Table(table) =>
            table.clear()
            return true
        _ => return false

pub def observe(item: Item) -> bool:
    return clear(item)
"#,
    )?;
    assert!(!rust.contains("item:&Item"), "{rust}");
    Ok(())
}

#[test]
fn crate_visible_helpers_keep_owned_entry_points_and_borrow_local_calls() -> TestResult {
    use incan::backend::ir::{AstLowering, IrEmitter};
    let source = r#"
from rust::toml_edit import Item

def classify(item: Item) -> bool:
    match item:
        Item.Table(_) => return true
        _ => return false

pub class Holder:
    item: Item

    def observe(self) -> bool:
        return classify(self.item)
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse: {errors:?}")))?;
    let mut checker = incan::frontend::typechecker::TypeChecker::new();
    checker.set_current_module_path(Some(vec!["__incan_std".to_string(), "borrow_fixture".to_string()]));
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("check: {errors:?}")))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    lowering.set_current_source_module_name(Some("__incan_std.borrow_fixture".to_string()));
    let ir = lowering
        .lower_program(&ast)
        .map_err(|error| std::io::Error::other(format!("lower: {error:?}")))?;
    let rust: String = IrEmitter::new(&ir.function_registry)
        .emit_program(&ir)?
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(rust.contains("item:Item"), "{rust}");
    assert!(rust.contains("item:&Item"), "{rust}");
    assert!(rust.contains("fn__incan_shared_function_"), "{rust}");
    assert!(!rust.contains("self.item.clone()"), "{rust}");
    Ok(())
}

/// Seed source-proven call facts at the checker/lowering boundary; extractor tests independently prove their origin.
fn rust_with_receiver_contracts(source: &str, calls: &[(&str, bool)]) -> Result<String, std::io::Error> {
    use incan::backend::ir::{AstLowering, IrEmitter};
    use incan::frontend::typechecker::TypeChecker;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse: {errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("check: {errors:?}")))?;
    let mut info = checker.type_info().clone();
    for (call, returns_receiver_borrow) in calls {
        let start = source
            .find(call)
            .ok_or_else(|| std::io::Error::other("missing call fixture"))?;
        let result_type = if *returns_receiver_borrow {
            incan::frontend::symbols::ResolvedType::Generic(
                "Option".to_string(),
                vec![incan::frontend::symbols::ResolvedType::Ref(Box::new(
                    incan::frontend::symbols::ResolvedType::Named("Item".to_string()),
                ))],
            )
        } else {
            incan::frontend::symbols::ResolvedType::Bool
        };
        info.expressions
            .expr_types
            .insert((start, start + call.len()), result_type);
        info.rust.receiver_contracts.insert(
            (start, start + call.len()),
            incan_core::interop::RustReceiverContract {
                shared: true,
                returns_receiver_borrow: *returns_receiver_borrow,
            },
        );
    }
    let ir = AstLowering::new_with_type_info(info)
        .lower_program(&ast)
        .map_err(|error| std::io::Error::other(format!("lower: {error:?}")))?;
    IrEmitter::new(&ir.function_registry)
        .emit_program(&ir)
        .map_err(|error| std::io::Error::other(format!("emit: {error:?}")))
}

#[test]
fn proven_receiver_child_infers_a_borrowed_helper_and_cursor() -> TestResult {
    let source = r#"
from rust::toml_edit import Item

def traverse(item: Item, keys: list[str]) -> bool:
    mut current = item
    for key in keys:
        match current.get(key):
            Some(child) => current = child
            None => return false
    return current.is_integer()

pub def observe(item: Item) -> bool:
    return traverse(item, ["project", "count"])
"#;
    let rust = rust_with_receiver_contracts(source, &[("current.get(key)", true), ("current.is_integer()", false)])?;
    let compact: String = rust.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(compact.contains("item:&Item"), "{rust}");
    assert!(compact.contains("current=child;"), "{rust}");
    assert!(
        !compact.contains("item.clone()") && !compact.contains("child.clone()"),
        "{rust}"
    );
    Ok(())
}

#[test]
fn inferred_method_helper_preserves_owned_public_abi() -> TestResult {
    let source = r#"
from rust::toml_edit import Item

pub class Holder:
    item: Item

    def walk(self, item: Item, keys: list[str]) -> bool:
        mut current = item
        for key in keys:
            match current.get(key):
                Some(child) => current = child
                None => return false
        return current.is_integer()

    def observe(self) -> bool:
        return self.walk(self.item, ["project", "count"])
"#;
    let rust = rust_with_receiver_contracts(source, &[("current.get(key)", true), ("current.is_integer()", false)])?;
    assert!(rust.contains("fn __incan_shared_method_"), "{rust}");
    assert!(!rust.contains("self.item.clone()"), "{rust}");
    assert!(
        rust.contains("item: Item"),
        "the original public method keeps its owned parameter: {rust}"
    );
    Ok(())
}

#[test]
fn inline_tree_cursor_borrows_a_field_without_annotations() -> TestResult {
    let source = r#"
from rust::toml_edit import Item

pub class Holder:
    item: Item

    def walk(self, keys: list[str]) -> bool:
        mut current = self.item
        for key in keys:
            match current.get(key):
                Some(child) => current = child
                None => return false
        return current.is_integer()
"#;
    let rust = rust_with_receiver_contracts(source, &[("current.get(key)", true), ("current.is_integer()", false)])?;
    assert!(!rust.contains("self.item.clone()"), "{rust}");
    assert!(rust.contains("= &self.item"), "{rust}");
    Ok(())
}

#[test]
fn later_owner_mutation_preserves_the_cursor_snapshot() -> TestResult {
    let source = r#"
from rust::toml_edit import Item

pub class Holder:
    item: Item

    def inspect(mut self, replacement: Item) -> bool:
        current = self.item
        self.item = replacement
        return current.is_integer()
"#;
    let rust = rust_with_receiver_contracts(source, &[("current.is_integer()", false)])?;
    assert!(rust.contains("self.item.clone()"), "{rust}");
    Ok(())
}

#[test]
fn unrelated_return_lifetime_does_not_authorize_cursor_assignment() -> TestResult {
    let source = r#"
from rust::toml_edit import Item

def walk(item: Item, key: str) -> bool:
    mut current = item
    match current.get(key):
        Some(child) => current = child
        None => return false
    return current.is_integer()

pub def observe(item: Item) -> bool:
    return walk(item, "count")
"#;
    let rust = rust_with_receiver_contracts(source, &[("current.get(key)", false), ("current.is_integer()", false)])?;
    assert!(!rust.contains("item: &Item"), "{rust}");
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn inferred_traversal_compiles_and_runs_without_any_tree_clones() -> TestResult {
    let source = r#"
from rust::borrow_fixture import Item

def traverse(item: Item, keys: list[str]) -> bool:
    mut current = item
    for key in keys:
        match current.get(key):
            Some(child) => current = child
            None => return false
    return current.is_integer()

pub def observe(item: Item) -> bool:
    return traverse(item, ["project", "count"])

pub class Reader:
    def select(self, item: Item, keys: list[str]) -> Item:
        mut current = item
        for key in keys:
            if let Some(span) = current.span():
                assert span >= 0
            match current.get(key):
                Some(child) => current = child
                None => return Item.Integer(0)
        return current.clone()

    def read(self, item: Item) -> Item:
        return self.select(item, ["project", "count"])

pub class Holder:
    item: Item

    def snapshot(mut self) -> bool:
        current = self.item
        self.item = Item.Integer(0)
        return current.is_integer()
"#;
    let directory = tempfile::tempdir()?;
    std::fs::create_dir_all(directory.path().join("src"))?;
    let fixture = directory.path().join("src/lib.rs");
    std::fs::write(
        &fixture,
        r#"
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
static CLONES: AtomicUsize = AtomicUsize::new(0);
#[derive(Debug)]
pub enum Item { Table(BTreeMap<String, Item>), Integer(i64) }
impl Item {
    pub fn get(&self, key: &str) -> Option<&Item> {
        match self { Self::Table(items) => items.get(key), _ => None }
    }
    pub fn is_integer(&self) -> bool { matches!(self, Self::Integer(_)) }
    pub fn span(&self) -> Option<usize> { Some(0) }
}
impl Clone for Item {
    fn clone(&self) -> Self {
        CLONES.fetch_add(1, Ordering::SeqCst);
        match self { Self::Table(items) => Self::Table(items.clone()), Self::Integer(value) => Self::Integer(*value) }
    }
}
pub fn document() -> Item {
    Item::Table(BTreeMap::from([("project".into(), Item::Table(BTreeMap::from([("count".into(), Item::Integer(7))])))]))
}
pub fn clones() -> usize { CLONES.load(Ordering::SeqCst) }
"#,
    )?;
    std::fs::write(
        directory.path().join("Cargo.toml"),
        r#"[package]
name = "borrow_fixture"
version = "0.1.0"
edition = "2024"
"#,
    )?;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse: {errors:?}")))?;
    let mut checker = incan::frontend::typechecker::TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(directory.path().to_path_buf());
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("check: {errors:?}")))?;
    for call in [
        "current.get(key)",
        "current.is_integer()",
        "current.span()",
        "current.clone()",
    ] {
        for (start, _) in source.match_indices(call) {
            assert!(
                checker
                    .type_info()
                    .rust
                    .receiver_contracts
                    .contains_key(&(start, start + call.len())),
                "missing source contract for {call}"
            );
        }
    }
    let ir = incan::backend::ir::AstLowering::new_with_type_info(checker.type_info().clone())
        .lower_program(&ast)
        .map_err(|error| std::io::Error::other(format!("lower: {error:?}")))?;
    let generated = incan::backend::ir::IrEmitter::new(&ir.function_registry).emit_program(&ir)?;
    let capability = compiler_suite_env::OvenCompilerSuiteCapability::from_environment(
        compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV,
    )?;
    let rustc = capability
        .as_ref()
        .map(|capability| capability.rustc.clone())
        .unwrap_or_else(|| {
            std::env::var_os("RUSTC")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| "rustc".into())
        });
    let fixture_library = directory.path().join("libborrow_fixture.rlib");
    let fixture_output = std::process::Command::new(&rustc)
        .args(["--edition=2024", "--crate-type=rlib", "--crate-name=borrow_fixture"])
        .arg(&fixture)
        .arg("-o")
        .arg(&fixture_library)
        .output()?;
    assert!(
        fixture_output.status.success(),
        "{}",
        String::from_utf8_lossy(&fixture_output.stderr)
    );
    let consumer = directory.path().join("consumer.rs");
    std::fs::write(
        &consumer,
        format!(
            "{generated}\nfn main() {{ assert!(observe(borrow_fixture::document())); assert_eq!(borrow_fixture::clones(), 0); let reader = Reader {{}}; let selected = reader.read(borrow_fixture::document()); assert!(matches!(selected, borrow_fixture::Item::Integer(7))); assert_eq!(borrow_fixture::clones(), 1); let mut holder = Holder {{ item: borrow_fixture::document() }}; assert!(!holder.snapshot()); assert!(matches!(holder.item, borrow_fixture::Item::Integer(0))); assert_eq!(borrow_fixture::clones(), 4); }}\n"
        ),
    )?;
    let binary = directory
        .path()
        .join(format!("consumer{}", std::env::consts::EXE_SUFFIX));
    let mut command = std::process::Command::new(&rustc);
    command.args(["--edition=2024", "--crate-name=borrow_consumer"]);
    if let Some(capability) = capability {
        for path in capability.dependency_search_paths {
            command.arg("-L").arg(format!("dependency={}", path.display()));
        }
        for (name, path) in capability.externs {
            command.arg("--extern").arg(format!("{name}={}", path.display()));
        }
    } else {
        let executable = std::env::current_exe()?;
        let dependencies = executable
            .parent()
            .ok_or("test executable has no dependency directory")?;
        let candidates = std::fs::read_dir(dependencies)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("libincan_stdlib-") && name.ends_with(".rlib"))
            })
            .collect::<Vec<_>>();
        let [stdlib] = candidates.as_slice() else {
            return Err(
                "native borrowing proof requires one compiled stdlib artifact or an explicit Oven capability".into(),
            );
        };
        command.arg("-L").arg(format!("dependency={}", dependencies.display()));
        command
            .arg("--extern")
            .arg(format!("incan_stdlib={}", stdlib.display()));
        let derives = std::fs::read_dir(dependencies)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.trim_start_matches("lib").starts_with("incan_derive-"))
                    && path.extension().and_then(|extension| extension.to_str())
                        == Some(std::env::consts::DLL_EXTENSION)
            })
            .collect::<Vec<_>>();
        let [derive] = derives.as_slice() else {
            return Err(
                "native borrowing proof requires one compiled derive artifact or an explicit Oven capability".into(),
            );
        };
        command
            .arg("--extern")
            .arg(format!("incan_derive={}", derive.display()));
    }
    let output = command
        .arg("--extern")
        .arg(format!("borrow_fixture={}", fixture_library.display()))
        .arg(&consumer)
        .arg("-o")
        .arg(&binary)
        .output()?;
    assert!(
        output.status.success(),
        "{}\n{generated}",
        String::from_utf8_lossy(&output.stderr)
    );
    let execution = std::process::Command::new(binary).output()?;
    assert!(
        execution.status.success(),
        "{}\n{generated}",
        String::from_utf8_lossy(&execution.stderr)
    );
    Ok(())
}

/// Lower source through the ordinary compiler and normalize only whitespace for ownership assertions.
fn compact_rust(source: &str) -> Result<String, std::io::Error> {
    let tokens =
        lexer::lex(source).map_err(|errors| std::io::Error::other(format!("fixture did not lex: {errors:?}")))?;
    let program =
        parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("fixture did not parse: {errors:?}")))?;
    let generated = IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| std::io::Error::other(format!("fixture did not codegen: {error:?}")))?;
    Ok(generated
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect())
}

#[test]
fn explicit_reference_local_borrows_a_field_without_cloning() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::toml_edit import Item

pub class Holder:
    item: Item

    def observe(self) -> bool:
        current: &Item = self.item
        return current.is_table()
"#,
    )?;
    assert!(rust.contains("letcurrent:&Item=&self.item;"), "{rust}");
    assert!(!rust.contains("self.item.clone()"), "{rust}");
    Ok(())
}

#[test]
fn owned_reassignment_materializes_a_borrowed_generic_and_infers_clone_bound() -> TestResult {
    let rust = compact_rust(
        r#"
pub def replace[T](first: T, second: &T) -> T:
    mut current = first
    current = second
    return current
"#,
    )?;
    assert!(rust.contains("<T:Clone,"), "{rust}");
    assert!(rust.contains("current=second.clone();"), "{rust}");
    Ok(())
}

#[test]
fn borrowed_reassignment_preserves_reference_shape_without_clone_bound() -> TestResult {
    let rust = compact_rust(
        r#"
pub def replace[T](first: &T, second: &T) -> None:
    mut current = first
    current = second
"#,
    )?;
    assert!(rust.contains("current=second;"), "{rust}");
    assert!(!rust.contains("T:Clone") && !rust.contains("second.clone()"), "{rust}");
    Ok(())
}

#[test]
fn owned_reassignment_preserves_a_field_source() -> TestResult {
    let rust = compact_rust(
        r#"
pub class Holder:
    name: str

    def copy_name(self) -> str:
        mut current = ""
        current = self.name
        return current
"#,
    )?;
    assert!(rust.contains("current=self.name.clone();"), "{rust}");
    Ok(())
}

#[test]
fn opaque_rust_child_reassignment_does_not_add_a_second_borrow() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::toml_edit import Item

pub def traverse(item: &Item, keys: list[str]) -> bool:
    mut current = item
    for key in keys:
        match current.get(key):
            Some(child) => current = child
            None => return false
    return current.is_integer()
"#,
    )?;
    assert!(rust.contains("current=child;"), "{rust}");
    assert!(
        !rust.contains("current=&child") && !rust.contains("current=child.clone()"),
        "{rust}"
    );
    Ok(())
}

#[test]
fn callback_in_a_trait_default_keeps_the_owned_abi() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::demo import Item

def classify(item: Item) -> bool:
    match item:
        Item.Table(_) => return true
        _ => return false

pub trait Factory:
    def callback(self) -> Callable[Item, bool]:
        return classify
"#,
    )?;
    assert!(!rust.contains("item:&Item"), "{rust}");
    Ok(())
}

#[test]
fn comprehension_shadowing_does_not_rewrite_an_unrelated_binding() -> TestResult {
    let rust = compact_rust(
        r#"
from rust::demo import Item

def classify(item: Item) -> bool:
    values = [item for item in [1, 2]]
    match item:
        Item.Table(_) => return true
        _ => return false

pub def observe(item: Item) -> bool:
    return classify(item)
"#,
    )?;
    assert!(!rust.contains("item:&Item"), "{rust}");
    Ok(())
}
