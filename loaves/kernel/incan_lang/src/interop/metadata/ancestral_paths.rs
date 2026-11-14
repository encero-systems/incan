//! Ancestral Rust paths and displays for `std` re-exports and prelude roots.

/// `std` re-exports of items that `core` and `alloc` define.
///
/// Rust spells one item several ways: `std::boxed::Box`, `alloc::boxed::Box`, a prelude `Box`, and any crate that
/// re-exports the standard library for `no_std` consumers (`prost::alloc::boxed::Box`). Rust metadata identifies the
/// defining crate, so it records the ancestral spelling; Incan source imports the `std` one. The two
/// can only unify if comparison happens in the ancestral namespace, and this table is the whole of that knowledge —
/// facts about the standard library, not a list of downstream crates.
pub(super) const STD_REEXPORT_ROOTS: &[(&str, &str)] = &[
    ("std::option::", "core::option::"),
    ("std::result::", "core::result::"),
    ("std::boxed::", "alloc::boxed::"),
    ("std::vec::", "alloc::vec::"),
    ("std::string::", "alloc::string::"),
    ("std::rc::", "alloc::rc::"),
    ("std::cell::", "core::cell::"),
    ("std::iter::", "core::iter::"),
    // Only the `alloc`-defined items of `std::sync`; `Mutex`, `RwLock`, and friends are `std`'s own.
    ("std::sync::Arc", "alloc::sync::Arc"),
    ("std::sync::Weak", "alloc::sync::Weak"),
];

/// Prelude spellings of the items in [`STD_REEXPORT_ROOTS`], as rust-analyzer displays them inside the crate that
/// defines them and as Rust source commonly writes them.
pub(super) const PRELUDE_ROOTS: &[(&str, &str)] = &[
    ("Box", "alloc::boxed::Box"),
    ("Option", "core::option::Option"),
    ("Result", "core::result::Result"),
    ("Vec", "alloc::vec::Vec"),
    ("String", "alloc::string::String"),
];

/// Rewrite one Rust item path to the crate that defines it.
///
/// Strips Incan's synthetic `rust::` surface namespace first, then folds a `std` re-export onto its `core`/`alloc`
/// origin. Paths that are already ancestral, or that name ordinary crates, pass through unchanged.
#[must_use]
pub fn ancestral_rust_path(path: &str) -> String {
    let path = path.strip_prefix("rust::").unwrap_or(path);
    // A bare prelude name is one more spelling of the same item: metadata that reads `Box<T>` names
    // `alloc::boxed::Box<T>` exactly as a `std::boxed::Box` import does.
    if let Some(ancestral) = PRELUDE_ROOTS
        .iter()
        .find_map(|(name, ancestral)| (*name == path).then_some(*ancestral))
    {
        return ancestral.to_string();
    }
    for (reexport, ancestral) in STD_REEXPORT_ROOTS {
        if let Some(rest) = path.strip_prefix(reexport) {
            return format!("{ancestral}{rest}");
        }
    }
    path.to_string()
}

/// Return whether a recorded Rust path names the `Box` storage carrier, through any re-export.
///
/// `alloc::boxed::Box` reaches metadata under many spellings: the prelude `Box`, `std::boxed::Box`, and the
/// re-exports generated code leans on (`prost::alloc::boxed::Box`, or a crate's `pub extern crate alloc`). They are
/// one item, so the recorders that strip a variant payload's carrier decide through this rule rather than a spelling.
pub fn is_box_carrier_path(path: &str) -> bool {
    let ancestral = ancestral_rust_path(path);
    ancestral == "alloc::boxed::Box"
        || ancestral.ends_with("::alloc::boxed::Box")
        || ancestral.ends_with("::std::boxed::Box")
}

/// Rewrite every item path inside a Rust type display — generic arguments included — to its ancestral spelling.
///
/// `std::option::Option<std::boxed::Box<T>>` and `core::option::Option<alloc::boxed::Box<T>>` name the same type;
/// nominal comparison of displays must see them as equal.
#[must_use]
pub fn ancestral_rust_display(display: &str) -> String {
    // rust-analyzer spells the standard collections with their defaulted allocator (`Vec<T, alloc::Global>`,
    // `Box<T, A = Global>` rendered as `Box<T, alloc::Global>`); written Rust and Incan source never do. The
    // default is not part of the item's identity, so drop it before comparing.
    let display = strip_defaulted_global_allocator(display);
    let display = display.as_str();
    let mut out = String::with_capacity(display.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if !token.is_empty() {
            out.push_str(&ancestral_rust_path(token));
            token.clear();
        }
    };
    for ch in display.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == ':' || ch == '#' {
            token.push(ch);
        } else {
            flush(&mut token, &mut out);
            out.push(ch);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// Remove a trailing `, alloc::Global` / `, std::alloc::Global` generic argument wherever it appears in a display.
pub(super) fn strip_defaulted_global_allocator(display: &str) -> String {
    let mut out = display.to_string();
    for spelling in [
        ", alloc::Global>",
        ",alloc::Global>",
        ", std::alloc::Global>",
        ",std::alloc::Global>",
    ] {
        while let Some(index) = out.find(spelling) {
            out.replace_range(index..index + spelling.len() - 1, "");
        }
    }
    out
}
