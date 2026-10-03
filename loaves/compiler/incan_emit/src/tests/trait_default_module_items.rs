//! Defaults of a trait declared in another module of the program, expanded into an adopter and built and run by rustc.

use std::collections::HashMap;

use super::mut_ownership_regressions::run_generated_modules;
use super::packages::{TestResult, parse};
use crate::IrCodegen;

/// #1759, #1873: an adopter in another module runs the defaults of a trait from a nested module, which construct that
/// module's model, read its field and read the module's private constant, while the adopter's module declares and uses
/// a model of the same name.
#[test]
fn trait_default_reads_its_module_model_and_private_const_issue1873() -> TestResult {
    let shapes = parse(
        r#"
const LIMIT: int = 7


pub model Extent:
    pub n: int


pub trait Measured:
    def extent(self) -> Extent:
        return Extent(n=2)

    def extent_size(self) -> int:
        return self.extent().n

    def limit(self) -> int:
        return LIMIT
"#,
    )?;
    let main = parse(
        r#"
from geometry.shapes import Measured


model Extent:
    label: str


model Card with Measured:
    size: int


def main() -> None:
    card = Card(size=1)
    println(Extent(label="local").label)
    println(card.extent_size())
    println(card.limit())
"#,
    )?;
    let path = vec!["geometry".to_string(), "shapes".to_string()];
    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("geometry_shapes", &shapes, path.clone());
    let (root, modules) = codegen.try_generate_multi_file_nested(&main, std::slice::from_ref(&path))?;
    let modules = modules.into_iter().collect::<HashMap<_, _>>();
    assert_eq!(run_generated_modules(&root, &modules)?, "local\n2\n7\n");
    Ok(())
}
