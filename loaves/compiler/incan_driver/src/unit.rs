//! The unit description: what the compiler knows about one unit, as Oven describes it (DD-0008).
//!
//! Oven reads a project's manifest once and describes each unit it compiles, checks or analyzes. The compiler reads
//! no manifest: everything a session needs about the project arrives here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
mod search_index;
use search_index::read_search_index;

use crate::error::{CliError, CliResult};

/// The schema of a unit description.
pub const UNIT_DESCRIPTION_SCHEMA: &str = "incan.unit/1";

/// The compatibility schema of JSON search indexes published by an earlier compiler generation.
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
    /// Further test modules a test unit compiles beside its entry module, in run order: the modules that share a plain
    /// session-scoped `conftest.incn` fixture with it, whose tests run in one process so the fixture is set up once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_batch: Vec<PathBuf>,
    /// The dependency key under which a test unit's `loaves` bind its project's shared library unit. That library
    /// holds every module of the source root but `excluded_sources` and the root entry files, so a project-local
    /// import of any of them names the library: the test unit compiles only its own modules, and the rest compile
    /// once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_library: Option<String>,
    /// Source files the project's shared library leaves out: its test modules. The library unit omits them from its
    /// module set, and a test unit naming `project_library` compiles them as its own modules.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_sources: Vec<PathBuf>,
    /// Check a library unit as its project's program is checked: a project's shared library is no published API, so
    /// it skips the publication-only API docstring check.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub program_checks: bool,
    /// The package's root directory, which package-relative paths are spelled against.
    pub project_root: PathBuf,
    /// The package's source root, which module paths are spelled against.
    pub source_root: PathBuf,
    /// The package name, which is also the package identity of a library unit's declarations.
    pub package: String,
    /// Registry declared by this package's own manifest. Absence does not synthesize a default root key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_id: Option<String>,
    /// The package version.
    pub version: String,
    /// The build profile whose Loaf artifacts the unit reads.
    pub profile: String,
    /// Source-independent semantic context of private callable products, owned and reconciled by Oven.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_context: Option<String>,
    /// Explicit private item-product directory for independently validated scalar declarations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_products: Option<PathBuf>,
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
    /// The Cloves (C-library Loaves, RFC 159) the unit reaches, each configuration compiled by Oven; a
    /// `c.loaf_library` binding verifies against one and links its archive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cloves: Vec<UnitClove>,
    /// The declared `[[interop.c.targets]]` entry checked C bindings verify against instead of the host
    /// (`--interop-target`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_abi_target: Option<UnitCAbiTarget>,
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
    /// Stable authority labels covering each selected Loaf whose compile-time code may run for this unit.
    #[serde(default)]
    pub covered_by: BTreeMap<String, String>,
    /// Selected Loaves unavailable to code-free analysis because preparing them would run untrusted code.
    #[serde(default)]
    pub not_prepared_loaves: Vec<String>,
    /// Stable missing-trust stop retained for editor presentation while code-free analysis continues.
    #[serde(default)]
    pub trust_stop: String,
    /// The exact Rust libraries this unit links, including its toolchain and reached Loaves.
    pub externs: Vec<UnitNativeCrate>,
    /// Directories containing the transitive dependencies of the named libraries. Once read, this also holds the
    /// directories of the search index, sorted by their bytes without repeats.
    pub search: Vec<PathBuf>,
    /// The toolchain generation's content-named search index (Crumb v2 or legacy JSON v1): the directories of the toolchain's crates
    /// and of its compiled registry, published once by Oven and named by the SHA-256 of its content. The description
    /// lists in `search` only the directories beyond it.
    #[serde(default)]
    pub search_index: Option<PathBuf>,
    /// Rust ABI caches supplied by the toolchain for this unit.
    pub rust_abi: Vec<PathBuf>,
    /// The directory of Oven's bakery-owned Rust ABI extractions for this description, beside it as
    /// `<name>.rust-facts` for `<name>.unit.json`. Its `index` names the extractions the unit used last time, one
    /// `identity<TAB>path` line each, the path a read-only bakery payload; a run writes each extraction the index
    /// lacks to `staging/<identity>.rustabi` and appends every identity it read or wrote to `staging/used`, and
    /// Oven bakes these before the description's next run. Read from the description's location, never from its
    /// contents, so a compiler that predates it reads the same description; without the directory no check-time
    /// extraction is cached.
    #[serde(skip)]
    pub rust_facts: Option<PathBuf>,
    /// The rustc-internal access the unit's Loaf declares; a description names it only when there is some, and the
    /// compiler grants nothing else.
    #[serde(default, skip_serializing_if = "ToolchainAccess::is_empty")]
    pub toolchain_access: ToolchainAccess,
    /// The `search` directories the description itself lists, without the index's. A library unit's native closure
    /// publishes these; a consumer of the same toolchain generation adds the index through its own description.
    #[serde(skip)]
    pub own_search: Vec<PathBuf>,
}

/// The rustc-internal access a Loaf declares (`[build]`), which Oven admitted against the pinned toolchain and bound
/// into the description: its sysroot dependencies are among the unit's `externs`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainAccess {
    /// Unstable features of the pinned compiler the unit's crate enables, such as `rustc_private`.
    #[serde(default)]
    pub unstable_features: Vec<String>,
    /// Components of the pinned toolchain the unit requires, such as `rustc-dev`.
    #[serde(default)]
    pub toolchain_components: Vec<String>,
    /// Libraries of the pinned toolchain the unit links, such as `rustc_driver`.
    #[serde(default)]
    pub sysroot_dependencies: Vec<String>,
}

impl ToolchainAccess {
    /// Whether the unit declares no access at all.
    pub fn is_empty(&self) -> bool {
        self.unstable_features.is_empty()
            && self.toolchain_components.is_empty()
            && self.sysroot_dependencies.is_empty()
    }

    /// Whether an executable of the unit loads toolchain libraries at run time.
    pub fn links_sysroot(&self) -> bool {
        !self.sysroot_dependencies.is_empty()
    }
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

/// One Clove a unit reaches: its library, the headers it exposes and its compiled configurations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitClove {
    /// The Clove's Loaf name, which `c.loaf_library` names.
    pub name: String,
    /// The Clove's root directory, which its public headers and include roots are relative to.
    pub root: PathBuf,
    /// The archive's link name: `lib<library>.a`.
    pub library: String,
    /// The headers a binding may name, Clove-relative.
    pub public_headers: Vec<String>,
    /// The Clove's include roots, absolute.
    pub include: Vec<PathBuf>,
    /// Whether the archive holds C++ objects, whose runtime library the final link adds.
    pub cplusplus: bool,
    /// The compiler identity that built the archives; `host:` marks the non-portable local override.
    pub compiler: String,
    /// Every compiled configuration.
    pub configurations: Vec<UnitCloveConfiguration>,
}

/// One compiled configuration of a Clove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitCloveConfiguration {
    /// The configuration name.
    pub name: String,
    /// Its resolved preprocessor definitions, `NAME=VALUE`.
    pub definitions: Vec<String>,
    /// The static archive Oven compiled for it.
    pub archive: PathBuf,
    /// The archive's content identity.
    pub identity: String,
}

/// The declared `[[interop.c.targets]]` entry a unit's checked C bindings verify against instead of the host
/// (`--interop-target`): its ABI facts, definitions and capability requirements. Verification only; nothing is
/// cross-compiled for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitCAbiTarget {
    /// The declared target triple.
    pub target: String,
    /// An Android target's API level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_level: Option<u32>,
    /// An iOS target's minimum deployment target, `major.minor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment_target: Option<String>,
    /// The target's preprocessor definitions, `NAME` or `NAME=VALUE`.
    #[serde(default)]
    pub definitions: Vec<String>,
    /// The compatible toolchain capability the selected compiler must provide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<UnitCapabilityRequirement>,
    /// The compatible SDK capability the selected SDK must provide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<UnitCapabilityRequirement>,
}

/// A declared toolchain or SDK capability and its semantic-version requirement, empty for any version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitCapabilityRequirement {
    /// The capability's name.
    pub capability: String,
    /// The compatible versions, empty for any.
    #[serde(default)]
    pub version: String,
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
    /// Whether the toolchain supplies this facade rather than the project declaring it as a dependency.
    #[serde(default)]
    pub toolchain_provided: bool,
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
        let mut unit: Self = if bytes.starts_with(b"CRMB") {
            let fields = internal_models::decode_unit_document(&bytes.to_vec())
                .map_err(|error| CliError::failure(format!("{}: {error}", origin.display())))?;
            serde_json::to_value(fields).and_then(serde_json::from_value)
        } else {
            serde_json::from_slice(bytes)
        }
            .map_err(|error| CliError::failure(format!("{} is not a unit description: {error}", origin.display())))?;
        if unit.schema != UNIT_DESCRIPTION_SCHEMA {
            return Err(CliError::failure(format!(
                "{} has schema `{}`; this compiler reads `{UNIT_DESCRIPTION_SCHEMA}`",
                origin.display(),
                unit.schema
            )));
        }
        if let Some(registry) = &unit.registry_id {
            incan_semantics_core::record_identity::validate_record_registry_id(registry)
                .map_err(|error| CliError::failure(format!("{}: {error}", origin.display())))?;
        }
        if let Some(identity) = unit.record_package_identity() {
            identity
                .validate()
                .map_err(|error| CliError::failure(format!("{}: {error}", origin.display())))?;
        }
        unit.merge_search_index()?;
        unit.rust_facts = rust_facts_directory(origin);
        crate::loaf_publication::vocab::prepared_registration(&unit)?;
        Ok(unit)
    }

    /// Retain a persistent record namespace only when the unit declares both a registry and a scoped name.
    /// Scripts and packages without the released default registry key keep ordinary reflection without a selector.
    pub fn record_package_identity(&self) -> Option<incan_semantics_core::record_identity::RecordPackageIdentity> {
        if !self.package.contains('/') {
            return None;
        }
        Some(incan_semantics_core::record_identity::RecordPackageIdentity {
            registry_id: self.registry_id.as_ref()?.clone(),
            scoped_name: self.package.clone(),
        })
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

/// The Rust ABI extraction directory Oven keeps beside a `<name>.unit.crumb` or legacy `<name>.unit.json` description, when it exists.
fn rust_facts_directory(origin: &Path) -> Option<PathBuf> {
    let file = origin.file_name()?.to_str()?;
    let name = file.strip_suffix(".unit.crumb").or_else(|| file.strip_suffix(".unit.json"))?;
    let directory = origin.with_file_name(format!("{name}.rust-facts"));
    directory.is_dir().then_some(directory)
}
