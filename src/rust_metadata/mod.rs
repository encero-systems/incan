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
pub(crate) use test_fixtures::write_substrait_probe_crate;

pub use cache::RustMetadataCache;
pub use error::RustMetadataError;
pub use extractor::extract_rust_item;
pub use loader::RustWorkspace;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

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

[dependencies]
regex = "1"
hashbrown = "0.15"
"#,
        )?;
        fs::write(root.join("src/lib.rs"), "// probe crate for rust metadata tests\n")?;
        Ok(())
    }

    fn method_names(meta: &incan_core::interop::RustItemMetadata) -> Vec<String> {
        match &meta.kind {
            incan_core::interop::RustItemKind::Type(info) => info.methods.iter().map(|m| m.name.clone()).collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn hashmap_has_expected_public_methods() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        // Sysroot `std` is not always registered under the display name `std` in minimal workspaces;
        // `hashbrown::HashMap` is a normal dependency with the same public map surface we care about.
        let meta = cache.get_or_extract(tmp.path(), "hashbrown::HashMap", &|_| ())?;
        let names = method_names(&meta);
        for required in ["insert", "get", "len", "contains_key"] {
            assert!(
                names.iter().any(|n| n == required),
                "expected inherent method `{required}` on HashMap, have {:?}",
                names
            );
        }
        Ok(())
    }

    #[test]
    fn regex_type_exposes_core_methods() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_probe_crate(tmp.path())?;
        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(tmp.path(), "regex::Regex", &|_| ())?;
        let names = method_names(&meta);
        for required in ["new", "is_match", "find"] {
            assert!(
                names.iter().any(|n| n == required),
                "expected method `{required}` on regex::Regex, have {:?}",
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
        let a = cache.get_or_extract(tmp.path(), "regex::Regex", &|_| ())?;
        let b = cache.get_or_extract(tmp.path(), "regex::Regex", &|_| ())?;
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
}
