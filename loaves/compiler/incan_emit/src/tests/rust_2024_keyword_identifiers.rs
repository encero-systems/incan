//! Source names that are keywords of Rust 2024, the edition generated code is compiled with: every emitted identifier
//! spelled like one is a raw identifier, so a function, parameter, field or module named `gen` builds (#1561).

use std::error::Error;

use super::mut_ownership_regressions::compile_generated_rust;
use super::packages::parse;
use crate::IrCodegen;

/// Generate one program and compile it with rustc as a library.
fn build(case: &str, source: &str) -> Result<String, Box<dyn Error>> {
    let code = IrCodegen::new()
        .try_generate(&parse(source)?)
        .map_err(|error| format!("{case}: generation failed: {error:?}"))?;
    compile_generated_rust(&code).map_err(|error| format!("{case} did not build: {error}\n{code}"))?;
    Ok(code)
}

/// #1561: a function, a parameter and a field named `gen` are emitted as `r#gen` and build.
#[test]
fn gen_named_function_parameter_and_field_build_issue1561() -> Result<(), Box<dyn Error>> {
    let function = build(
        "function",
        "pub def gen() -> int:\n    return 1\n\npub def caller() -> int:\n    return gen() + 1\n",
    )?;
    assert!(
        function.contains("r#gen"),
        "the function is not a raw identifier:\n{function}"
    );
    build(
        "parameter",
        "pub def twice(gen: int) -> int:\n    return gen * 2\n\npub def caller() -> int:\n    return twice(gen=3)\n",
    )?;
    build(
        "field",
        "pub model Batch:\n    pub gen: int\n\npub def next_batch(batch: Batch) -> int:\n    return batch.gen + 1\n\npub def make() -> Batch:\n    return Batch(gen=1)\n",
    )?;
    build(
        "local captured by a closure",
        "pub def total(values: list[int]) -> int:\n    gen = 2\n    scale = () => gen * 3\n    return scale() + len(values)\n",
    )?;
    Ok(())
}

/// #1561: a module named `gen` and a function it exports are reached through `r#gen` from the importing module.
#[test]
fn gen_named_module_builds_issue1561() -> Result<(), Box<dyn Error>> {
    let module = parse("pub def produce() -> int:\n    return 7\n")?;
    let main = parse(
        "from gen import produce\nimport gen\n\npub def total() -> int:\n    return produce() + gen.produce()\n",
    )?;
    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("gen", &module, vec!["gen".to_string()]);
    let (main_code, generated) = codegen.try_generate_multi_file_nested(&main, &[vec!["gen".to_string()]])?;
    let module_code = generated
        .into_iter()
        .find_map(|(path, code)| (path == ["gen".to_string()]).then_some(code))
        .ok_or("no generated `gen` module")?;
    assert!(
        main_code.contains("r#gen"),
        "the importing module does not name the module as a raw identifier:\n{main_code}"
    );
    // The project generator declares the module as `#[path = "gen.rs"] mod r#gen;`; an inline module is the same
    // crate shape for one rustc invocation.
    let crate_source = format!("{main_code}\npub mod r#gen {{\n{module_code}\n}}\n");
    compile_generated_rust(&crate_source)
        .map_err(|error| format!("the `gen` module did not build: {error}\n{crate_source}"))?;
    Ok(())
}
