//! Compiler-backed codegraph inspection.
//!
//! The RFC 106 graph under the broader RFC 102 semantic inspection umbrella, behind `incan inspect codegraph` and the
//! language server: stable files, modules, declarations, imports, exports, containment, checked references and
//! diagnostics, without a storage or indexing engine inside the compiler.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use incan_codegraph::{
    CODEGRAPH_SCHEMA_VERSION, CodegraphCBindingBuffer, CodegraphCBindingCallRecord, CodegraphCBindingEnum,
    CodegraphCBindingEnumVariant, CodegraphCBindingFacadeRecord, CodegraphCBindingOutcome, CodegraphCBindingParameter,
    CodegraphCBindingRecord, CodegraphCBindingResource, CodegraphCBindingStruct, CodegraphCBindingStructField,
    CodegraphCBindingSymbol, CodegraphCBindingType, CodegraphCallRecord, CodegraphCanonicalSymbolId,
    CodegraphCapabilityRecord, CodegraphCapabilityScopeDimension, CodegraphContainmentRecord,
    CodegraphDeclarationRecord, CodegraphDiagnosticRecord, CodegraphDiagnosticRelatedDeclaration,
    CodegraphDiagnosticRelatedSpan, CodegraphExportRecord, CodegraphFileRecord, CodegraphHeaderRecord,
    CodegraphIdentitySpan, CodegraphImportBinding, CodegraphImportRecord, CodegraphLanguage, CodegraphLoafProjection,
    CodegraphMode, CodegraphModuleRecord, CodegraphNamespaceRecord, CodegraphPackage, CodegraphProvenance,
    CodegraphRecord, CodegraphReferenceRecord, CodegraphRegistryRecord, CodegraphRegistryReexportProjection,
    CodegraphSemanticContext, CodegraphSourceSpan, CodegraphStableDeclarationId, CodegraphStableDeclarationLocation,
    CodegraphSymbolOrigin,
};
use incan_lang::lang::c_abi::{link_capability_as_str, scalar_type_as_str};
use incan_semantics_core::namespace::{enclosing_namespace, is_namespace_root};
use incan_semantics_core::stable_identity::{StableDeclarationContext, StableDeclarationId, StableDeclarationLocation};
use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, SemanticModuleSnapshot, SymbolOrigin};
use serde_json::{Value, json};

use incan_frontend::ast::{
    AssertKind, CallArg, ComprehensionClause, Condition, Declaration, Decorator, DecoratorArg, DecoratorArgValue,
    DictEntry, Expr, FStringPart, FunctionDecl, ImportDecl, ImportItem, ImportKind, ImportPath, ListEntry, MatchArm,
    MatchBody, RaceForBody, Span, Spanned, Statement, SurfaceExprPayload, SurfaceStmtPayload, TypeParam, Visibility,
};
use incan_frontend::diagnostics::{self, StableDiagnostic};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::module::logical_module_segments_from_file;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::registry_metadata::{
    CheckedRegistryMetadataModule, CheckedRegistrySubjectKind, CheckedRegistryValue, collect_checked_registry_metadata,
};
use incan_frontend::typechecker::{
    CAbiInteropArtifacts, CBindingEnum, CBindingEnumVariant, CBindingOutcome, CBindingParameter, CBindingResource,
    CBindingStruct, CBindingStructField, CBindingSymbol, CBindingType, COutputMode, CResourceAccess,
    CapabilityDeclarationInfo, c_binding_descriptor_identity,
};
use incan_lang::version::INCAN_VERSION;

use crate::diagnostics::CliDiagnosticFailure;
use crate::loaf_publication::PublicationFacts;
use crate::modules::{
    collect_library_modules_detailed_with_session, collect_modules_detailed_with_session,
    collect_modules_detailed_with_session_at_path,
};
use crate::session::{CompilationAnalysis, CompilationSession};
use crate::sources::read_source;
use crate::unit::{UnitDescription, UnitKind};

mod body_records;
mod builder_setup;
mod c_binding_records;
mod checked_records;
mod collection;
mod record_builders;
mod references;
mod stable_identities;

use c_binding_records::*;
pub use collection::*;
use record_builders::*;
pub use stable_identities::*;

/// A failure while producing codegraph records.
///
/// Owned here rather than borrowed from the driver: a command consumes analysis rather than the reverse, so
/// analysis reports its own failure and the driver's error converts at its boundary through the `From`
/// implementations below.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct CodegraphError {
    /// What went wrong, phrased for the person who ran the command.
    pub message: String,
}

impl CodegraphError {
    /// Build a failure carrying a human-readable explanation.
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// The result type every producer in this module returns.
pub type CodegraphResult<T> = Result<T, CodegraphError>;

impl From<crate::error::CliError> for CodegraphError {
    /// Absorb an error raised by the session and discovery code this module calls into, which reports `CliError`.
    fn from(error: crate::error::CliError) -> Self {
        Self::failure(error.message)
    }
}

impl From<CodegraphError> for crate::error::CliError {
    /// Convert at the command boundary, so a command can report an analysis failure as its own.
    fn from(error: CodegraphError) -> Self {
        Self::failure(error.message)
    }
}

struct CheckedCodegraphAnalysis {
    /// Checking-only interface; it authenticates no native library or executable body.
    interface: Option<LibraryManifest>,
    diagnostics: Vec<StableDiagnostic>,
    semantic_snapshots_by_path: BTreeMap<PathBuf, SemanticModuleSnapshot>,
    /// Header-owned source identities and optional executable digests, keyed by their canonical owner.
    declaration_facts: BTreeMap<CanonicalSymbolId, CheckedDeclarationFacts>,
    registry_metadata_by_path: BTreeMap<PathBuf, CheckedRegistryMetadataModule>,
    capabilities_by_path: BTreeMap<PathBuf, Vec<CapabilityDeclarationInfo>>,
    c_abi_by_path: BTreeMap<PathBuf, CAbiInteropArtifacts>,
}

type DirectoryTypecheckArtifacts = (Vec<ParsedModule>, CheckedCodegraphAnalysis);

/// The sessions of the described units, each opened on first use. A file belongs to the unit whose project root is
/// its nearest ancestor.
struct UnitSessions<'u> {
    units: &'u [UnitDescription],
    sessions: BTreeMap<PathBuf, CompilationSession>,
    rust: Option<&'u crate::session::RustFactsSetup>,
}

impl<'u> UnitSessions<'u> {
    /// Bind lazily opened sessions to the units Oven selected; an optional Rust source is installed before checking.
    fn new(units: &'u [UnitDescription]) -> Self {
        Self {
            units,
            sessions: BTreeMap::new(),
            rust: None,
        }
    }

    /// The unit whose project contains `file`.
    fn unit_for(&self, file: &Path) -> CodegraphResult<&'u UnitDescription> {
        self.units
            .iter()
            .filter(|unit| file.starts_with(&unit.project_root))
            .max_by_key(|unit| unit.project_root.components().count())
            .ok_or_else(|| CodegraphError::failure(format!("no described unit owns {}", file.display())))
    }

    /// The session of the unit that contains `file`.
    fn for_file(&mut self, file: &Path) -> CodegraphResult<&mut CompilationSession> {
        let unit = self.unit_for(file)?;
        if !self.sessions.contains_key(&unit.project_root) {
            let mut session = CompilationSession::new(unit)?;
            if let Some(rust) = self.rust {
                session = session.with_rust_facts(rust)?;
            }
            self.sessions.insert(unit.project_root.clone(), session);
        }
        self.sessions
            .get_mut(&unit.project_root)
            .ok_or_else(|| CodegraphError::failure(format!("no session for {}", unit.project_root.display())))
    }
}

struct CodegraphBuilder<'analysis> {
    records: Vec<CodegraphRecord>,
    diagnostics: Vec<StableDiagnostic>,
    file_ids: BTreeMap<String, String>,
    module_ids: BTreeSet<String>,
    /// Namespace ids already emitted, so one namespace yields one node however many modules sit beneath it.
    namespace_ids: BTreeSet<String>,
    mode: CodegraphMode,
    root_path: String,
    root_path_buf: PathBuf,
    package: Option<CodegraphPackage>,
    semantic_contexts: Vec<CodegraphSemanticContext>,
    next_body_fact_index: usize,
    semantic_snapshots_by_path: Cow<'analysis, BTreeMap<PathBuf, SemanticModuleSnapshot>>,
    /// Early source identities and optional executable digests, keyed by their canonical declaration owners.
    /// A failed Body IR projection leaves the source identity available and the executable digest unproven.
    declaration_facts: BTreeMap<CanonicalSymbolId, CheckedDeclarationFacts>,
    /// Checked owner and collision-local ordinal for nested declarations.
    stable_contexts_by_identity: BTreeMap<CanonicalSymbolId, (CanonicalSymbolId, u32)>,
    registry_metadata_by_path: BTreeMap<PathBuf, CheckedRegistryMetadataModule>,
    capabilities_by_path: BTreeMap<PathBuf, Vec<CapabilityDeclarationInfo>>,
    c_abi_by_path: BTreeMap<PathBuf, CAbiInteropArtifacts>,
    canonical_target_ids: BTreeMap<CanonicalSymbolId, String>,
    /// Full compiler spans distinguish generated references that share authored coordinates.
    reference_sites: BTreeSet<(String, Span, Option<CanonicalSymbolId>)>,
    /// Call linkage retains expansion context even though public locations contain authored coordinates.
    call_ids: BTreeMap<(String, Span), String>,
}

/// Borrowed source and owner context shared by one expression-record walk. Helper boundaries retain the same
/// module identity and degraded status without copying the parsed module or deriving new checking authority.
struct ExpressionRecordContext<'source> {
    module: &'source ParsedModule,
    module_id: &'source str,
    owner_id: Option<&'source str>,
    degraded: bool,
}

/// Compact source declaration facts used before serializing a public declaration record.
struct DeclarationSummary {
    kind: String,
    name: String,
    visibility: Visibility,
    type_params: Vec<String>,
    signature: Option<String>,
}

/// Stable record id for one namespace.
///
/// The crate root is a real namespace with an empty path, so it needs a spelling of its own rather than an empty
/// suffix that would collide with a malformed id.
fn namespace_id(namespace_path: &[String]) -> String {
    if namespace_path.is_empty() {
        "namespace:<root>".to_string()
    } else {
        format!("namespace:{}", namespace_path.join("."))
    }
}

/// A source identity and independent optional executable/documentation digests.
#[derive(Debug, Clone, Default)]
struct CheckedDeclarationFacts {
    /// The checker's source-signature authority; executable types never enter this identity.
    source_identity: Option<StableDeclarationId>,
    /// Digest over executable meaning, unavailable when Body IR construction fails.
    semantic_digest: Option<String>,
    /// Documentation is independent of semantic identity.
    doc_digest: Option<String>,
}
