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

/// Release-evidence gates owned by Oven.
#[derive(Subcommand, Debug)]
pub enum OvenGateCommand {
    /// Verify a checkout and its committed lock against a schema-1 registry pin
    RegistryPin {
        /// Schema-1 checkout and lock pin
        #[arg(long, value_name = "PATH")]
        pin: PathBuf,
        /// Git checkout to verify
        #[arg(long, value_name = "PATH")]
        checkout: PathBuf,
    },
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
    /// Run project tests through the shared owner
    Tests {
        /// Collect only Incan source tests
        #[arg(long, conflicts_with = "rust_only")]
        incan_only: bool,
        /// Collect only Rust source tests
        #[arg(long, conflicts_with = "incan_only")]
        rust_only: bool,
        #[command(flatten)]
        arguments: TestArgs,
    },
    /// Compile a pinned adopted-Loaf closure without Cargo or build-script execution
    CompileClosure(CompileClosureArgs),
    /// Retain compiler-suite reports and transcripts, then reclaim disposable output
    RetainSuiteOutput {
        /// Print the host clock in nanoseconds
        #[arg(long)]
        clock: bool,
        /// Positional retention arguments: output, scratch, success, report, status, and optional wrapper clocks
        #[arg(value_name = "ARG")]
        arguments: Vec<String>,
    },
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
    /// Run release-evidence gates over Oven-owned semantic state
    Gate {
        #[command(subcommand)]
        command: OvenGateCommand,
    },
    /// Bake locked C/C++ interop shims and static inputs into one receipt-bound direct-rustc plan
    Interop {
        #[command(subcommand)]
        command: OvenInteropCommand,
    },
    /// Run one authored compiler test root against ordinary declaration-selected native Loafs
    #[command(hide = true)]
    CompilerNativeTests {
        /// Source checkout owning the test root and its authored Loaf dependencies
        #[arg(long, default_value = ".")]
        compiler_root: PathBuf,
        /// Checkout-relative Rust libtest source
        #[arg(long)]
        target: PathBuf,
        /// Authored dependency declaration; defaults to the source's sibling `.loaf.toml`
        #[arg(long)]
        declaration: Option<PathBuf>,
        /// Explicit resolved registry selection and local facet policy, without a standard-library publication
        #[arg(long)]
        native_graph: Option<PathBuf>,
        /// Registry index containing the exact revision declared by the native graph
        #[arg(long)]
        native_index: Option<PathBuf>,
        /// Digest-addressed source archives needed by the native graph
        #[arg(long)]
        native_blobs: Option<PathBuf>,
        /// Additional checkout-owned compiled or embedded source input, repeated for separate files or trees
        #[arg(long = "input")]
        source_inputs: Vec<PathBuf>,
        /// Exact inventory-verified case, including explicitly selected ignored cases
        #[arg(long = "exact")]
        exact_names: Vec<String>,
        /// Caller-owned output; kept outputs reuse the receipt-bound executable
        #[arg(long)]
        output: PathBuf,
        #[command(flatten)]
        store: OvenStoreCliFlags,
        /// Persistent source workspaces for explicit native fixture bakes
        #[arg(long)]
        explicit_bake_workspace: PathBuf,
        /// Compiler executable matching the selected native Loafs
        #[arg(long)]
        rustc: PathBuf,
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

/// Physical inputs and policy for the shared adopted-closure executor.
#[derive(Args, Debug)]
pub struct CompileClosureArgs {
    /// Index checkout whose committed files supply facts
    #[arg(long)]
    pub pin: PathBuf,
    /// Full index commit identity, used for every read
    #[arg(long)]
    pub index_commit: String,
    /// JSON root requests with target and host triples
    #[arg(long)]
    pub roots: PathBuf,
    /// Compilation profile
    #[arg(long, value_parser = ["debug", "release"])]
    pub profile: String,
    /// Closure result JSON destination
    #[arg(long)]
    pub out: PathBuf,
    /// Optional existing resolution; omission runs the Incan resolver at the supplied index commit
    #[arg(long)]
    pub lock: Option<PathBuf>,
    /// Digest-addressed source archive directory
    #[arg(long)]
    pub blobs: Option<PathBuf>,
    /// Selected compiler; defaults to the active pinned compiler
    #[arg(long)]
    pub rustc: Option<PathBuf>,
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
}

/// Shared project test options used by both entry points.
#[derive(clap::Args, Debug)]
pub struct TestArgs {
    /// Path to test file or directory
    #[arg(value_name = "PATH", default_value = ".")]
    pub path: PathBuf,
    /// Select Incan package features for collection and generated test batches
    #[command(flatten)]
    pub package_features: PackageFeatureCliFlags,
    /// Select a non-persistent SDK profile for collection and generated test batches
    #[command(flatten)]
    pub sdk_profile: SdkProfileCliFlags,
    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,
    /// Stop on first failure
    #[arg(short = 'x', long = "exitfirst")]
    pub stop_on_fail: bool,
    /// Include slow tests
    #[arg(long)]
    pub slow: bool,
    /// Filter tests by keyword expression
    #[arg(short = 'k', value_name = "EXPR")]
    pub filter: Option<String>,
    /// Filter tests by marker expression
    #[arg(short = 'm', long = "markers", value_name = "EXPR")]
    pub marker_expr: Option<String>,
    /// Treat unknown marker names as collection errors
    #[arg(long = "strict-markers")]
    pub strict_markers: bool,
    /// Total test worker budget, shared by native test threads and isolated execution units
    #[arg(short = 'j', long = "jobs", value_name = "N", default_value_t = 4)]
    pub jobs: usize,
    /// Run Rust test cases in separate processes; Incan fixture groups preserve their existing scope
    #[arg(long)]
    pub isolate: bool,
    /// Enable a collection-time testing feature for std.testing.feature("name")
    #[arg(long = "feature", value_name = "NAME")]
    pub test_features: Vec<String>,
    /// Default generated test-batch timeout, such as 250ms, 5s, or 2m
    #[arg(long = "timeout", value_name = "DURATION")]
    pub timeout: Option<String>,
    /// Show test stdout/stderr even when tests pass
    #[arg(long = "nocapture")]
    pub no_capture: bool,
    /// Fail if no tests are collected
    #[arg(long = "fail-on-empty")]
    pub fail_on_empty: bool,
    /// List collected tests after filtering and do not execute them
    #[arg(long = "list")]
    pub list_only: bool,
    /// Output format
    #[arg(long = "format", value_enum, default_value = "console")]
    pub report_format: crate::test_runner::TestOutputFormat,
    /// Write a JUnit XML report to this path
    #[arg(long = "junit", value_name = "PATH")]
    pub junit_path: Option<PathBuf>,
    /// Show the slowest N test durations after the run
    #[arg(long = "durations", value_name = "N")]
    pub durations: Option<usize>,
    /// Shuffle test execution order
    #[arg(long)]
    pub shuffle: bool,
    /// Seed used with --shuffle
    #[arg(long, value_name = "N")]
    pub seed: Option<u64>,
    /// Run xfail tests as ordinary tests
    #[arg(long = "run-xfail")]
    pub run_xfail: bool,
    /// Require up-to-date oven.lock; does not authorize a Cargo command
    #[arg(long, hide = true)]
    pub locked: bool,
    /// Disable INCAN_LOCKED for this invocation
    #[arg(long = "no-locked", conflicts_with_all = ["locked", "frozen"], hide = true)]
    pub no_locked: bool,
    /// Require offline-compatible locked inputs; does not authorize a Cargo command
    #[arg(long, hide = true)]
    pub offline: bool,
    /// Disable INCAN_OFFLINE for this invocation
    #[arg(long = "no-offline", conflicts_with_all = ["offline", "frozen"], hide = true)]
    pub no_offline: bool,
    /// Require an up-to-date frozen oven.lock; does not authorize a Cargo command
    #[arg(long, hide = true)]
    pub frozen: bool,
    /// Disable INCAN_FROZEN for this invocation
    #[arg(long = "no-frozen", conflicts_with = "frozen", hide = true)]
    pub no_frozen: bool,
    /// Select every member in the active workspace
    #[arg(long, conflicts_with = "members")]
    pub workspace: bool,
    /// Select one workspace member by name or root-relative path; may be repeated
    #[arg(long = "member", value_name = "NAME_OR_PATH", conflicts_with = "workspace")]
    pub members: Vec<String>,
}
