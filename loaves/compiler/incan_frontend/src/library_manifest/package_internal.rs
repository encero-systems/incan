//! The package-internal declaration family of a project's shared library (DD-0007).
//!
//! A project's shared library holds every module of its source root but its tests and root entry files, and only the
//! units of that same project link it: its programs and its tests. Those units are the library's own package, and a
//! caller in another module of the same package receives every default, private name and field a whole-program
//! compile gives it. The public family of the library's manifest carries what another package may see, as for any
//! Loaf; this family carries, beside it, what only the same package may see. It is written only for a project's shared
//! library and opened only by a unit that names that library as its project library, so another package's view of any
//! Loaf is unchanged, and a manifest without it is byte for byte what it was before the family existed.

use serde::{Deserialize, Serialize};

/// Stable schema version of the package-internal section.
pub const PACKAGE_INTERNAL_SCHEMA_VERSION: u32 = 1;

/// What a project's shared library publishes for the units of its own package alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageInternalExports {
    pub schema_version: u32,
}

impl PackageInternalExports {
    /// The section a project's shared library writes, before any package-internal fact is added to it.
    pub fn new() -> Self {
        Self {
            schema_version: PACKAGE_INTERNAL_SCHEMA_VERSION,
        }
    }

    /// Whether a consuming unit may open the package-internal section of the library named `library`: only a unit
    /// whose project library is that library, which is to say a unit of the same package.
    pub fn opened_by(consumer_project_library: Option<&str>, library: &str) -> bool {
        consumer_project_library == Some(library)
    }
}

impl Default for PackageInternalExports {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_manifest::LibraryManifest;

    #[test]
    fn a_manifest_without_the_package_internal_section_serializes_as_before() {
        let manifest = LibraryManifest::new("provider", "0.1.0");
        let text = manifest.to_json_string().unwrap_or_default();
        assert!(!text.is_empty());
        assert!(!text.contains("package_internal"), "{text}");
        let read = LibraryManifest::from_json_str(&text).map(|value| value.package_internal);
        assert!(matches!(read, Ok(None)));
    }

    #[test]
    fn a_project_library_section_round_trips_and_leaves_the_public_part_unchanged() {
        let public = LibraryManifest::new("demo_project_library", "0.1.0");
        let mut internal = public.clone();
        internal.package_internal = Some(PackageInternalExports::new());
        let text = internal.to_json_string().unwrap_or_default();
        assert!(text.contains("package_internal"), "{text}");
        let read = LibraryManifest::from_json_str(&text);
        assert!(matches!(&read, Ok(value) if value.package_internal == Some(PackageInternalExports::new())));
        let mut without = read.unwrap_or_else(|_| public.clone());
        without.package_internal = None;
        assert_eq!(without.to_json_string().ok(), public.to_json_string().ok());
    }

    #[test]
    fn only_a_unit_of_the_same_package_opens_the_section() {
        let library = "demo_project_library";
        let opened = [Some(library), None, Some("other_project_library")]
            .iter()
            .filter(|consumer| PackageInternalExports::opened_by(**consumer, library))
            .count();
        assert_eq!(opened, 1);
    }

    /// Check `source` as the module `helpers` of the project library `demo_project_library` and collect its exports.
    fn checked_exports(source: &str) -> Vec<crate::library_exports::CheckedNamedExport> {
        let tokens = crate::lexer::lex(source).unwrap_or_default();
        let program = crate::parser::parse(&tokens).ok();
        let mut checker = crate::typechecker::TypeChecker::new();
        checker.set_current_package_identity(Some("demo_project_library".into()));
        checker.set_current_module_path(Some(vec!["helpers".to_owned()]));
        let Some(program) = program.filter(|program| checker.check_program(program).is_ok()) else {
            return Vec::new();
        };
        crate::library_exports::collect_checked_public_exports(&program, &checker)
    }

    #[test]
    fn a_project_library_lists_each_default_its_public_family_leaves_required_and_no_other() {
        let exports = checked_exports(
            "pub static ITEMS: list[int] = [1, 2]\n\n\npub def has(flag: bool = ITEMS.contains(2)) -> bool:\n    return flag\n\n\npub def size(n: int = len(ITEMS)) -> int:\n    return n\n\n\npub def take(n: int = 3, m: int = 4) -> int:\n    return n + m\n",
        );
        assert!(!exports.is_empty());
        let section = PackageInternalExports::new().with_source_defaults("demo_project_library", &exports);
        let listed = section
            .source_defaults
            .iter()
            .map(|default| (default.callable.declaration_name.as_str(), default.parameter))
            .collect::<Vec<_>>();
        assert_eq!(listed, [("has", 0), ("size", 0)]);
        let has = exports
            .iter()
            .find(|export| export.name == "has")
            .and_then(|export| export.identity.canonical.clone());
        assert!(has.is_some_and(|identity| section.omittable_parameters(&identity).count() == 1));
    }

    #[test]
    fn a_section_without_source_defaults_serializes_as_the_empty_frame() {
        let mut manifest = LibraryManifest::new("demo_project_library", "0.1.0");
        manifest.package_internal = Some(PackageInternalExports::new());
        let text = manifest.to_json_string().unwrap_or_default();
        assert!(!text.contains("source_defaults"), "{text}");
    }
}
