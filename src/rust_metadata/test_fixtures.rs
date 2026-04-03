//! Shared on-disk fixtures for `rust-metadata` tests (typechecker + extractor).

use std::fs;
use std::path::Path;

/// Minimal “prost-style” crate mirroring oneof/enum shapes used by Substrait protos.
pub(crate) fn write_substrait_probe_crate(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("substrait").join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "ra_substrait_probe"
version = "0.1.0"
edition = "2021"

[dependencies]
substrait = { path = "substrait" }
"#,
    )?;
    fs::write(
        root.join("src/lib.rs"),
        "pub fn touch() { let _ = substrait::proto::PlanRel; }\n",
    )?;
    fs::write(
        root.join("substrait").join("Cargo.toml"),
        r#"[package]
name = "substrait"
version = "0.63.0"
edition = "2021"
"#,
    )?;
    fs::write(
        root.join("substrait").join("src/lib.rs"),
        r#"pub mod proto {
    pub struct PlanRel;

    pub struct Rel {
        pub rel_type: std::option::Option<rel::RelType>,
    }

    pub struct ReadRel {
        pub read_type: std::option::Option<read_rel::ReadType>,
    }

    pub mod rel {
        pub enum RelType {
            Read(Box<super::ReadRel>),
        }
    }

    pub mod read_rel {
        pub struct NamedTable;

        pub enum ReadType {
            NamedTable(Box<NamedTable>),
        }
    }
}
"#,
    )?;
    Ok(())
}
