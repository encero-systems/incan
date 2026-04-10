//! Load Cargo workspaces with rust-analyzer and extract [`incan_core::interop::RustItemMetadata`].
//!
//! This module is behind the `rust-metadata` feature so default builds avoid the heavy `ra_ap_*` dependency stack (RFC
//! 041).

mod cache;
mod error;
mod extractor;
mod loader;

#[cfg(test)]
mod test_fixtures;

#[cfg(test)]
pub(crate) use test_fixtures::{
    write_async_result_probe_crate, write_hyphenated_function_probe_crate, write_nested_context_probe_crate,
    write_reexported_function_probe_crate, write_substrait_probe_crate,
};

pub use cache::RustMetadataCache;
pub use error::RustMetadataError;
pub use extractor::extract_rust_item;
pub use loader::RustWorkspace;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use crate::backend::project::ProjectGenerator;
    use crate::manifest::{DependencySource, DependencySpec};

    use super::*;
    use incan_core::interop::{RustItemKind, RustTypeShape};

    fn type_mismatch(msg: impl std::fmt::Display) -> Box<dyn std::error::Error> {
        std::io::Error::other(msg.to_string()).into()
    }

    fn write_probe_crate(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        fs::create_dir_all(root.join("src"))?;
        fs::write(
            root.join("Cargo.toml"),
            r#"[package]
name = "ra_metadata_probe"
version = "0.1.0"
edition = "2021"
"#,
        )?;
        fs::write(
            root.join("src/lib.rs"),
            r#"pub struct ProbeType;

impl ProbeType {
    pub fn new() -> Self {
        Self
    }

    pub fn is_match(&self, needle: &str) -> bool {
        needle == "match"
    }

    pub fn find(&self, haystack: &str) -> Option<usize> {
        haystack.find("match")
    }

    pub fn len(&self) -> usize {
        1
    }
}
"#,
        )?;
        Ok(())
    }

    fn method_names(meta: &incan_core::interop::RustItemMetadata) -> Vec<String> {
        match &meta.kind {
            incan_core::interop::RustItemKind::Type(info) => info.methods.iter().map(|m| m.name.clone()).collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn probe_type_has_expected_public_methods() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(tmp.path(), "ra_metadata_probe::ProbeType", &|_| ())?;
        let names = method_names(&meta);
        for required in ["new", "is_match", "find", "len"] {
            assert!(
                names.iter().any(|n| n == required),
                "expected inherent method `{required}` on ProbeType, have {:?}",
                names
            );
        }
        Ok(())
    }

    #[test]
    fn probe_type_exposes_core_methods() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(tmp.path(), "ra_metadata_probe::ProbeType", &|_| ())?;
        let names = method_names(&meta);
        for required in ["new", "is_match", "find"] {
            assert!(
                names.iter().any(|n| n == required),
                "expected method `{required}` on ProbeType, have {:?}",
                names
            );
        }
        Ok(())
    }

    #[test]
    fn cache_returns_same_arc_without_second_load() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        let a = cache.get_or_extract(tmp.path(), "ra_metadata_probe::ProbeType", &|_| ())?;
        let b = cache.get_or_extract(tmp.path(), "ra_metadata_probe::ProbeType", &|_| ())?;
        assert!(Arc::ptr_eq(&a, &b));
        Ok(())
    }

    #[test]
    fn substrait_rel_field_preserves_concrete_oneof_payload_type() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_substrait_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(tmp.path(), "substrait::proto::Rel", &|_| ())?;
        let info = match &meta.kind {
            RustItemKind::Type(info) => info,
            other => {
                return Err(type_mismatch(format!(
                    "expected `substrait::proto::Rel` metadata to be a type, got {other:?}"
                )));
            }
        };
        let rel_type = info
            .fields
            .iter()
            .find(|field| field.name == "rel_type")
            .ok_or_else(|| {
                type_mismatch(format!(
                    "expected `rel_type` field on substrait::proto::Rel, got {:?}",
                    info.fields
                ))
            })?;
        assert_eq!(
            rel_type.type_shape,
            RustTypeShape::Option(Box::new(RustTypeShape::RustPath {
                path: "substrait::proto::rel::RelType".to_string(),
                args: vec![],
            })),
            "expected concrete oneof payload type for Rel.rel_type, got {:?} (display: {})",
            rel_type.type_shape,
            rel_type.type_display
        );
        Ok(())
    }

    #[test]
    fn substrait_relative_variant_payloads_preserve_canonical_paths() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_substrait_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();

        let rel_type_meta = cache.get_or_extract(tmp.path(), "substrait::proto::rel::RelType", &|_| ())?;
        let rel_type_info = match &rel_type_meta.kind {
            RustItemKind::Type(info) => info,
            other => {
                return Err(type_mismatch(format!(
                    "expected `substrait::proto::rel::RelType` metadata to be a type, got {other:?}"
                )));
            }
        };
        assert_eq!(
            rel_type_info.variants[0].fields,
            vec![RustTypeShape::RustPath {
                path: "substrait::proto::ReadRel".to_string(),
                args: vec![],
            }],
            "expected `super::ReadRel` payload to normalize to the canonical path, got {:?}",
            rel_type_info.variants[0].fields
        );

        let read_type_meta = cache.get_or_extract(tmp.path(), "substrait::proto::read_rel::ReadType", &|_| ())?;
        let read_type_info = match &read_type_meta.kind {
            RustItemKind::Type(info) => info,
            other => {
                return Err(type_mismatch(format!(
                    "expected `substrait::proto::read_rel::ReadType` metadata to be a type, got {other:?}"
                )));
            }
        };
        assert_eq!(
            read_type_info.variants[0].fields,
            vec![RustTypeShape::RustPath {
                path: "substrait::proto::read_rel::NamedTable".to_string(),
                args: vec![],
            }],
            "expected bare `NamedTable` payload to normalize to the canonical path, got {:?}",
            read_type_info.variants[0].fields
        );
        Ok(())
    }

    #[test]
    fn reexported_function_paths_resolve_to_function_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_reexported_function_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(tmp.path(), "ra_reexport_probe::consumer::consume", &|_| ())?;
        match &meta.kind {
            RustItemKind::Function(sig) => {
                assert_eq!(sig.params.len(), 2);
                assert!(sig.params[0].type_display.starts_with('&'));
                assert!(sig.params[1].type_display.starts_with('&'));
                assert!(
                    sig.is_async,
                    "expected re-exported function metadata to preserve async-ness"
                );
            }
            other => {
                return Err(type_mismatch(format!(
                    "expected re-exported function metadata, got {other:?}"
                )));
            }
        }
        Ok(())
    }

    #[test]
    fn hyphenated_package_names_resolve_via_underscored_rust_paths() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_hyphenated_function_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(tmp.path(), "foo_bar::consumer::consume", &|_| ())?;
        match &meta.kind {
            RustItemKind::Function(sig) => {
                assert_eq!(sig.params.len(), 2);
                assert!(sig.params[0].type_display.starts_with('&'));
                assert!(sig.params[1].type_display.starts_with('&'));
                assert!(sig.is_async, "expected async metadata for hyphenated-package probe");
            }
            other => {
                return Err(type_mismatch(format!(
                    "expected function metadata for hyphenated-package probe, got {other:?}"
                )));
            }
        }
        Ok(())
    }

    #[test]
    fn generated_lock_projects_resolve_hyphenated_dependencies_via_rust_paths() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let dep_root = tmp.path().join("foo-bar-dep");
        write_hyphenated_function_probe_crate(&dep_root)?;

        let lock_root = tmp.path().join("generated_lock");
        let mut generator = ProjectGenerator::new(&lock_root, "lock_probe", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "foo-bar".to_string(),
            version: None,
            features: vec![],
            default_features: true,
            source: DependencySource::Path { path: dep_root.clone() },
            optional: false,
            package: None,
        }]);
        generator.generate("fn main() {}")?;

        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(&lock_root, "foo_bar::consumer::consume", &|_| ())?;
        match &meta.kind {
            RustItemKind::Function(sig) => {
                assert_eq!(sig.params.len(), 2);
                assert!(sig.params[0].type_display.starts_with('&'));
                assert!(sig.params[1].type_display.starts_with('&'));
                assert!(sig.is_async, "expected async metadata via generated lock project");
            }
            other => {
                return Err(type_mismatch(format!(
                    "expected function metadata via generated lock project, got {other:?}"
                )));
            }
        }
        Ok(())
    }

    #[test]
    fn nested_method_signatures_preserve_canonical_return_paths() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_nested_context_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(
            tmp.path(),
            "ra_context_probe::execution::context::SessionContext",
            &|_| (),
        )?;
        let info = match &meta.kind {
            RustItemKind::Type(info) => info,
            other => {
                return Err(type_mismatch(format!(
                    "expected type metadata for nested context probe, got {other:?}"
                )));
            }
        };
        let new_sig = info
            .methods
            .iter()
            .find(|method| method.name == "new")
            .ok_or_else(|| type_mismatch("expected `new` method on nested context probe"))?;
        let state_sig = info
            .methods
            .iter()
            .find(|method| method.name == "state")
            .ok_or_else(|| type_mismatch("expected `state` method on nested context probe"))?;

        assert_eq!(
            new_sig.signature.return_type,
            "ra_context_probe::execution::context::SessionContext"
        );
        assert_eq!(
            state_sig.signature.return_type,
            "ra_context_probe::execution::context::SessionContext"
        );
        Ok(())
    }

    #[test]
    fn async_function_signatures_preserve_result_output_types() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_async_result_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(tmp.path(), "ra_async_result_probe::consume", &|_| ())?;
        match &meta.kind {
            RustItemKind::Function(sig) => {
                assert!(sig.is_async, "expected async metadata for async result probe");
                assert_eq!(sig.params.len(), 2);
                assert_eq!(
                    sig.return_type,
                    "Result<ra_async_result_probe::LogicalPlan, ra_async_result_probe::ConsumerError>"
                );
            }
            other => {
                return Err(type_mismatch(format!(
                    "expected function metadata for async result probe, got {other:?}"
                )));
            }
        }
        Ok(())
    }

    #[test]
    fn generated_lock_projects_resolve_registry_dependencies_via_cargo_lock_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let registry_src_root = tmp.path().join("cargo-home").join("registry").join("src");
        let dep_root = registry_src_root.join("index.crates.io-test").join("foo-bar-0.1.0");
        write_hyphenated_function_probe_crate(&dep_root)?;

        let lock_root = tmp.path().join("generated_lock");
        let mut generator = ProjectGenerator::new(&lock_root, "lock_probe", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "foo-bar".to_string(),
            version: Some("0.1.0".to_string()),
            features: vec![],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }]);
        generator.set_cargo_lock_payload(Some(
            r#"version = 3

[[package]]
name = "lock_probe"
version = "0.1.0"
dependencies = ["foo-bar"]

[[package]]
name = "foo-bar"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#
            .to_string(),
        ));
        generator.generate("use foo_bar as _;\nfn main() {}\n")?;

        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract_with_registry_src_roots(
            &lock_root,
            "foo_bar::consumer::consume",
            std::slice::from_ref(&registry_src_root),
            &|_| (),
        )?;
        match &meta.kind {
            RustItemKind::Function(sig) => {
                assert_eq!(sig.params.len(), 2);
                assert!(
                    sig.params[0].type_display.starts_with('&'),
                    "expected borrowed first param, got {}",
                    sig.params[0].type_display
                );
                assert!(
                    sig.params[1].type_display.starts_with('&'),
                    "expected borrowed second param, got {}",
                    sig.params[1].type_display
                );
                assert!(sig.is_async, "expected async metadata for registry fallback probe");
            }
            other => {
                return Err(type_mismatch(format!(
                    "expected function metadata for registry fallback probe, got {other:?}"
                )));
            }
        }
        Ok(())
    }
}
