//! The layouts of the transitive type and trait tables.
//!
//! A layout is either built when it is installed, or deferred: a compiled Loaf's declaration and the remappings its
//! install applies, built the first time a lookup reads it. A compiled Loaf a checker installs therefore costs the
//! layouts its lookups read, not one per declaration. Every table position and every checker that installed one
//! deferred layout shares its single build.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use super::{TraitInfo, TypeChecker, TypeInfo};
use crate::symbols::SymbolKind;

/// The compiled declaration a deferred layout is built from.
#[derive(Debug)]
pub(in crate::typechecker) enum LayoutDeclaration {
    /// The model at this position of the manifest's root exports.
    Model(usize),
    /// The class at this position of the manifest's root exports.
    Class(usize),
    /// The enum at this position of the manifest's root exports.
    Enum(usize),
    /// The newtype at this position of the manifest's root exports.
    Newtype(usize),
    /// The trait at this position of the manifest's root exports.
    Trait(usize),
    /// The model, class, enum or newtype the manifest's root exports under this public name.
    Export(String),
    /// The checked API declaration at this position of this checked API module.
    Api { module: usize, declaration: usize },
    /// The checked API declaration at this target path.
    ApiPath(Vec<String>),
}

/// How to build one compiled declaration's layout as its install would: the declaration, then each remapping in
/// order, then the compiled class field provider.
#[derive(Debug)]
pub(in crate::typechecker) struct LayoutRecipe {
    /// The admitted Loaf whose layout this recipe will read.
    pub(in crate::typechecker) library: String,
    pub(in crate::typechecker) declaration: LayoutDeclaration,
    pub(in crate::typechecker) remappings: Vec<Arc<HashMap<String, String>>>,
    /// The Loaf a compiled class's fields are marked as provided by, when the install marks them.
    pub(in crate::typechecker) field_provider: Option<String>,
}

/// A table value a layout's symbol kind holds.
pub(in crate::typechecker) trait LayoutValue: Sized {
    /// The value of `kind`, when it is this table's kind.
    fn of_kind(kind: SymbolKind) -> Option<Self>;
}

impl LayoutValue for TypeInfo {
    fn of_kind(kind: SymbolKind) -> Option<Self> {
        match kind {
            SymbolKind::Type(info) => Some(info),
            _ => None,
        }
    }
}

impl LayoutValue for TraitInfo {
    fn of_kind(kind: SymbolKind) -> Option<Self> {
        match kind {
            SymbolKind::Trait(info) => Some(info),
            _ => None,
        }
    }
}

struct LayoutCell<T> {
    value: OnceLock<Option<T>>,
    recipe: Option<LayoutRecipe>,
}

/// One layout of a transitive table.
pub struct TransitiveLayout<T>(Arc<LayoutCell<T>>);

impl<T> Clone for TransitiveLayout<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for TransitiveLayout<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.value.get() {
            Some(value) => value.fmt(formatter),
            None => formatter.write_str("deferred layout"),
        }
    }
}

impl<T: LayoutValue> TransitiveLayout<T> {
    /// A layout already built.
    pub fn ready(value: T) -> Self {
        Self(Arc::new(LayoutCell {
            value: OnceLock::from(Some(value)),
            recipe: None,
        }))
    }

    /// A layout built from `recipe` the first time it is read.
    pub(in crate::typechecker) fn deferred(recipe: LayoutRecipe) -> Self {
        Self(Arc::new(LayoutCell {
            value: OnceLock::new(),
            recipe: Some(recipe),
        }))
    }

    /// The layout, built by `checker` the first time it is read. `None` only for a recipe whose declaration is not of
    /// this table's kind or whose admitted layout input could not be read.
    pub fn get(&self, checker: &TypeChecker) -> Option<&T> {
        self.0
            .value
            .get_or_init(|| {
                let recipe = self.0.recipe.as_ref()?;
                T::of_kind(checker.build_deferred_layout(recipe)?)
            })
            .as_ref()
    }
}

/// The values of every layout in `table`, each built if it was deferred.
pub(in crate::typechecker) fn built_table<T: LayoutValue + Clone>(
    table: &HashMap<String, Vec<TransitiveLayout<T>>>,
    checker: &TypeChecker,
) -> HashMap<String, Vec<T>> {
    table
        .iter()
        .map(|(name, layouts)| {
            let values = layouts
                .iter()
                .filter_map(|layout| layout.get(checker).cloned())
                .collect();
            (name.clone(), values)
        })
        .collect()
}

/// `values` as built layouts.
pub(in crate::typechecker) fn ready_layouts<T: LayoutValue + Clone>(values: &[T]) -> Vec<TransitiveLayout<T>> {
    values.iter().cloned().map(TransitiveLayout::ready).collect()
}

/// `table` with every value as a built layout.
pub(in crate::typechecker) fn ready_tables<T: LayoutValue>(
    table: HashMap<String, Vec<T>>,
) -> HashMap<String, Vec<TransitiveLayout<T>>> {
    table
        .into_iter()
        .map(|(name, values)| (name, values.into_iter().map(TransitiveLayout::ready).collect()))
        .collect()
}
