//! Imports of derive vocabulary (`from std.derives.copying import Clone`): the stdlib declaration stub binds nothing in
//! generated Rust, so the import lowers to no IR import while protocol and callable imports stay (#1727).

use super::*;

/// #1727: importing the stdlib's declaration stub of a derivable trait binds nothing in generated Rust, so the
/// import lowers to no IR import; the adopter's expanded stdlib adapter defaults keep the bare `Clone` bound the
/// stdlib trait itself compiles against, which the module's Rust prelude resolves once nothing shadows it. The
/// source-owned protocol and the callable trait keep their imports.
#[test]
fn derive_vocabulary_imports_lower_to_no_ir_import_issue1727() -> Result<(), String> {
    let ir = lower_source(
        r#"
from std.derives.collection import FallibleIterator
from std.derives.copying import Clone
from std.derives.comparison import Eq as Equality
from std.traits.callable import Callable1

model Readings with FallibleIterator[int, str]:
  items: list[int]
  index: int

  def __next__(mut self) -> Result[Option[int], str]:
    if self.index >= len(self.items):
      return Ok(None)
    item = self.items[self.index]
    self.index += 1
    return Ok(Some(item))

@derive(Clone, Eq)
model Double with Callable1[int, int]:
  def __call__(self, value: int) -> int:
    return value * 2

def pick[T with Equality](value: T) -> T:
  return value

def main() -> None:
  match Readings(items=[1, 2, 3], index=0).map(Double()).collect():
    Ok(values) => println(len(values))
    Err(error) => println(error)
  chosen = pick(Double())
  println(chosen(21))
"#,
    )
    .map_err(|errors| format!("lowering failed: {errors:?}"))?;

    let imports = ir
        .declarations
        .iter()
        .filter_map(|decl| match &decl.kind {
            IrDeclKind::Import { path, items, .. } => Some((
                path.join("."),
                items.iter().map(|item| item.name.clone()).collect::<Vec<_>>(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        imports.contains(&(
            "std.derives.collection".to_string(),
            vec!["FallibleIterator".to_string()]
        )),
        "the source-owned protocol keeps its import: {imports:?}"
    );
    assert!(
        imports.contains(&("std.traits.callable".to_string(), vec!["Callable1".to_string()])),
        "the callable trait keeps its import: {imports:?}"
    );
    assert!(
        !imports
            .iter()
            .any(|(path, _)| path == "std.derives.copying" || path == "std.derives.comparison"),
        "derive vocabulary binds nothing in generated Rust: {imports:?}"
    );

    let adopter = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Impl(impl_block)
                if impl_block.target_type == "Readings"
                    && impl_block.trait_name.as_deref() == Some("FallibleIterator") =>
            {
                Some(impl_block)
            }
            _ => None,
        })
        .ok_or("the FallibleIterator adopter impl is missing")?;
    let map = adopter
        .methods
        .iter()
        .find(|method| method.name == "map")
        .ok_or("the expanded `map` default is missing from the adopter")?;
    let clone_bound = incan_lang::lang::trait_bounds::incan_to_rust(core_traits::as_str(TraitId::Clone))
        .ok_or("the registry maps `Clone`")?;
    let map_fn = map
        .type_params
        .iter()
        .find(|param| param.name == "MapFn")
        .ok_or("the expanded `map` default keeps its `MapFn` parameter")?;
    assert!(
        map_fn.bounds.iter().any(|bound| bound.trait_path == clone_bound),
        "the expanded default spells the derive-owned `Clone` bound: {:?}",
        map_fn.bounds
    );

    // The aliased derivable bound is the Rust trait its derive implements, never the generated stub.
    let pick = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == "pick" => Some(function),
            _ => None,
        })
        .ok_or("the generic function `pick` is missing")?;
    let equality_bound = incan_lang::lang::trait_bounds::incan_to_rust(core_traits::as_str(TraitId::Eq))
        .ok_or("the registry maps `Eq`")?;
    let t = pick
        .type_params
        .iter()
        .find(|param| param.name == "T")
        .ok_or("`pick` keeps its `T` parameter")?;
    assert!(
        t.bounds.iter().any(|bound| bound.trait_path == equality_bound),
        "`T with Equality` lowers to the registry's Rust bound: {:?}",
        t.bounds
    );
    Ok(())
}
