//! Shared on-disk fixtures for `rust_inspect` tests (typechecker + extractor).

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

/// Minimal crate with a hyphenated package name and underscored lib name.
pub(crate) fn write_hyphenated_function_probe_crate(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "foo-bar"
version = "0.1.0"
edition = "2021"

[lib]
name = "foo_bar"
"#,
    )?;
    fs::write(
        root.join("src/lib.rs"),
        r#"pub struct State;
pub struct Plan;

pub mod consumer {
    pub mod nested {
        pub async fn consume(_state: &super::super::State, _plan: &super::super::Plan) {}
    }

    pub use nested::consume;
}
"#,
    )?;
    Ok(())
}

/// Minimal crate exposing an async free function that returns `Result<ConcreteType, Error>`.
pub(crate) fn write_async_result_probe_crate(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "ra_async_result_probe"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        root.join("src/lib.rs"),
        r#"pub struct State;
pub struct Plan;
pub struct LogicalPlan;
pub struct ConsumerError;
pub struct SessionContext;

impl SessionContext {
    pub fn new() -> SessionContext {
        SessionContext
    }

    pub fn state(&self) -> State {
        State
    }
}

pub async fn consume(_state: &State, _plan: &Plan) -> Result<LogicalPlan, ConsumerError> {
    Err(ConsumerError)
}
"#,
    )?;
    Ok(())
}

/// Minimal crate exposing nested borrowed function params through local imported type names.
pub(crate) fn write_borrowed_param_probe_crate(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("substrait").join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "ra_borrowed_param_probe"
version = "0.1.0"
edition = "2021"

[dependencies]
substrait = { path = "substrait" }
"#,
    )?;
    fs::write(
        root.join("substrait").join("Cargo.toml"),
        r#"[package]
name = "substrait"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        root.join("substrait").join("src/lib.rs"),
        r#"pub mod proto {
    pub struct Plan;
}
"#,
    )?;
    fs::write(
        root.join("src/lib.rs"),
        r#"pub mod execution {
    pub mod session_state {
        pub struct SessionState;
    }
}

pub mod logical_plan {
    pub mod consumer {
        pub mod plan {
            use crate::execution::session_state::SessionState;
            use substrait::proto::Plan;

            pub async fn consume(_state: &SessionState, _plan: &Plan) {}
        }

        pub use plan::consume;
    }
}
"#,
    )?;
    Ok(())
}
