//! Surface “method” vocabularies.
//!
//! This module groups the per-receiver method registries (e.g. `str` methods, `list` methods) into a single compilation
//! unit to reduce file sprawl, while keeping the public module structure re-exportable from `crate::lang::surface`.

use crate::lang::registry::LangItemInfo;

/// Resolve a spelling to a stable id for a registry table.
fn from_str_impl<Id: Copy>(items: &[LangItemInfo<Id>], name: &str) -> Option<Id> {
    if let Some(m) = items.iter().find(|m| m.canonical == name) {
        return Some(m.id);
    }
    items
        .iter()
        .find(|m| {
            let aliases: &[&str] = m.aliases;
            aliases.contains(&name)
        })
        .map(|m| m.id)
}

/// Return the registry metadata entry for a stable id.
///
/// ## Panics
/// - If the registry is missing an entry for `id` (this indicates a programming error).
fn info_for_impl<Id: Copy + PartialEq>(
    items: &'static [LangItemInfo<Id>],
    id: Id,
    missing_msg: &'static str,
) -> &'static LangItemInfo<Id> {
    items
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("{missing_msg}"))
}

pub mod string_methods;

pub mod set_methods;

pub mod list_methods;

pub mod dict_methods;

pub mod frozen_set_methods;

pub mod frozen_list_methods;

pub mod frozen_dict_methods;

pub mod bytes_methods;

pub mod frozen_bytes_methods;

pub mod float_methods;

pub mod option_methods;

pub mod result_methods;

pub mod iterator_methods;

pub mod clone_methods;
