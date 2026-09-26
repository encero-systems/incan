//! The generated Rust of modules and packages that re-export a projected declaration: every re-exported name binds the
//! projection its consumers import, publicly and once per module (#1750, #1751, #1764).

use std::collections::HashMap;
use std::error::Error;

use super::packages::{TestResult, declared_function_projection, parse, provider_plan_of, publish_package};
use crate::IrCodegen;
use incan_frontend::library_manifest::LibraryManifest;
use std::sync::Arc;

/// A function and a public `alias` declaration of it.
const PROVIDER: &str = "pub def scale(value: int) -> int:\n    return value * 2\n\n\npub scale_alias = alias scale\n";

/// Generate a project whose root is `main` and whose other modules are `modules`, by top-level module name.
fn generate_project(modules: &[(&str, &str)], main: &str) -> Result<HashMap<String, String>, Box<dyn Error>> {
    generate_project_with_dependencies(modules, main, &[])
}

/// Generate a project whose root is `main`, with its other `modules` and the `pub::` packages `dependencies`.
fn generate_project_with_dependencies(
    modules: &[(&str, &str)],
    main: &str,
    dependencies: &[&LibraryManifest],
) -> Result<HashMap<String, String>, Box<dyn Error>> {
    let programs = modules
        .iter()
        .map(|(name, source)| Ok((*name, parse(source)?)))
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let main = parse(main)?;
    let mut codegen = IrCodegen::new();
    codegen.set_provider_plan(Arc::new(provider_plan_of(dependencies)?));
    for (name, program) in &programs {
        codegen.add_module_with_path_segments(name, program, vec![name.to_string()]);
    }
    let paths = modules
        .iter()
        .map(|(name, _)| vec![name.to_string()])
        .collect::<Vec<_>>();
    let (main_code, generated) = codegen.try_generate_multi_file_nested(&main, &paths)?;
    let mut code = generated
        .into_iter()
        .map(|(path, code)| (path.join("."), code))
        .collect::<HashMap<_, _>>();
    code.insert("main".to_string(), main_code);
    Ok(code)
}

/// Return one generated module's code.
fn module<'a>(code: &'a HashMap<String, String>, name: &str) -> Result<&'a str, Box<dyn Error>> {
    Ok(code
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("no generated `{name}` module"))?)
}

/// Return how many lines of `code` are exactly `statement`.
fn count_lines(code: &str, statement: &str) -> usize {
    code.lines().filter(|line| line.trim() == statement).count()
}

/// Return the name `code` first reads through the module path `prefix`, whatever whitespace the Rust is laid out with.
fn read_through(code: &str, prefix: &str) -> Result<String, Box<dyn Error>> {
    let compacted = code
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    Ok(compacted
        .split(prefix)
        .nth(1)
        .and_then(|tail| {
            tail.split(|character: char| !character.is_alphanumeric() && character != '_')
                .next()
        })
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("nothing is read through `{prefix}`:\n{code}"))?)
}

/// Return the statement that publicly binds `name` to `projection` from the module path `from`.
fn public_binding(from: &str, projection: &str, name: &str) -> String {
    if name == projection {
        format!("pub use {from}::{projection};")
    } else {
        format!("pub use {from}::{projection} as {name};")
    }
}

/// #1750: a facade re-exporting an `alias` declaration without its target, a second facade renaming it again, and a
/// facade listing the alias before its target each bind the projection publicly once, beside every public name.
#[test]
fn alias_declaration_facades_bind_the_projection_they_reexport_issue1750() -> TestResult {
    let code = generate_project(
        &[
            ("provider", PROVIDER),
            ("facade", "pub from provider import scale_alias\n"),
            ("public_api", "pub from facade import scale_alias as exported_scale\n"),
            ("both_names", "pub from provider import scale_alias, scale\n"),
        ],
        "from both_names import scale\nfrom facade import scale_alias\nfrom public_api import exported_scale\n\n\ndef main() -> None:\n    println(scale(3))\n    println(scale_alias(21))\n    println(exported_scale(4))\n",
    )?;
    let projection = declared_function_projection(module(&code, "provider")?)?;

    let facade = module(&code, "facade")?;
    assert_eq!(
        count_lines(facade, &format!("pub use crate::provider::{projection};")),
        1,
        "{facade}"
    );
    assert_eq!(
        count_lines(
            facade,
            &format!("pub use crate::provider::{projection} as scale_alias;")
        ),
        1,
        "{facade}"
    );

    let public_api = module(&code, "public_api")?;
    assert_eq!(
        count_lines(public_api, &format!("pub use crate::facade::{projection};")),
        1,
        "{public_api}"
    );
    assert_eq!(
        count_lines(
            public_api,
            &format!("pub use crate::facade::{projection} as exported_scale;")
        ),
        1,
        "{public_api}"
    );

    let both_names = module(&code, "both_names")?;
    for statement in [
        format!("pub use crate::provider::{projection};"),
        format!("pub use crate::provider::{projection} as scale;"),
        format!("pub use crate::provider::{projection} as scale_alias;"),
    ] {
        assert_eq!(count_lines(both_names, &statement), 1, "`{statement}`:\n{both_names}");
    }

    let main = module(&code, "main")?;
    assert!(main.contains(&projection), "{main}");
    Ok(())
}

/// #1750: a facade that imports the target privately for its own use and re-exports the alias binds the projection
/// through the public re-export, so a consumer's import of the alias finds a public name (E0603 before).
#[test]
fn private_import_beside_a_public_reexport_binds_the_projection_publicly_issue1750() -> TestResult {
    let code = generate_project(
        &[
            ("provider", PROVIDER),
            (
                "facade",
                "from provider import scale\npub from provider import scale_alias\n\n\npub def doubled(value: int) -> int:\n    return scale(value)\n",
            ),
        ],
        "from facade import scale_alias\n\n\ndef main() -> None:\n    println(scale_alias(21))\n",
    )?;
    let projection = declared_function_projection(module(&code, "provider")?)?;
    let facade = module(&code, "facade")?;
    assert_eq!(
        count_lines(facade, &format!("pub use crate::provider::{projection};")),
        1,
        "{facade}"
    );
    assert_eq!(
        count_lines(facade, &format!("use crate::provider::{projection};")),
        0,
        "{facade}"
    );
    assert_eq!(
        count_lines(
            facade,
            &format!("pub use crate::provider::{projection} as scale_alias;")
        ),
        1,
        "{facade}"
    );
    Ok(())
}

/// #1764: an alias of a module member beside a direct import of the member binds the projection publicly and keeps
/// the alias's Rust-facing name.
#[test]
fn module_member_alias_beside_a_direct_import_binds_publicly_issue1764() -> TestResult {
    let code = generate_project(
        &[
            ("helpers", "pub def double(value: int) -> int:\n    return value * 2\n"),
            (
                "facade",
                "from helpers import double\nimport helpers as h\n\npub twice = h.double\n\n\npub def quadruple(value: int) -> int:\n    return double(double(value))\n",
            ),
        ],
        "from facade import twice\n\n\ndef main() -> None:\n    println(twice(21))\n",
    )?;
    let projection = declared_function_projection(module(&code, "helpers")?)?;
    let facade = module(&code, "facade")?;
    assert_eq!(
        count_lines(facade, &format!("pub use crate::helpers::{projection};")),
        1,
        "{facade}"
    );
    assert_eq!(
        count_lines(facade, &format!("use crate::helpers::{projection};")),
        0,
        "{facade}"
    );
    assert_eq!(
        count_lines(facade, &format!("pub use crate::helpers::{projection} as twice;")),
        1,
        "{facade}"
    );
    Ok(())
}

/// #1751: package `calc_facade` re-exports `calc_lib`'s renamed export under a further name. It binds the projection
/// publicly beside that name, and a third package importing `b_calculate` from it reads a name it binds.
#[test]
fn package_reexport_of_a_renamed_export_reaches_a_third_package_issue1751() -> TestResult {
    let (calc_lib, calc_lib_code) = publish_package(
        "calc_lib",
        "pub def calculate(value: int) -> int:\n    return value + 1\n\n\npub facade_calculate = alias calculate\n",
        &[],
    )?;
    let projection = declared_function_projection(&calc_lib_code)?;

    let (calc_facade, calc_facade_code) = publish_package(
        "calc_facade",
        "pub from pub::calc_lib import facade_calculate as b_calculate\n",
        &[&calc_lib],
    )?;
    assert_eq!(
        count_lines(&calc_facade_code, &format!("pub use calc_lib::{projection};")),
        1,
        "{calc_facade_code}"
    );
    assert_eq!(
        count_lines(
            &calc_facade_code,
            &format!("pub use calc_lib::{projection} as b_calculate;")
        ),
        1,
        "{calc_facade_code}"
    );

    let consumer =
        parse("from pub::calc_facade import b_calculate\n\n\ndef main() -> None:\n    println(b_calculate(41))\n")?;
    let mut codegen = IrCodegen::new();
    codegen.set_provider_plan(Arc::new(provider_plan_of(&[&calc_lib, &calc_facade])?));
    let consumer_code = codegen.try_generate(&consumer)?;
    let read = read_through(&consumer_code, "calc_facade::")?;
    assert_eq!(
        count_lines(&calc_facade_code, &public_binding("calc_lib", &projection, &read)),
        1,
        "the consumer reads `calc_facade::{read}`, which `calc_facade` binds publicly:\n{calc_facade_code}\n{consumer_code}"
    );
    Ok(())
}

/// #1751: a module re-exporting a package's renamed export under a further name binds the projection publicly beside
/// that name, and a sibling module importing the name through it reads a name it binds.
#[test]
fn module_reexport_of_a_package_renamed_export_binds_the_projection_issue1751() -> TestResult {
    let (calc_lib, calc_lib_code) = publish_package(
        "calc_lib",
        "pub def calculate(value: int) -> int:\n    return value + 1\n\n\npub facade_calculate = alias calculate\n",
        &[],
    )?;
    let projection = declared_function_projection(&calc_lib_code)?;
    let code = generate_project_with_dependencies(
        &[(
            "facade",
            "pub from pub::calc_lib import facade_calculate as b_calculate\n",
        )],
        "from facade import b_calculate\n\n\ndef main() -> None:\n    println(b_calculate(41))\n",
        &[&calc_lib],
    )?;
    let facade = module(&code, "facade")?;
    assert_eq!(
        count_lines(facade, &format!("pub use calc_lib::{projection};")),
        1,
        "{facade}"
    );
    assert_eq!(
        count_lines(facade, &format!("pub use calc_lib::{projection} as b_calculate;")),
        1,
        "{facade}"
    );
    let main = module(&code, "main")?;
    let read = read_through(main, "crate::facade::")?;
    assert_eq!(
        count_lines(facade, &public_binding("calc_lib", &projection, &read)),
        1,
        "`main` reads `facade::{read}`, which `facade` binds publicly:\n{facade}\n{main}"
    );
    Ok(())
}
