//! The stdlib loader records where each const a parameter default names is declared (#1771).

use crate::typechecker::stdlib_loader::StdlibAstCache;

/// Spell a dotted path as owned segments.
fn path(dotted: &str) -> Vec<String> {
    dotted.split('.').map(str::to_string).collect()
}

/// #1771: `reader_digest`'s `chunk_size: int = DEFAULT_CHUNK_SIZE` names a const that `std.hash._streaming` imports
/// from `std.hash._core`. The loader resolves that spelling to the const's declaring module, whether the function is
/// looked up through the `std.hash` facade or through the module that declares it, and records nothing for a literal
/// default such as `length: int = 0`.
#[test]
fn stdlib_function_default_const_resolves_to_its_declaring_module_issue1771() -> Result<(), String> {
    let mut cache = StdlibAstCache::new();
    for module in ["std.hash", "std.hash._streaming"] {
        let source = cache
            .lookup_function_source(&path(module), "reader_digest")
            .ok_or_else(|| format!("`reader_digest` must be found through `{module}`"))?;
        assert_eq!(
            source.default_const_paths.get("DEFAULT_CHUNK_SIZE"),
            Some(&path("std.hash._core.DEFAULT_CHUNK_SIZE")),
            "through `{module}`: {:?}",
            source.default_const_paths
        );
        assert_eq!(
            source.default_const_paths.len(),
            1,
            "only the const default is recorded through `{module}`: {:?}",
            source.default_const_paths
        );
    }
    Ok(())
}
