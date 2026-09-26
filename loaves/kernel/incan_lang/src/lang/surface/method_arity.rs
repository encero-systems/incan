//! How many arguments each builtin list, dict and set method (frozen or not) takes.
//!
//! These methods have no declared signature: the checker types them from the method registry
//! ([`super::methods`]), so this table is the one place their argument count is stated. Every argument is positional;
//! a call with another count, or with a keyword argument, is refused.

use super::methods::dict_methods::{self, DictMethodId};
use super::methods::frozen_dict_methods::{self, FrozenDictMethodId};
use super::methods::frozen_list_methods::{self, FrozenListMethodId};
use super::methods::frozen_set_methods::{self, FrozenSetMethodId};
use super::methods::list_methods::{self, ListMethodId};
use super::methods::set_methods::{self, SetMethodId};
use crate::lang::types::collections::CollectionTypeId;

/// Return how many positional arguments `method` takes on a builtin `collection`, or `None` when the collection has no
/// registry method of that name.
pub fn builtin_collection_method_arity(collection: CollectionTypeId, method: &str) -> Option<usize> {
    match collection {
        CollectionTypeId::List => list_methods::from_str(method).map(list_method_arity),
        CollectionTypeId::Dict => dict_methods::from_str(method).map(dict_method_arity),
        CollectionTypeId::Set => set_methods::from_str(method).map(set_method_arity),
        CollectionTypeId::FrozenList => frozen_list_methods::from_str(method).map(frozen_list_method_arity),
        CollectionTypeId::FrozenDict => frozen_dict_methods::from_str(method).map(frozen_dict_method_arity),
        CollectionTypeId::FrozenSet => frozen_set_methods::from_str(method).map(frozen_set_method_arity),
        _ => None,
    }
}

/// Return how many arguments a list method takes.
fn list_method_arity(id: ListMethodId) -> usize {
    match id {
        ListMethodId::Clone | ListMethodId::Pop => 0,
        ListMethodId::Append
        | ListMethodId::Extend
        | ListMethodId::Contains
        | ListMethodId::Reserve
        | ListMethodId::ReserveExact
        | ListMethodId::Remove
        | ListMethodId::Count
        | ListMethodId::Index => 1,
        ListMethodId::Swap => 2,
    }
}

/// Return how many arguments a dict method takes.
fn dict_method_arity(id: DictMethodId) -> usize {
    match id {
        DictMethodId::Keys | DictMethodId::Values => 0,
        DictMethodId::Get | DictMethodId::ContainsKey => 1,
        DictMethodId::Insert => 2,
    }
}

/// Return how many arguments a set method takes.
fn set_method_arity(id: SetMethodId) -> usize {
    match id {
        SetMethodId::Add | SetMethodId::Contains => 1,
    }
}

/// Return how many arguments a frozen list method takes.
fn frozen_list_method_arity(id: FrozenListMethodId) -> usize {
    match id {
        FrozenListMethodId::Len | FrozenListMethodId::IsEmpty => 0,
    }
}

/// Return how many arguments a frozen dict method takes.
fn frozen_dict_method_arity(id: FrozenDictMethodId) -> usize {
    match id {
        FrozenDictMethodId::Len | FrozenDictMethodId::IsEmpty => 0,
        FrozenDictMethodId::ContainsKey => 1,
    }
}

/// Return how many arguments a frozen set method takes.
fn frozen_set_method_arity(id: FrozenSetMethodId) -> usize {
    match id {
        FrozenSetMethodId::Len | FrozenSetMethodId::IsEmpty => 0,
        FrozenSetMethodId::Contains => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_collection_method_has_an_arity() -> Result<(), String> {
        let families: [(CollectionTypeId, Vec<&str>); 6] = [
            (
                CollectionTypeId::List,
                list_methods::LIST_METHODS.iter().map(|m| m.canonical).collect(),
            ),
            (
                CollectionTypeId::Dict,
                dict_methods::DICT_METHODS.iter().map(|m| m.canonical).collect(),
            ),
            (
                CollectionTypeId::Set,
                set_methods::SET_METHODS.iter().map(|m| m.canonical).collect(),
            ),
            (
                CollectionTypeId::FrozenList,
                frozen_list_methods::FROZEN_LIST_METHODS
                    .iter()
                    .map(|m| m.canonical)
                    .collect(),
            ),
            (
                CollectionTypeId::FrozenDict,
                frozen_dict_methods::FROZEN_DICT_METHODS
                    .iter()
                    .map(|m| m.canonical)
                    .collect(),
            ),
            (
                CollectionTypeId::FrozenSet,
                frozen_set_methods::FROZEN_SET_METHODS
                    .iter()
                    .map(|m| m.canonical)
                    .collect(),
            ),
        ];
        for (collection, methods) in families {
            for method in methods {
                assert!(
                    builtin_collection_method_arity(collection, method).is_some(),
                    "{collection:?}.{method} has no arity"
                );
            }
        }
        assert_eq!(builtin_collection_method_arity(CollectionTypeId::Dict, "get"), Some(1));
        assert_eq!(
            builtin_collection_method_arity(CollectionTypeId::List, "remove"),
            Some(1)
        );
        assert_eq!(builtin_collection_method_arity(CollectionTypeId::Tuple, "get"), None);
        Ok(())
    }
}
