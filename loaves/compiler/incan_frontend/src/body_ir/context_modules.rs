//! The Body IR modules one module is lowered against, borrowed where their owners keep them.

use incan_semantics_core::body_ir as bir;

/// Every module of a [`ContextModules`] view in order: the loaded modules, then the borrowed ones.
pub type ContextModulesIter<'a> = std::iter::Chain<
    std::slice::Iter<'a, bir::BodyIrModule>,
    std::iter::Copied<std::slice::Iter<'a, &'a bir::BodyIrModule>>,
>;

/// The Body IR modules a module is lowered against: a run of modules kept together, then modules kept elsewhere.
///
/// A compilation lowers each source component against the same dependency Loaves and against the source modules the
/// component imports. The view reads both where they are kept, so a component never copies the context it reads.
#[derive(Clone, Copy, Default)]
pub struct ContextModules<'a> {
    loaded: &'a [bir::BodyIrModule],
    borrowed: &'a [&'a bir::BodyIrModule],
}

impl<'a> ContextModules<'a> {
    /// View `loaded` followed by `borrowed`.
    pub fn new(loaded: &'a [bir::BodyIrModule], borrowed: &'a [&'a bir::BodyIrModule]) -> Self {
        Self { loaded, borrowed }
    }

    /// Every module in order: the loaded modules, then the borrowed ones.
    pub fn iter(self) -> ContextModulesIter<'a> {
        self.loaded.iter().chain(self.borrowed.iter().copied())
    }
}

impl<'a> IntoIterator for ContextModules<'a> {
    type Item = &'a bir::BodyIrModule;
    type IntoIter = ContextModulesIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> From<&'a [bir::BodyIrModule]> for ContextModules<'a> {
    fn from(loaded: &'a [bir::BodyIrModule]) -> Self {
        Self { loaded, borrowed: &[] }
    }
}

impl<'a> From<&'a Vec<bir::BodyIrModule>> for ContextModules<'a> {
    fn from(loaded: &'a Vec<bir::BodyIrModule>) -> Self {
        Self::from(loaded.as_slice())
    }
}

impl<'a, const N: usize> From<&'a [bir::BodyIrModule; N]> for ContextModules<'a> {
    fn from(loaded: &'a [bir::BodyIrModule; N]) -> Self {
        Self::from(loaded.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &str) -> bir::BodyIrModule {
        bir::BodyIrModule::declarations_only(name.to_string())
    }

    #[test]
    fn a_view_reads_its_modules_in_place_and_in_order() {
        let loaded = vec![module("dependency")];
        let imported = module("imported");
        let built = module("built");
        let borrowed = [&imported, &built];
        let context = ContextModules::new(&loaded, &borrowed);

        let read = context.iter().collect::<Vec<_>>();

        assert_eq!(read.len(), 3);
        assert!(std::ptr::eq(read[0], &loaded[0]));
        assert!(std::ptr::eq(read[1], &imported));
        assert!(std::ptr::eq(read[2], &built));
    }
}
