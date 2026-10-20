//! The unit description: what the compiler knows about one unit, as Oven describes it (DD-0008).
//!
//! Oven reads a project's manifest once and describes each unit it compiles, checks or analyzes. The compiler reads
//! no manifest: everything a session needs about the project arrives here.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CliError, CliResult};

/// The schema of a unit description.
pub const UNIT_DESCRIPTION_SCHEMA: &str = "incan.unit/1";

/// One unit: its source, its package and every Loaf it reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitDescription {
    /// Always [`UNIT_DESCRIPTION_SCHEMA`].
    pub schema: String,
    /// The unit's entry source: a program's entry file, a Loaf's `lib.incn` or a test module.
    pub source: PathBuf,
    /// The package's root directory, which package-relative paths are spelled against.
    pub project_root: PathBuf,
    /// The package's source root, which module paths are spelled against.
    pub source_root: PathBuf,
    /// The package name, which is also the package identity of a library unit's declarations.
    pub package: String,
    /// The package version.
    pub version: String,
    /// The build profile whose Loaf artifacts the unit reads.
    pub profile: String,
    /// Every feature the package declares, which `when feature(...)` may name.
    pub declared_features: BTreeSet<String>,
    /// The features active for this unit.
    pub active_features: BTreeSet<String>,
    /// Every Loaf the unit reaches, transitive ones and the standard packages included (DD-0007).
    pub loaves: Vec<UnitLoaf>,
    /// The Rust crates the package declares, which `rust::` imports may name.
    pub rust_crates: BTreeSet<String>,
    /// The package's contract model bundle files, package-relative.
    pub contract_models: Vec<String>,
    /// The C headers the package declares, package-relative, which checked C bindings may name.
    pub c_headers: Vec<String>,
}

/// One Loaf a unit reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitLoaf {
    /// The key the unit names it by: its dependency key, or a standard package's name.
    pub key: String,
    /// The Loaf's package name, which names its artifact files.
    pub name: String,
    /// Its artifact directory for the unit's profile.
    pub dir: PathBuf,
    /// Whether it is a standard package, which owns `std.*` roots rather than `pub::<key>`.
    pub standard: bool,
}

impl UnitDescription {
    /// Read a unit description Oven wrote.
    pub fn read(path: &Path) -> CliResult<Self> {
        let bytes = fs::read(path).map_err(|error| {
            CliError::failure(format!("cannot read the unit description {}: {error}", path.display()))
        })?;
        let unit: Self = serde_json::from_slice(&bytes)
            .map_err(|error| CliError::failure(format!("{} is not a unit description: {error}", path.display())))?;
        if unit.schema != UNIT_DESCRIPTION_SCHEMA {
            return Err(CliError::failure(format!(
                "{} has schema `{}`; this compiler reads `{UNIT_DESCRIPTION_SCHEMA}`",
                path.display(),
                unit.schema
            )));
        }
        Ok(unit)
    }
}
