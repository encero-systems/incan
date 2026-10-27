//! The unit description: what the compiler knows about one unit, as Oven describes it (DD-0008).
//!
//! Oven reads a project's manifest once and describes each unit it compiles, checks or analyzes. The compiler reads
//! no manifest: everything a session needs about the project arrives here.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CliError, CliResult};

/// The schema of a unit description.
pub const UNIT_DESCRIPTION_SCHEMA: &str = "incan.unit/1";

/// The schema of a toolchain generation's search index, which a unit description names.
pub const SEARCH_INDEX_SCHEMA: &str = "incan.search-index/1";

/// The entry contract of a compilation unit; all kinds share source analysis and Body IR lowering.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitKind {
    /// A native executable with an authored main.
    #[default]
    Program,
    /// A Loaf exporting its declarations as an rlib.
    Library,
    /// A native test executable whose entries come from session test collection.
    Test,
}

impl UnitKind {
    /// The kind spelling shared with Incan lowering and Oven's unit description.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Program => "program",
            Self::Library => "library",
            Self::Test => "test",
        }
    }
}

/// One unit: its source, its package and every Loaf it reaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitDescription {
    /// Always [`UNIT_DESCRIPTION_SCHEMA`].
    pub schema: String,
    /// The unit's entry source: a program's entry file, a Loaf's `lib.incn` or a test module.
    pub source: PathBuf,
    /// The entry contract; descriptions predating explicit kinds describe programs.
    #[serde(default)]
    pub kind: UnitKind,
    /// Additional program entry sources, in selector order. Each must be a collected source module with a main.
    #[serde(default)]
    pub program_entries: Vec<PathBuf>,
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
    /// Producer registration prepared by Oven, in the existing versioned vocabulary descriptor format.
    #[serde(default)]
    pub vocab: Option<PathBuf>,
    /// Project-relative authored entry, used to check the registration export in a vocabulary unit.
    #[serde(default)]
    pub vocab_entry: Option<PathBuf>,
    /// This native unit registers vocabulary and must not activate its own surfaces through its imports.
    #[serde(default)]
    pub vocabulary: bool,
    /// Exact vocabulary worker unit identities admitted after Oven locks the resolved provider artifacts.
    #[serde(default)]
    pub vocab_admissions: Vec<String>,
    /// Policy that admitted or denied the vocabulary identities in this unit's resolved provider lock.
    #[serde(default = "default_vocab_policy")]
    pub vocab_policy: String,
    /// This vocabulary unit is the generated adapter for the required public desugar export.
    #[serde(default)]
    pub vocab_desugarer: bool,
    /// The exact Rust libraries this unit links, including its toolchain and reached Loaves.
    pub externs: Vec<UnitNativeCrate>,
    /// Directories containing the transitive dependencies of the named libraries. Once read, this also holds the
    /// directories of the search index, sorted by their bytes without repeats.
    pub search: Vec<PathBuf>,
    /// The toolchain generation's search index (`incan.search-index/1`): the directories of the toolchain's crates
    /// and of its compiled registry, published once by Oven and named by the SHA-256 of its content. The description
    /// lists in `search` only the directories beyond it.
    #[serde(default)]
    pub search_index: Option<PathBuf>,
    /// Rust ABI caches supplied by the toolchain for this unit.
    pub rust_abi: Vec<PathBuf>,
    /// The `search` directories the description itself lists, without the index's. A library unit's native closure
    /// publishes these; a consumer of the same toolchain generation adds the index through its own description.
    #[serde(skip)]
    pub own_search: Vec<PathBuf>,
}

/// A toolchain generation's search index, as Oven publishes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchIndex {
    /// Always [`SEARCH_INDEX_SCHEMA`].
    schema: String,
    /// The directories every unit of the generation searches.
    search: Vec<PathBuf>,
}

/// Preserve the default policy when reading unit descriptions written before explicit provider evidence.
fn default_vocab_policy() -> String {
    "locked-dependencies".into()
}

/// One exact native artifact bound to the crate alias the unit names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitNativeCrate {
    /// The Rust crate alias, with hyphens normalized to underscores.
    pub name: String,
    /// The library artifact; the compiler never chooses it from a directory.
    pub path: PathBuf,
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
    /// Optional authored source root selected by Oven; editor locations never infer it from artifact placement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<PathBuf>,
}

impl UnitDescription {
    /// Read a unit description Oven wrote.
    pub fn read(path: &Path) -> CliResult<Self> {
        let bytes = fs::read(path).map_err(|error| {
            CliError::failure(format!("cannot read the unit description {}: {error}", path.display()))
        })?;
        Self::parse(&bytes, path)
    }

    /// Decode and validate a description from Oven, retaining its origin in schema and registration failures.
    /// Every client receives the same search-index merge and prepared-vocabulary admission checks.
    pub fn parse(bytes: &[u8], origin: &Path) -> CliResult<Self> {
        let mut unit: Self = serde_json::from_slice(bytes)
            .map_err(|error| CliError::failure(format!("{} is not a unit description: {error}", origin.display())))?;
        if unit.schema != UNIT_DESCRIPTION_SCHEMA {
            return Err(CliError::failure(format!(
                "{} has schema `{}`; this compiler reads `{UNIT_DESCRIPTION_SCHEMA}`",
                origin.display(),
                unit.schema
            )));
        }
        unit.merge_search_index()?;
        crate::loaf_publication::vocab::prepared_registration(&unit)?;
        Ok(unit)
    }

    /// Keep the description's own directories in `own_search` and merge the search index it names into `search`.
    /// The merged list is sorted by its bytes without repeats, the order Oven writes a description's directories in.
    /// [`UnitDescription::read`] does this; a caller that decodes a description itself does it once afterward.
    pub fn merge_search_index(&mut self) -> CliResult<()> {
        self.own_search = self.search.clone();
        let Some(index) = &self.search_index else {
            return Ok(());
        };
        let mut merged = read_search_index(index)?;
        merged.extend(self.own_search.iter().cloned());
        merged.sort_by(|left, right| {
            left.as_os_str()
                .as_encoded_bytes()
                .cmp(right.as_os_str().as_encoded_bytes())
        });
        merged.dedup();
        self.search = merged;
        Ok(())
    }
}

/// Read a search index, refusing one whose bytes are not those its name, their SHA-256, promises.
fn read_search_index(path: &Path) -> CliResult<Vec<PathBuf>> {
    let bytes = fs::read(path)
        .map_err(|error| CliError::failure(format!("cannot read the search index {}: {error}", path.display())))?;
    let named = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
    if hex::encode(Sha256::digest(&bytes)) != named {
        return Err(CliError::failure(format!(
            "the search index {} does not match the digest it is named by",
            path.display()
        )));
    }
    let index: SearchIndex = serde_json::from_slice(&bytes)
        .map_err(|error| CliError::failure(format!("{} is not a search index: {error}", path.display())))?;
    if index.schema != SEARCH_INDEX_SCHEMA {
        return Err(CliError::failure(format!(
            "{} has schema `{}`; this compiler reads `{SEARCH_INDEX_SCHEMA}`",
            path.display(),
            index.schema
        )));
    }
    Ok(index.search)
}
