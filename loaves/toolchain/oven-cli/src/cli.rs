//! The clap types of the three command families this package owns: `oven` and its nested store, plan, interop
//! and compatibility commands, `lock`, and `tools`. The `oven` binary parses them at its root; `incan` mounts
//! the same types under `incan oven`, `incan lock` and `incan tools`, so one definition serves both spellings
//! and the flag groups the compilation commands share with them live here too.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use incan_provider::FeatureSelection;

use crate::commands;
use crate::commands::tools::{ToolsDoctorFormat, ToolsMetadataFormat, ToolsModelMetadataFormat};

/// Output encoding for explicit Oven Alpha receipts, plans, storage, test reports, and run reports.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenOutputFormat {
    /// Human-readable command result.
    Text,
    /// Stable machine-readable JSON report.
    Json,
}

/// Fixed caller-owned native layout selected by `incan oven interop stage`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OvenInteropAdapterArgument {
    /// Android arm64 JNI runtime layout.
    Android,
    /// iOS device or simulator framework runtime layout.
    Ios,
}

/// Built-in compiler-owned Loaf envelope selected by the hidden baker.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenLoafEnvelopeArgument {
    /// Minimal Loafs shipped in a release toolchain.
    Release,
    /// Complete Loaf set used by the repository compiler suite.
    CompilerSuite,
}

/// Incan package-feature selection shared by compilation commands.
///
/// These flags select package-owned semantic features. They are intentionally separate from the explicitly prefixed
/// Cargo feature flags, which remain private backend controls.
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageFeatureCliFlags {
    /// Incan package features to enable (comma-separated)
    #[arg(long = "features", value_delimiter = ',', value_name = "FEATURE")]
    pub features: Vec<String>,
    /// Disable the Incan package's default features
    #[arg(long = "no-default-features")]
    pub no_default_features: bool,
    /// Enable every Incan package feature
    #[arg(long = "all-features")]
    pub all_features: bool,
}

impl From<PackageFeatureCliFlags> for FeatureSelection {
    fn from(flags: PackageFeatureCliFlags) -> Self {
        Self {
            requested: flags.features.into_iter().collect(),
            no_default_features: flags.no_default_features,
            all_features: flags.all_features,
        }
    }
}

/// Command-local SDK profile selection shared by compilation and inspection commands.
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct SdkProfileCliFlags {
    /// Replace the project's SDK profile for this invocation without changing explicit additions or exclusions
    #[arg(long = "sdk-profile", value_name = "PROFILE")]
    pub sdk_profile: Option<String>,
}

impl SdkProfileCliFlags {
    /// Return the command-local SDK profile override without changing persistent project selection.
    pub fn profile(&self) -> Option<&str> {
        self.sdk_profile.as_deref()
    }
}

#[derive(Subcommand, Debug)]
pub enum ToolsCommand {
    /// Inspect local `incan` / `incan-lsp` path resolution
    Doctor {
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: ToolsDoctorFormat,
    },
    /// Extract checked metadata for tooling and documentation consumers
    Metadata {
        #[command(subcommand)]
        command: ToolsMetadataCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum ToolsMetadataCommand {
    /// Emit checked public API metadata as JSON
    Api {
        /// Incan source file or project directory to inspect
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "json")]
        format: ToolsMetadataFormat,
    },
    /// Emit a contract-backed model from checked model metadata
    Model {
        /// Project directory, bundle JSON, or `.incnlib` artifact to inspect
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Logical type name or stable model id to emit
        #[arg(value_name = "MODEL")]
        model: String,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "incan")]
        format: ToolsModelMetadataFormat,
    },
}

/// Explicit Oven Alpha lifecycle commands.
#[derive(Subcommand, Debug)]
pub enum OvenCommand {
    /// Explicitly materialize or reuse sealed toolchain Loafs for an Incan project
    Bake {
        /// Project root containing loaf.toml and src/lib.incn and/or src/main.incn
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// Explicit Rust compilation target; defaults to the active compiler's host target
        #[arg(long, value_name = "TRIPLE")]
        target: Option<String>,
        /// Select Incan package features for the baked project Loaf
        #[command(flatten)]
        package_features: PackageFeatureCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Emit the compiler-owned SDK provider source identity for repository automation.
    #[command(hide = true)]
    SdkProviderStoreIdentity {
        /// Compiler source checkout that owns the built-in standard library
        #[arg(long = "compiler-root", value_name = "PATH", default_value = ".")]
        compiler_root: PathBuf,
    },
    /// Import frozen Cargo declarations as receipt evidence without launching Cargo
    Import {
        /// Root of the frozen Cargo package to import
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// Explicit target triple recorded in the receipt
        #[arg(long, value_name = "TRIPLE")]
        target: String,
        /// Exact selected Rust toolchain identity recorded in the receipt
        #[arg(long, value_name = "IDENTITY")]
        toolchain: String,
        /// Build profile recorded in the receipt
        #[arg(long, default_value = "release")]
        profile: String,
        /// Explicit feature selected for the build unit; may be repeated
        #[arg(long = "feature", value_name = "NAME")]
        features: Vec<String>,
        /// Generated source evidence expressed as `NAME=PATH`; paths are digested, never persisted
        #[arg(long = "source", value_name = "NAME=PATH")]
        source_inputs: Vec<String>,
        /// Receipt output path; defaults to `.incan/oven/receipt.json` below --project
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Bake locked C/C++ interop shims and static inputs into one receipt-bound direct-rustc plan
    Interop {
        #[command(subcommand)]
        command: OvenInteropCommand,
    },
    /// Internal compatibility publisher; never used by normal build, run, or test execution
    #[command(hide = true)]
    LegacyCargo {
        #[command(subcommand)]
        command: OvenLegacyCargoCommand,
    },
    /// Compile and run the stored compiler workspace native suite through a direct-rustc plan
    CompilerLibtests {
        /// Repository root containing the compiler Cargo package and src/lib.rs
        #[arg(long = "compiler-root", value_name = "PATH", default_value = ".")]
        compiler_root: PathBuf,
        /// Explicit rustc executable; the active toolchain is used when omitted
        #[arg(long, value_name = "PATH")]
        rustc: Option<PathBuf>,
        /// Root-package feature to include; default Cargo features are always included
        #[arg(long = "feature", value_name = "NAME")]
        features: Vec<String>,
        /// Receipt-bound test source path to execute; may be repeated. Omitting this runs the complete stored suite.
        #[arg(long = "target", value_name = "SOURCE")]
        targets: Vec<String>,
        /// Exact test in one receipt-bound target; may be repeated to reuse one compiled libtest binary
        #[arg(long = "exact", value_name = "TEST")]
        exact_names: Vec<String>,
        /// Zero-based receipt-index partition used by the bounded CI replay; requires `--partition-count`
        #[arg(
            long = "partition-index",
            value_name = "INDEX",
            requires = "partition_count",
            hide = true
        )]
        partition_index: Option<usize>,
        /// Total receipt-index partitions used by the bounded CI replay; requires `--partition-index`
        #[arg(
            long = "partition-count",
            value_name = "COUNT",
            requires = "partition_index",
            hide = true
        )]
        partition_count: Option<usize>,
        /// Explicit Cargo for compiler-suite roots that deliberately exercise the Loaf baker
        #[arg(long = "fixture-cargo", value_name = "PATH", hide = true)]
        fixture_cargo: Option<PathBuf>,
        /// Caller-owned direct-rustc libtest output path
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Validate and store a receipt-bound direct-rustc artifact plan
    Plan {
        #[command(subcommand)]
        command: OvenPlanCommand,
    },
    /// Inspect or prune bounded Oven artifact storage
    Store {
        #[command(subcommand)]
        command: OvenStoreCommand,
    },
    /// Compile with a stored direct-rustc plan and run exact native tests only after inventory verification
    Test {
        /// Receipt authorizing the generated source and stored plan
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        /// Immutable stored direct-rustc plan identity
        #[arg(long = "plan", value_name = "SHA256")]
        plan_identity: String,
        /// Explicit rustc executable path
        #[arg(long, value_name = "PATH")]
        rustc: PathBuf,
        /// Receipt-authorized generated Rust test source
        #[arg(long, value_name = "PATH")]
        source: PathBuf,
        /// Caller-owned native libtest output path
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        /// Rust test crate name
        #[arg(long = "crate-name", value_name = "NAME")]
        crate_name: String,
        /// Rust edition for the native test binary
        #[arg(long, default_value = "2024")]
        edition: String,
        /// Named receipt source evidence authorizing --source
        #[arg(long = "source-evidence", value_name = "NAME")]
        source_evidence_key: String,
        /// Exact test name; may be repeated and is verified against native inventory before execution
        #[arg(long = "exact", value_name = "TEST")]
        exact_names: Vec<String>,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Compile and run one stored direct-rustc binary without a Cargo consumer
    Run {
        /// Receipt authorizing the generated source and stored plan
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        /// Immutable stored direct-rustc plan identity
        #[arg(long = "plan", value_name = "SHA256")]
        plan_identity: String,
        /// Explicit rustc executable path
        #[arg(long, value_name = "PATH")]
        rustc: PathBuf,
        /// Receipt-authorized generated Rust binary source
        #[arg(long, value_name = "PATH")]
        source: PathBuf,
        /// Caller-owned native binary output path
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        /// Rust binary crate name
        #[arg(long = "crate-name", value_name = "NAME")]
        crate_name: String,
        /// Rust edition for the native binary
        #[arg(long, default_value = "2024")]
        edition: String,
        /// Named receipt source evidence authorizing --source
        #[arg(long = "source-evidence", value_name = "NAME")]
        source_evidence_key: String,
        /// Arguments forwarded after compilation only to the native binary
        #[arg(last = true, allow_hyphen_values = true, value_name = "ARG")]
        arguments: Vec<OsString>,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
}

/// Explicit Oven-owned native interop baking commands.
#[derive(Subcommand, Debug)]
pub enum OvenInteropCommand {
    /// Select declared native tools, compile locked shims, and publish one complete direct-rustc interop plan
    Bake {
        /// Package root containing loaf.toml and the locked interop inputs
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// Exact locked target triple to bake
        #[arg(long, value_name = "TRIPLE")]
        target: String,
        /// Existing pre-interop Oven receipt used to select the sealed base Loaf plan; omit to prepare the exact debug
        /// base
        #[arg(long = "base-receipt", value_name = "PATH")]
        base_receipt: Option<PathBuf>,
        /// Explicit selected C compiler for a declared C shim or toolchain requirement
        #[arg(long = "c-compiler", value_name = "PATH")]
        c_compiler: Option<PathBuf>,
        /// Explicit selected C++ compiler for a declared C++ shim
        #[arg(long = "cxx-compiler", value_name = "PATH")]
        cxx_compiler: Option<PathBuf>,
        /// Explicit selected static archiver for declared C/C++ shims
        #[arg(long, value_name = "PATH")]
        archiver: Option<PathBuf>,
        /// Semantic version of the explicitly selected compiler required by the locked toolchain capability
        #[arg(long = "toolchain-version", value_name = "VERSION")]
        toolchain_version: Option<String>,
        /// Explicit selected SDK root required by the locked SDK capability
        #[arg(long = "sdk-root", value_name = "PATH")]
        sdk_root: Option<PathBuf>,
        /// Semantic version of the explicitly selected SDK required by the locked SDK capability
        #[arg(long = "sdk-version", value_name = "VERSION")]
        sdk_version: Option<String>,
        /// Regular identity file below --sdk-root whose content records the selected SDK provider identity
        #[arg(long = "sdk-identity-file", value_name = "PATH")]
        sdk_identity_file: Option<PathBuf>,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Atomically stage a baked interop plan's bundled runtime files without starting a platform build tool
    Stage {
        /// Package root containing loaf.toml, oven.lock, and the selected interop receipt
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// Exact locked target triple whose already baked plan will be staged
        #[arg(long, value_name = "TRIPLE")]
        target: String,
        /// Existing pre-interop Oven receipt used to reconstruct the final immutable interop plan receipt
        #[arg(long = "base-receipt", value_name = "PATH")]
        base_receipt: PathBuf,
        /// Fixed Android or iOS output layout; this does not invoke Gradle, Xcode, or signing
        #[arg(long, value_enum)]
        adapter: OvenInteropAdapterArgument,
        /// New caller-owned output directory; existing output is never replaced
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
}

/// Internal compatibility-publisher commands for baking immutable Oven inputs.
#[derive(Subcommand, Debug)]
pub enum OvenLegacyCargoCommand {
    /// Prepare one receipt-bound direct-rustc closure and retain only the bounded Oven result
    Prepare {
        /// Generated-project receipt authorizing this preparation
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        /// Caller-owned generated Rust project with Cargo.toml and src/main.rs
        #[arg(long = "generated-project", value_name = "PATH")]
        generated_project: PathBuf,
        /// Explicit Cargo executable used only for this publisher transition
        #[arg(long, value_name = "PATH")]
        cargo: PathBuf,
        /// Explicit Rust compiler required to match the receipt
        #[arg(long, value_name = "PATH")]
        rustc: PathBuf,
        /// Stable compatibility domain for bounded Oven storage
        #[arg(long, value_name = "NAME")]
        domain: String,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Bake or reuse one complete compiler-owned Alpha Loaf envelope
    #[command(hide = true)]
    BakeLoafs {
        /// Compiler checkout or staged toolchain root used for runtime-source identity
        #[arg(long = "compiler-root", value_name = "PATH", default_value = ".")]
        compiler_root: PathBuf,
        /// Destination directory for immutable `<identity>.loaf` bundles
        #[arg(long, value_name = "PATH")]
        output: PathBuf,
        /// Bounded compiler-suite store baked with the compiler-suite envelope
        #[arg(long = "suite-store", value_name = "PATH")]
        suite_store: Option<PathBuf>,
        /// Existing Oven store containing the exact release policy ProjectOutput
        #[arg(
            long = "policy-engine-store",
            value_name = "PATH",
            requires_all = ["policy_engine_identity", "policy_engine_target"]
        )]
        policy_engine_store: Option<PathBuf>,
        /// Exact ProjectOutput identity to embed in the release envelope
        #[arg(
            long = "policy-engine-identity",
            value_name = "IDENTITY",
            requires_all = ["policy_engine_store", "policy_engine_target"]
        )]
        policy_engine_identity: Option<String>,
        /// Exact Rust target required of the embedded policy engine
        #[arg(
            long = "policy-engine-target",
            value_name = "TRIPLE",
            requires_all = ["policy_engine_store", "policy_engine_identity"]
        )]
        policy_engine_target: Option<String>,
        /// Built-in release or compiler-suite Loaf envelope
        #[arg(long, value_enum)]
        envelope: OvenLoafEnvelopeArgument,
        /// Exact compiler-owned SDK provider inventory
        #[arg(long = "sdk-inventory", value_name = "PATH")]
        sdk_inventory: PathBuf,
        /// Explicit Cargo executable used only for a genuine Loaf miss
        #[arg(long, value_name = "PATH")]
        cargo: PathBuf,
        /// Explicit Rust compiler recorded by each Loaf receipt
        #[arg(long, value_name = "PATH")]
        rustc: PathBuf,
        /// Aggregate physical Loaf-envelope allowance
        #[arg(long = "max-physical-bytes", value_name = "BYTES")]
        max_physical_bytes: Option<u64>,
        /// Physical allowance for one Loaf compatibility domain
        #[arg(long = "max-domain-physical-bytes", value_name = "BYTES")]
        max_domain_physical_bytes: Option<u64>,
        /// Logical allowance for one Loaf compatibility domain
        #[arg(long = "max-domain-logical-bytes", value_name = "BYTES")]
        max_domain_logical_bytes: Option<u64>,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
        /// Registered Loaf registry checkout whose adoption manifests govern captured registry units
        #[arg(long = "loaf-registry", value_name = "PATH")]
        loaf_registry: Option<PathBuf>,
        /// Write one harvest proposal per captured registry unit with a build-script edge into this directory
        #[arg(long = "harvest-dir", value_name = "PATH")]
        harvest_dir: Option<PathBuf>,
    },
}

/// Direct-rustc plan publication commands.
#[derive(Subcommand, Debug)]
pub enum OvenPlanCommand {
    /// Validate and retain a direct-rustc artifact manifest under bounded Oven policy
    Publish {
        /// Receipt authorizing the plan
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        /// JSON direct-rustc artifact manifest
        #[arg(long, value_name = "PATH")]
        manifest: PathBuf,
        /// Immutable artifact root used to validate the manifest content
        #[arg(long = "artifact-root", value_name = "PATH")]
        artifact_root: PathBuf,
        /// Stable compatibility domain for capacity policy and selection
        #[arg(long, value_name = "NAME")]
        domain: String,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
}

/// Bounded Oven store management commands.
#[derive(Subcommand, Debug)]
pub enum OvenStoreCommand {
    /// Report physical allocation separately from logical artifact bytes
    Inspect {
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
    /// Prune inactive immutable artifacts to the configured physical allocation policy
    Prune {
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Preview policy-selected removals without changing the Oven store
        #[arg(long)]
        dry_run: bool,
        /// Output format
        #[arg(long = "format", value_enum, default_value = "text")]
        format: OvenOutputFormat,
    },
}

/// Shared CLI policy flags for a bounded Oven artifact store.
#[derive(Args, Debug, Clone)]
pub struct OvenStoreCliFlags {
    /// Explicit Oven store root; defaults below INCAN_HOME or the user home directory
    #[arg(long = "store", value_name = "PATH")]
    pub root: Option<PathBuf>,
    /// Maximum aggregate physical allocation in bytes
    #[arg(long = "max-physical-bytes", value_name = "BYTES")]
    pub max_physical_bytes: Option<u64>,
    /// Maximum physical allocation for one compatibility domain in bytes
    #[arg(long = "max-domain-physical-bytes", value_name = "BYTES")]
    pub max_domain_physical_bytes: Option<u64>,
    /// Maximum logical artifact bytes for one compatibility domain in bytes
    #[arg(long = "max-domain-logical-bytes", value_name = "BYTES")]
    pub max_domain_logical_bytes: Option<u64>,
}

impl From<OvenStoreCliFlags> for commands::OvenStoreCommandOptions {
    /// Convert command-line bounded store selections into command-owned request data.
    fn from(value: OvenStoreCliFlags) -> Self {
        Self {
            root: value.root,
            max_physical_bytes: value.max_physical_bytes,
            max_domain_physical_bytes: value.max_domain_physical_bytes,
            max_domain_logical_bytes: value.max_domain_logical_bytes,
        }
    }
}

/// Arguments of `lock`: the entry file and the feature selections that shape the locked graph.
#[derive(Args, Debug)]
pub struct LockArgs {
    /// Entry file used to resolve inline dependencies
    #[arg(value_name = "FILE")]
    pub file: Option<PathBuf>,
    /// Select Incan package features for the locked graph
    #[command(flatten)]
    pub package_features: PackageFeatureCliFlags,
    /// Select a non-persistent SDK profile for the locked graph
    #[command(flatten)]
    pub sdk_profile: SdkProfileCliFlags,
    /// Cargo features to enable (comma-separated)
    #[arg(long = "cargo-features", value_delimiter = ',')]
    pub cargo_features: Vec<String>,
    /// Disable Cargo default features
    #[arg(long = "cargo-no-default-features")]
    pub cargo_no_default_features: bool,
    /// Enable all Cargo features
    #[arg(long = "cargo-all-features")]
    pub cargo_all_features: bool,
}
