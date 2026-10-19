//! The Rust prelude type names signatures write bare, and the standard modules that define them.

/// Return the standard module that defines a prelude type name (`Result` → `result`), or `None` for any other name.
///
/// A bare prelude name means that standard item only when the declaring module imports nothing else under it:
/// `std::panic` writes `Result<R>` after `use crate::thread::Result;`.
pub(crate) fn prelude_type_module(name: &str) -> Option<&'static str> {
    match name {
        "String" => Some("string"),
        "Vec" => Some("vec"),
        "Option" => Some("option"),
        "Result" => Some("result"),
        "Box" => Some("boxed"),
        _ => None,
    }
}

/// Whether `path` is the standard prelude's item for `name` (`core::result::Result`, `alloc::vec::Vec`, …).
pub(crate) fn names_prelude_item(path: &str, name: &str) -> bool {
    let Some(module) = prelude_type_module(name) else {
        return false;
    };
    let segments = path.split("::").collect::<Vec<_>>();
    matches!(
        segments.as_slice(),
        [root, owner, item] if matches!(*root, "core" | "alloc" | "std") && *owner == module && *item == name
    )
}
