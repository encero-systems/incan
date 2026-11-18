//! Declaration identities and source origins shared across compiler stages.

use serde::{Deserialize, Serialize};

pub const IDENTITY_ADAPTER_PROFILE: &str = "incan.compiler-identities/1";
pub const IDENTITY_ADAPTER_SOURCE: &str = "incan_semantics_core identities at executable representation 53";
pub const IDENTITY_ADAPTER_GENERATION: i64 = 1;
pub const IDENTITY_ADAPTER_REMOVAL_GATE: &str =
    "remove only after every direct native identity and source-span consumer accepts the Incan-owned declarations";
pub const RUNTIME_INSTANCE_IDENTITY: &str = "runtime:";

/// Resolution namespace of a checked binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolNamespace {
    OrdinaryLexical,
    Member,
    ModulePath,
}

/// Selected immutable package version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageIdentity {
    pub name: String,
    pub version: String,
}

/// Semantic owner of a declaration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolOrigin {
    Module(Vec<String>),
    Package(PackageIdentity, Vec<String>),
    RustCrate(Vec<String>),
    Builtin,
}

/// Declaration categories in their serialized order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclarationKind {
    Function,
    Model,
    Class,
    Newtype,
    Rusttype,
    Enum,
    TypeAlias,
    Partial,
    Variant,
    Trait,
    Capability,
    Field,
    Method,
    Property,
    Const,
    Static,
    Local,
    Parameter,
    Receiver,
    GenericBinder,
    Module,
    RustItem,
    Builtin,
    Other(String),
}

/// Inclusive-exclusive byte range with optional expansion identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HirSourceSpan {
    pub file: String,
    pub start: i64,
    pub end: i64,
    pub expansion: Option<Vec<i64>>,
}

/// Plan source position as it crosses the record boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSourceSpanRecord {
    pub file: String,
    pub line: i64,
    pub column: i64,
}

/// A declaration in one compilation, bound to its exact source origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalSymbolId {
    pub namespace: SymbolNamespace,
    pub origin: SymbolOrigin,
    pub declaration_name: String,
    pub kind: DeclarationKind,
    pub scope_discriminant: Option<i64>,
    pub declaration_span: HirSourceSpan,
}

/// Span-independent checked signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationSignature {
    pub spelling: String,
}

/// One element in a root-to-nearest-owner path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableDeclarationComponent {
    pub namespace: SymbolNamespace,
    pub origin: SymbolOrigin,
    pub declaration_name: String,
    pub kind: DeclarationKind,
    pub binding_ordinal: Option<i64>,
    pub signature: Option<DeclarationSignature>,
}

/// Location relative to the semantic owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StableDeclarationLocation {
    ModuleLevel,
    Nested(Vec<StableDeclarationComponent>, i64),
}

/// Identity independent of source positions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StableDeclarationId {
    pub namespace: SymbolNamespace,
    pub origin: SymbolOrigin,
    pub declaration_name: String,
    pub kind: DeclarationKind,
    pub location: StableDeclarationLocation,
    pub signature: Option<DeclarationSignature>,
}

/// A span has an ordered nonnegative range and, when supplied, exactly 32 octets of expansion identity.
pub fn valid_hir_source_span(span: &HirSourceSpan) -> bool {
    !span.file.is_empty()
        && span.start >= 0
        && span.end >= span.start
        && span
            .expansion
            .as_ref()
            .is_none_or(|bytes| bytes.len() == 32 && bytes.iter().all(|n| (0..=255).contains(n)))
}

/// Check the origin, scope and source span of this binding.
pub fn valid_canonical_symbol_id(id: &CanonicalSymbolId) -> bool {
    !id.declaration_name.is_empty()
        && valid_origin(&id.origin)
        && id.scope_discriminant.is_none_or(|ordinal| ordinal >= 0)
        && valid_hir_source_span(&id.declaration_span)
}

/// Check the optional signature and the complete ordered owner path.
pub fn valid_stable_declaration_id(id: &StableDeclarationId) -> bool {
    if id.declaration_name.is_empty() || !valid_origin(&id.origin) || !valid_signature(&id.signature) {
        return false;
    }
    match &id.location {
        StableDeclarationLocation::ModuleLevel => true,
        StableDeclarationLocation::Nested(owners, ordinal) => {
            *ordinal >= 0
                && !owners.is_empty()
                && owners.iter().enumerate().all(|(i, owner)| {
                    !owner.declaration_name.is_empty()
                        && valid_origin(&owner.origin)
                        && valid_signature(&owner.signature)
                        && match owner.binding_ordinal {
                            None => i == 0,
                            Some(n) => i > 0 && n >= 0,
                        }
                })
        }
    }
}

/// Only package owners and Rust crate paths have additional nonempty fields.
fn valid_origin(origin: &SymbolOrigin) -> bool {
    match origin {
        SymbolOrigin::Package(package, _) => !package.name.is_empty() && !package.version.is_empty(),
        SymbolOrigin::RustCrate(path) => !path.is_empty(),
        SymbolOrigin::Module(_) | SymbolOrigin::Builtin => true,
    }
}

/// A present signature must carry its spelling.
fn valid_signature(signature: &Option<DeclarationSignature>) -> bool {
    signature.as_ref().is_none_or(|value| !value.spelling.is_empty())
}
