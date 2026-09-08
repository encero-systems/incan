//! The encoded form of a module's checked representation, and the contract for refusing one.
//!
//! RFC 123 establishes that a package publishes an executable representation of its public surface, so that what a
//! declaration means does not change when it moves into a package. This module owns the wire form of that
//! representation and nothing else: encoding a checked module, decoding one back, and refusing — in terms a consumer
//! can act on — anything it cannot interpret.
//!
//! Publication projects the checked program onto a public executable closure. Frame-local slot indices remain
//! local to their fragment; checker-local binding identities are removed, and physical declaration addresses are
//! package-scoped projections of canonical identities. A binary index separates explicit coverage and requirements
//! from individually addressable payloads. File I/O belongs to the compiler's artifact resolver.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

mod publication;

use crate::CanonicalSymbolId;
use crate::body_ir::{Body, BodyIrModule, FieldlessEnumDeclaration, NominalDeclaration, ValueEnumDeclaration};

/// Version of the encoded representation, independent of the manifest that ships beside it.
///
/// RFC 123 versions the representation separately so a consumer can refuse an interpretation it does not support
/// without refusing the package: a package remains valid for every route that does not need to execute it.
///
/// Bump this whenever the encoded shape changes in a way an older consumer would misread. A change that only adds an
/// optional field a decoder can ignore does not need a bump; a change to an existing field's meaning or position
/// does, because a consumer has no way to detect it.
pub const EXECUTABLE_REPRESENTATION_VERSION: u32 = 4;

/// One module's checked representation, framed with the version needed to interpret it.
///
/// The version travels inside the encoded bytes rather than beside them, so a representation separated from its
/// manifest still says what it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodedModuleRepresentation {
    /// Encoded-shape version, checked before the module is interpreted.
    pub version: u32,
    /// The checked module this representation carries.
    pub module: BodyIrModule,
}

/// Why a representation could not be interpreted.
///
/// Every variant names what was unusable rather than reporting a generic decode failure, because RFC 123 requires a
/// consumer to refuse "in terms of the package and version it could not use". The package name is supplied by the
/// consumer that resolved it; this layer owns the version and the reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecutableRepresentationError {
    /// The representation was produced by a version this consumer does not implement.
    ///
    /// This is the expected refusal, not a corruption: a newer package encountering an older consumer. It carries
    /// both versions so the consumer can say what it would need in order to proceed.
    #[error(
        "executable representation is version {found}, but this compiler implements version {supported}; the package must be consumed by a compiler that implements version {found}"
    )]
    UnsupportedVersion {
        /// Version the representation declares.
        found: u32,
        /// Version this compiler implements.
        supported: u32,
    },
    /// The bytes were not a representation this decoder could parse at all.
    ///
    /// Distinct from an unsupported version: that is a representation this consumer declines to interpret, while this
    /// is one it could not read far enough to identify. A consumer must not treat this as an absent representation.
    #[error("executable representation could not be decoded: {reason}")]
    Malformed {
        /// Decoder's account of what it could not read.
        reason: String,
    },
    /// The surface is valid and simply does not cover this declaration.
    ///
    /// RFC 123 permits partial coverage, so this is a supported state of a working package, not a defect. It must
    /// never reach a user as an unsupported language construct: the package exports the declaration, and this route
    /// cannot execute it.
    #[error("this package publishes no executable representation for `{declaration}`")]
    DeclarationNotCovered {
        /// Declaration the consumer asked for.
        declaration: String,
    },
}

/// Encode an entire checked module for internal round-trip diagnostics.
///
/// Package publication uses [`build_surface`], which projects public coverage and excludes private declarations.
/// This whole-module codec deliberately retains every checked fact for compiler regression tests.
pub fn encode_module(module: &BodyIrModule) -> Result<Vec<u8>, ExecutableRepresentationError> {
    let framed = EncodedModuleRepresentation {
        version: EXECUTABLE_REPRESENTATION_VERSION,
        module: module.clone(),
    };
    postcard::to_allocvec(&framed).map_err(|error| ExecutableRepresentationError::Malformed {
        reason: format!("could not encode the checked module: {error}"),
    })
}

/// Decode an internal whole-module snapshot, refusing an unfamiliar version before decoding its contents.
///
/// The version is checked before the module is used, so an unfamiliar representation is refused rather than
/// partially interpreted. A decode that succeeds has produced the same checked module the declaring compilation
/// encoded; there is no reconstruction step in which a consumer could arrive at a different answer.
pub fn decode_module(bytes: &[u8]) -> Result<BodyIrModule, ExecutableRepresentationError> {
    require_supported_version(bytes)?;
    let framed: EncodedModuleRepresentation =
        postcard::from_bytes(bytes).map_err(|error| ExecutableRepresentationError::Malformed {
            reason: format!("could not decode the framed representation: {error}"),
        })?;
    if framed.version != EXECUTABLE_REPRESENTATION_VERSION {
        return Err(ExecutableRepresentationError::UnsupportedVersion {
            found: framed.version,
            supported: EXECUTABLE_REPRESENTATION_VERSION,
        });
    }
    Ok(framed.module)
}

/// Read the version a representation declares without interpreting the module it carries.
///
/// A consumer resolving dependencies needs to know whether it can execute a package before it commits to using it.
/// RFC 123 requires that requirement to be settled at resolution time rather than at the first call, and settling it
/// must not mean decoding a surface the consumer may then refuse.
pub fn representation_version(bytes: &[u8]) -> Result<u32, ExecutableRepresentationError> {
    // Postcard encodes struct fields in declaration order, so the leading varint is the version and reading it does
    // not require the module behind it to be interpretable by this consumer — which is the whole point.
    let (version, _) =
        postcard::take_from_bytes::<u32>(bytes).map_err(|error| ExecutableRepresentationError::Malformed {
            reason: format!("could not read the representation version: {error}"),
        })?;
    Ok(version)
}

/// Reject an unfamiliar encoding from its stable leading version, before decoding any version-specific fields.
pub fn require_supported_version(bytes: &[u8]) -> Result<(), ExecutableRepresentationError> {
    let found = representation_version(bytes)?;
    if found != EXECUTABLE_REPRESENTATION_VERSION {
        return Err(ExecutableRepresentationError::UnsupportedVersion {
            found,
            supported: EXECUTABLE_REPRESENTATION_VERSION,
        });
    }
    Ok(())
}

/// Stable reason a public export has no executable fragment. Reasons never disclose private declaration names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoverageReason {
    /// Checked lowering retained an operation with no portable execution contract.
    UnsupportedConstruct,
    /// Execution needs a declaration or layout outside the manifest's public surface.
    PrivateDependency,
    /// The checked body lacks a portable canonical reference or a concrete type fact.
    UnresolvedReference,
    /// A public callee or required type is itself uncovered.
    RequiredDeclarationUnavailable,
    /// This exported declaration has no retained executable body or supported type context.
    NoExecutableDeclaration,
}

/// One individually decodable public declaration. Type context is indexed exactly like executable bodies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExecutableDeclaration {
    /// Checked executable meaning of one public function or method.
    Body(Body),
    /// Checked source field order and canonical members for a public plain model.
    Nominal(NominalDeclaration),
    /// Checked public fieldless enum and canonical variants.
    FieldlessEnum(FieldlessEnumDeclaration),
    /// Checked public value enum and canonical scalar variants.
    ValueEnum(ValueEnumDeclaration),
}

impl ExecutableDeclaration {
    /// Canonical identity carried by this fragment; it must agree with the index identity used to select it.
    pub fn identity(&self) -> Option<&CanonicalSymbolId> {
        match self {
            Self::Body(body) => body.canonical.as_ref(),
            Self::Nominal(value) => Some(&value.canonical),
            Self::FieldlessEnum(value) => Some(&value.canonical),
            Self::ValueEnum(value) => Some(&value.canonical),
        }
    }
}

/// Explicit admission and payload address for one public declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclarationCoverage {
    /// Covered content and its complete direct public requirements; consumers resolve the transitive closure.
    Covered {
        /// Offset relative to the first payload byte.
        offset: u64,
        /// Exact encoded fragment length.
        length: u64,
        /// Canonical declarations whose executable content or public type context this fragment requires.
        requirements: Vec<CanonicalSymbolId>,
    },
    /// Deliberate absence of a fragment, distinct from a covered empty function.
    Uncovered(CoverageReason),
    /// A public member executes through its separately addressed declaring type context.
    TypeContext {
        /// Canonical public type declaration that owns this member.
        owner: CanonicalSymbolId,
    },
}

/// Small version-specific index, decoded only after the stable envelope version is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceIndex {
    /// Declaring package. Consumers compare it with the resolved artifact's manifest.
    pub library: String,
    /// Package version from the same build as the accompanying manifest.
    pub package_version: String,
    /// Declared coverage for every public identity owned by this package.
    pub declarations: BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
}

/// Build one package's deterministic public representation from its single declaring compilation.
///
/// `public` comes from the finalized manifest, including public members. `unrepresentable` records the current
/// execution profile's refusals in addition to this layer's portable-reference checks. Repeatedly removing callers
/// of uncovered owned declarations computes a fixed point, so mutual public recursion is independent of order.
pub fn build_surface(
    modules: &[BodyIrModule],
    library: &str,
    package_version: &str,
    public: &BTreeSet<CanonicalSymbolId>,
    unrepresentable: &BTreeSet<CanonicalSymbolId>,
) -> Result<Vec<u8>, ExecutableRepresentationError> {
    let mut declarations = public.iter().filter(|identity| matches!(&identity.origin, crate::SymbolOrigin::Package { library: owner, .. } if owner == library))
        .map(|identity| (identity.clone(), DeclarationCoverage::Uncovered(CoverageReason::NoExecutableDeclaration)))
        .collect::<BTreeMap<_, _>>();
    let mut admitted: BTreeMap<CanonicalSymbolId, (ExecutableDeclaration, BTreeSet<CanonicalSymbolId>)> =
        BTreeMap::new();
    for module in modules {
        for body in &module.bodies {
            let Some(identity) = body.canonical.as_ref() else {
                continue;
            };
            if !declarations.contains_key(identity) {
                continue;
            }
            let projected = if unrepresentable.contains(identity) {
                Err(CoverageReason::UnsupportedConstruct)
            } else {
                publication::project_body(body, module, library, public)
            };
            match projected {
                Ok(projected) => {
                    admitted.insert(
                        identity.clone(),
                        (ExecutableDeclaration::Body(projected.body), projected.requirements),
                    );
                }
                Err(reason) => {
                    declarations.insert(identity.clone(), DeclarationCoverage::Uncovered(reason));
                }
            }
        }
        for nominal in &module.nominal_declarations {
            if !declarations.contains_key(&nominal.canonical) {
                continue;
            }
            match publication::project_nominal(nominal, module, library, public) {
                Ok((nominal, requirements)) => {
                    admitted.insert(
                        nominal.canonical.clone(),
                        (ExecutableDeclaration::Nominal(nominal), requirements),
                    );
                }
                Err(reason) => {
                    declarations.insert(nominal.canonical.clone(), DeclarationCoverage::Uncovered(reason));
                }
            }
        }
        for value in &module.fieldless_enum_declarations {
            if !declarations.contains_key(&value.canonical)
                || !value.variants.iter().all(|variant| public.contains(&variant.canonical))
            {
                continue;
            }
            let mut value = value.clone();
            value.direct_declaration_id =
                publication::declaration_id(&value.canonical).map_err(|_| malformed("enum has no canonical owner"))?;
            for variant in &mut value.variants {
                variant.direct_declaration_id = publication::declaration_id(&variant.canonical)
                    .map_err(|_| malformed("variant has no canonical owner"))?;
            }
            admitted.insert(
                value.canonical.clone(),
                (ExecutableDeclaration::FieldlessEnum(value), BTreeSet::new()),
            );
        }
        for value in &module.value_enum_declarations {
            if !declarations.contains_key(&value.canonical)
                || !value.variants.iter().all(|variant| public.contains(&variant.canonical))
            {
                continue;
            }
            let mut value = value.clone();
            value.direct_declaration_id = publication::declaration_id(&value.canonical)
                .map_err(|_| malformed("value enum has no canonical owner"))?;
            for variant in &mut value.variants {
                variant.direct_declaration_id = publication::declaration_id(&variant.canonical)
                    .map_err(|_| malformed("variant has no canonical owner"))?;
            }
            admitted.insert(
                value.canonical.clone(),
                (ExecutableDeclaration::ValueEnum(value), BTreeSet::new()),
            );
        }
    }
    loop {
        let rejected = admitted
            .iter()
            .filter(|(_, (_, requirements))| {
                requirements.iter().any(|required| {
                    matches!(&required.origin, crate::SymbolOrigin::Package { library: owner, .. } if owner == library)
                        && !admitted.contains_key(required)
                })
            })
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>();
        if rejected.is_empty() {
            break;
        }
        for identity in rejected {
            admitted.remove(&identity);
            declarations.insert(
                identity,
                DeclarationCoverage::Uncovered(CoverageReason::RequiredDeclarationUnavailable),
            );
        }
    }
    for (identity, (declaration, _)) in &admitted {
        let members = match declaration {
            ExecutableDeclaration::Nominal(value) => value.field_identities.iter().collect::<Vec<_>>(),
            ExecutableDeclaration::FieldlessEnum(value) => {
                value.variants.iter().map(|variant| &variant.canonical).collect()
            }
            ExecutableDeclaration::ValueEnum(value) => {
                value.variants.iter().map(|variant| &variant.canonical).collect()
            }
            ExecutableDeclaration::Body(_) => Vec::new(),
        };
        for member in members {
            declarations.insert(
                member.clone(),
                DeclarationCoverage::TypeContext {
                    owner: identity.clone(),
                },
            );
        }
    }
    let mut payload = Vec::new();
    for (identity, (declaration, requirements)) in admitted {
        let encoded = postcard::to_allocvec(&declaration).map_err(|error| malformed(error.to_string()))?;
        declarations.insert(
            identity,
            DeclarationCoverage::Covered {
                offset: u64::try_from(payload.len()).map_err(|_| malformed("payload offset exceeds wire range"))?,
                length: u64::try_from(encoded.len()).map_err(|_| malformed("fragment length exceeds wire range"))?,
                requirements: requirements.into_iter().collect(),
            },
        );
        payload.extend(encoded);
    }
    let index = SurfaceIndex {
        library: library.to_owned(),
        package_version: package_version.to_owned(),
        declarations,
    };
    let encoded_index = postcard::to_allocvec(&index).map_err(|error| malformed(error.to_string()))?;
    let mut bytes =
        postcard::to_allocvec(&EXECUTABLE_REPRESENTATION_VERSION).map_err(|error| malformed(error.to_string()))?;
    bytes.extend(
        u64::try_from(encoded_index.len())
            .map_err(|_| malformed("index length exceeds wire range"))?
            .to_le_bytes(),
    );
    bytes.extend(encoded_index);
    bytes.extend(payload);
    Ok(bytes)
}

/// Normalize codec and contract errors without coupling this layer to package filesystem diagnostics.
fn malformed(reason: impl Into<String>) -> ExecutableRepresentationError {
    ExecutableRepresentationError::Malformed { reason: reason.into() }
}

impl SurfaceIndex {
    /// Validate duplicate-free canonical ownership, contiguous disjoint ranges, and payload bounds before selection.
    pub fn validate(&self, payload_length: u64) -> Result<(), ExecutableRepresentationError> {
        let mut next_offset = 0;
        for (identity, coverage) in &self.declarations {
            if identity.scope_discriminant.is_some()
                || !matches!(&identity.origin, crate::SymbolOrigin::Package { library, .. } if library == &self.library)
            {
                return Err(malformed("index identity does not belong to the declaring package"));
            }
            if let DeclarationCoverage::TypeContext { owner } = coverage {
                if !matches!(
                    identity.kind,
                    crate::SemanticSourceTargetKind::Field | crate::SemanticSourceTargetKind::Variant
                ) || owner.origin != identity.origin
                    || !matches!(
                        owner.kind,
                        crate::SemanticSourceTargetKind::Model | crate::SemanticSourceTargetKind::Enum
                    )
                    || !matches!(self.declarations.get(owner), Some(DeclarationCoverage::Covered { .. }))
                {
                    return Err(malformed(
                        "public member context does not select a covered declaring type",
                    ));
                }
            }
            if let DeclarationCoverage::Covered {
                offset,
                length,
                requirements,
            } = coverage
            {
                if *offset != next_offset || *length == 0 {
                    return Err(malformed("fragment ranges overlap, contain gaps, or are empty"));
                }
                next_offset = offset
                    .checked_add(*length)
                    .ok_or_else(|| malformed("fragment range overflow"))?;
                if requirements.windows(2).any(|pair| pair[0] >= pair[1]) {
                    return Err(malformed("requirements are duplicated or unsorted"));
                }
            }
        }
        if next_offset != payload_length {
            return Err(malformed("index does not match payload length"));
        }
        Ok(())
    }

    /// Decode a bounded index. The caller must already have accepted the stable envelope version.
    pub fn decode(bytes: &[u8], payload_length: u64) -> Result<Self, ExecutableRepresentationError> {
        let index: Self = postcard::from_bytes(bytes).map_err(|error| malformed(error.to_string()))?;
        // Re-encoding checks map ordering and duplicates, which serde's BTreeMap decoder otherwise normalizes.
        if postcard::to_allocvec(&index).map_err(|error| malformed(error.to_string()))? != bytes {
            return Err(malformed("noncanonical or duplicate index entries"));
        }
        index.validate(payload_length)?;
        Ok(index)
    }

    /// Inspect explicit coverage before any payload is decoded.
    pub fn coverage(
        &self,
        identity: &CanonicalSymbolId,
    ) -> Result<&DeclarationCoverage, ExecutableRepresentationError> {
        self.declarations
            .get(identity)
            .ok_or_else(|| ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            })
    }

    /// Decode exactly the selected range and bind its payload identity back to the index and manifest identity.
    pub fn decode_declaration(
        &self,
        identity: &CanonicalSymbolId,
        bytes: &[u8],
    ) -> Result<ExecutableDeclaration, ExecutableRepresentationError> {
        let DeclarationCoverage::Covered { length, .. } = self.coverage(identity)? else {
            return Err(ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            });
        };
        if u64::try_from(bytes.len()).ok() != Some(*length) {
            return Err(malformed("selected fragment length differs from its index"));
        }
        let declaration: ExecutableDeclaration =
            postcard::from_bytes(bytes).map_err(|error| malformed(error.to_string()))?;
        if declaration.identity() != Some(identity) {
            return Err(malformed("fragment identity differs from its index"));
        }
        Ok(declaration)
    }
}

/// In-memory reader for already-loaded artifact bytes. File consumers use the same index with bounded range reads.
#[derive(Debug, Clone)]
pub struct SurfaceReader<'bytes> {
    index: SurfaceIndex,
    payload: &'bytes [u8],
}

impl<'bytes> SurfaceReader<'bytes> {
    /// Refuse the stable prefix first, then decode only the bounded metadata index.
    pub fn open(bytes: &'bytes [u8]) -> Result<Self, ExecutableRepresentationError> {
        require_supported_version(bytes)?;
        let (_, remainder) = postcard::take_from_bytes::<u32>(bytes).map_err(|error| malformed(error.to_string()))?;
        let length_bytes: [u8; 8] = remainder
            .get(..8)
            .ok_or_else(|| malformed("missing index length"))?
            .try_into()
            .map_err(|_| malformed("invalid index length"))?;
        let index_length =
            usize::try_from(u64::from_le_bytes(length_bytes)).map_err(|_| malformed("index too large"))?;
        let end = 8usize
            .checked_add(index_length)
            .ok_or_else(|| malformed("index length overflow"))?;
        let encoded_index = remainder
            .get(8..end)
            .ok_or_else(|| malformed("index extends beyond representation"))?;
        let payload = remainder.get(end..).ok_or_else(|| malformed("missing payload"))?;
        let index = SurfaceIndex::decode(
            encoded_index,
            u64::try_from(payload.len()).map_err(|_| malformed("payload too large"))?,
        )?;
        Ok(Self { index, payload })
    }

    /// Declared package coverage, inspectable without decoding executable payloads.
    pub fn index(&self) -> &SurfaceIndex {
        &self.index
    }

    /// Canonical identities with covered executable fragments; uncovered exports remain visible through the index.
    pub fn covered_identities(&self) -> impl Iterator<Item = &CanonicalSymbolId> {
        self.index.declarations.iter().filter_map(|(identity, coverage)| {
            matches!(coverage, DeclarationCoverage::Covered { .. }).then_some(identity)
        })
    }

    /// Whether the index explicitly covers this identity.
    pub fn covers(&self, identity: &CanonicalSymbolId) -> bool {
        matches!(
            self.index.declarations.get(identity),
            Some(DeclarationCoverage::Covered { .. })
        )
    }

    /// Decode one public body. A type fragment is never treated as a callable.
    pub fn declaration(&self, identity: &CanonicalSymbolId) -> Result<Body, ExecutableRepresentationError> {
        match self.fragment(identity)? {
            ExecutableDeclaration::Body(body) => Ok(body),
            _ => Err(malformed(
                "selected declaration is type context, not an executable body",
            )),
        }
    }

    /// Decode only one indexed fragment, preserving unrelated payloads as uninterpreted bytes.
    pub fn fragment(
        &self,
        identity: &CanonicalSymbolId,
    ) -> Result<ExecutableDeclaration, ExecutableRepresentationError> {
        let DeclarationCoverage::Covered { offset, length, .. } = self.index.coverage(identity)? else {
            return Err(ExecutableRepresentationError::DeclarationNotCovered {
                declaration: identity.declaration_name.clone(),
            });
        };
        let start = usize::try_from(*offset).map_err(|_| malformed("fragment offset exceeds host range"))?;
        let length = usize::try_from(*length).map_err(|_| malformed("fragment length exceeds host range"))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| malformed("fragment range overflow"))?;
        self.index.decode_declaration(
            identity,
            self.payload
                .get(start..end)
                .ok_or_else(|| malformed("fragment outside payload"))?,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{
        CoverageReason, DeclarationCoverage, EXECUTABLE_REPRESENTATION_VERSION, ExecutableDeclaration,
        ExecutableRepresentationError, SurfaceReader, build_surface, decode_module,
    };
    use crate::body_ir::{
        ArgumentBinding, Block, Body, BodyIrModule, CallableParam, CallableParamDefault, CallableTarget, Callee,
        NamedCallableTarget, ScopeId, Statement, StatementKind,
    };
    use crate::{
        CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
        SymbolNamespace, SymbolOrigin,
    };

    /// Distinct package identities remain stable even when input bodies are reordered.
    fn identity(name: &str, ordinal: usize) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Package {
                library: "probe".into(),
                module_path: vec!["lib".into()],
            },
            declaration_name: name.into(),
            kind: SemanticSourceTargetKind::Function,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(ordinal * 100, ordinal * 100 + 99),
        }
    }

    /// A checked, empty unit function has executable coverage distinct from an uncovered export.
    fn body(name: &str, ordinal: usize) -> Body {
        let identity = identity(name, ordinal);
        Body {
            decl_id: CompilerNodeId::declaration_span(
                "lib",
                identity.declaration_span.start,
                identity.declaration_span.end,
            ),
            direct_call_id: CompilerNodeId::declaration_span(
                "lib",
                identity.declaration_span.start,
                identity.declaration_span.end,
            ),
            canonical: Some(identity.clone()),
            name: name.into(),
            span: identity.declaration_span,
            return_type: IncanType::Primitive(IncanPrimitiveType::Unit),
            named_type_identities: Default::default(),
            locals: Vec::new(),
            params: Vec::new(),
            param_locals: Vec::new(),
            scopes: Vec::new(),
            block: Block {
                scope: ScopeId(0),
                stmts: Vec::new(),
            },
            runtime_requirements: Vec::new(),
            panic_facts: Vec::new(),
            is_async: false,
        }
    }

    /// Synthetic module context used only for codec and public-closure invariants.
    fn module(bodies: Vec<Body>) -> BodyIrModule {
        BodyIrModule {
            module_id: CompilerNodeId::module("lib"),
            nominal_declarations: Vec::new(),
            fieldless_enum_declarations: Vec::new(),
            value_enum_declarations: Vec::new(),
            bodies,
        }
    }

    /// Build with an explicit public set, as the finalized manifest supplies at the real producer seam.
    fn publish(
        module: &BodyIrModule,
        public: &BTreeSet<CanonicalSymbolId>,
    ) -> Result<Vec<u8>, ExecutableRepresentationError> {
        build_surface(std::slice::from_ref(module), "probe", "1.2.3", public, &BTreeSet::new())
    }

    /// A canonical call participates in closure even before any branch is executed.
    fn call(target: CanonicalSymbolId) -> Statement {
        Statement {
            span: target.declaration_span,
            kind: StatementKind::Call {
                destination: None,
                callee: Callee::Function(CallableTarget::Named(NamedCallableTarget {
                    name: "arbitrary_alias".into(),
                    direct_call_id: None,
                    builtin: None,
                    type_args: Vec::new(),
                    binding: ArgumentBinding::UnresolvedPositional,
                    canonical: Some(target),
                })),
                args: Vec::new(),
                may_panic: false,
            },
        }
    }

    #[test]
    fn version_refusal_precedes_incompatible_header_or_module_decode() -> Result<(), Box<dyn std::error::Error>> {
        for version in [
            EXECUTABLE_REPRESENTATION_VERSION - 1,
            EXECUTABLE_REPRESENTATION_VERSION + 1,
        ] {
            let bytes = postcard::to_allocvec(&version)?;
            assert!(matches!(
                SurfaceReader::open(&bytes),
                Err(ExecutableRepresentationError::UnsupportedVersion { .. })
            ));
            assert!(matches!(
                decode_module(&bytes),
                Err(ExecutableRepresentationError::UnsupportedVersion { .. })
            ));
        }
        assert!(matches!(
            SurfaceReader::open(&[255; 12]),
            Err(ExecutableRepresentationError::Malformed { .. })
        ));
        Ok(())
    }

    /// A missing source field identity must not be mistaken for a compiler-proven structural projection.
    #[test]
    fn unresolved_named_fields_are_uncovered_and_synthesized_projections_remain_covered()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::body_ir::{LocalId, Operand, OwnershipFact, Place, PlaceElem};
        let mut unresolved = body("unresolved", 1);
        let mut place = Place::from_local(LocalId(0));
        place.projection.push(PlaceElem::field("private_secret", None));
        unresolved.block.stmts.push(Statement {
            span: unresolved.span,
            kind: StatementKind::Return {
                value: Some(Operand::place(place.clone(), OwnershipFact::Copy, false)),
            },
        });
        let mut structural = body("structural", 2);
        place.projection = vec![PlaceElem::synthetic_field("0")];
        structural.block.stmts.push(Statement {
            span: structural.span,
            kind: StatementKind::Return {
                value: Some(Operand::place(place, OwnershipFact::Copy, false)),
            },
        });
        let bytes = publish(
            &module(vec![unresolved, structural]),
            &BTreeSet::from([identity("unresolved", 1), identity("structural", 2)]),
        )?;
        let reader = SurfaceReader::open(&bytes)?;
        assert!(matches!(
            reader.index().coverage(&identity("unresolved", 1))?,
            DeclarationCoverage::Uncovered(CoverageReason::UnresolvedReference)
        ));
        assert!(reader.covers(&identity("structural", 2)));
        assert!(
            !bytes
                .windows(b"private_secret".len())
                .any(|part| part == b"private_secret")
        );
        Ok(())
    }

    #[test]
    fn public_membership_and_explicit_empty_body_coverage() -> Result<(), Box<dyn std::error::Error>> {
        let module = module(vec![body("public", 1), body("private_secret", 2)]);
        let public = BTreeSet::from([identity("public", 1), identity("unlowered", 3)]);
        let bytes = publish(&module, &public)?;
        let reader = SurfaceReader::open(&bytes)?;
        assert!(reader.covers(&identity("public", 1)));
        assert!(matches!(
            reader.index().coverage(&identity("unlowered", 3))?,
            DeclarationCoverage::Uncovered(CoverageReason::NoExecutableDeclaration)
        ));
        assert!(
            !bytes
                .windows(b"private_secret".len())
                .any(|part| part == b"private_secret")
        );
        assert_eq!(
            reader.declaration(&identity("public", 1))?.direct_call_id.path(),
            "pub::probe::lib#decl.100..199"
        );
        Ok(())
    }

    #[test]
    fn private_and_unsupported_dependencies_are_uncovered_transitively() -> Result<(), Box<dyn std::error::Error>> {
        let mut caller = body("caller", 1);
        caller.block.stmts.push(call(identity("private_secret", 2)));
        let mut dependent = body("dependent", 3);
        dependent.block.stmts.push(call(identity("caller", 1)));
        let mut unsupported = body("unsupported", 4);
        unsupported.block.stmts.push(Statement {
            span: unsupported.span,
            kind: StatementKind::Unsupported {
                description: "never serialized".into(),
            },
        });
        let module = module(vec![caller, body("private_secret", 2), dependent, unsupported]);
        let public = BTreeSet::from([
            identity("caller", 1),
            identity("dependent", 3),
            identity("unsupported", 4),
        ]);
        let bytes = publish(&module, &public)?;
        let reader = SurfaceReader::open(&bytes)?;
        assert_eq!(reader.covered_identities().count(), 0);
        assert!(matches!(
            reader.index().coverage(&identity("caller", 1))?,
            DeclarationCoverage::Uncovered(CoverageReason::PrivateDependency)
        ));
        assert!(matches!(
            reader.index().coverage(&identity("dependent", 3))?,
            DeclarationCoverage::Uncovered(CoverageReason::RequiredDeclarationUnavailable)
        ));
        assert!(
            !bytes
                .windows(b"private_secret".len())
                .any(|part| part == b"private_secret")
        );
        Ok(())
    }

    #[test]
    fn recursive_public_closure_is_order_independent() -> Result<(), Box<dyn std::error::Error>> {
        let mut alpha = body("alpha", 1);
        let mut beta = body("beta", 2);
        alpha.block.stmts.push(call(identity("beta", 2)));
        beta.block.stmts.push(call(identity("alpha", 1)));
        let public = BTreeSet::from([identity("alpha", 1), identity("beta", 2)]);
        let forward = publish(&module(vec![alpha.clone(), beta.clone()]), &public)?;
        let reverse = publish(&module(vec![beta, alpha]), &public)?;
        assert_eq!(forward, reverse);
        assert_eq!(SurfaceReader::open(&forward)?.covered_identities().count(), 2);
        Ok(())
    }

    #[test]
    fn private_default_dependency_is_not_hidden_by_an_empty_main_block() -> Result<(), Box<dyn std::error::Error>> {
        let mut exported = body("exported", 1);
        exported.params.push(CallableParam {
            local: crate::body_ir::LocalId(0),
            name: "x".into(),
            ty: IncanType::Primitive(IncanPrimitiveType::Int),
            span: exported.span,
            default: CallableParamDefault::Source(Box::new(crate::body_ir::DefaultComputation {
                span: exported.span,
                stmts: vec![call(identity("secret", 2))],
                result: crate::body_ir::Operand::Constant(crate::body_ir::Constant::Int(1)),
            })),
        });
        let bytes = publish(
            &module(vec![exported, body("secret", 2)]),
            &BTreeSet::from([identity("exported", 1)]),
        )?;
        assert!(!SurfaceReader::open(&bytes)?.covers(&identity("exported", 1)));
        Ok(())
    }

    #[test]
    fn three_selected_fragments_ignore_unrelated_corrupt_payloads() -> Result<(), Box<dyn std::error::Error>> {
        let bodies = (0..400)
            .map(|index| body(&format!("f{index:03}"), index))
            .collect::<Vec<_>>();
        let public = bodies.iter().filter_map(|body| body.canonical.clone()).collect();
        let mut bytes = publish(&module(bodies), &public)?;
        let last = bytes.len().checked_sub(1).ok_or("surface cannot be empty")?;
        bytes[last] ^= 255;
        let reader = SurfaceReader::open(&bytes)?;
        assert_eq!(reader.covered_identities().count(), 400);
        for index in [1, 2, 3] {
            assert_eq!(
                reader.declaration(&identity(&format!("f{index:03}"), index))?.name,
                format!("f{index:03}")
            );
        }
        assert!(reader.declaration(&identity("f399", 399)).is_err());
        Ok(())
    }

    #[test]
    fn index_refuses_overlaps_and_payload_identity_substitution() -> Result<(), Box<dyn std::error::Error>> {
        let public = BTreeSet::from([identity("alpha", 1), identity("beta", 2)]);
        let bytes = publish(&module(vec![body("alpha", 1), body("beta", 2)]), &public)?;
        let reader = SurfaceReader::open(&bytes)?;
        let mut index = reader.index().clone();
        let Some(DeclarationCoverage::Covered { offset, .. }) = index.declarations.get_mut(&identity("beta", 2)) else {
            return Err("covered beta missing".into());
        };
        *offset = 0;
        assert!(index.validate(u64::try_from(reader.payload.len())?).is_err());
        let wrong = postcard::to_allocvec(&ExecutableDeclaration::Body(reader.declaration(&identity("beta", 2))?))?;
        assert!(
            reader
                .index()
                .decode_declaration(&identity("alpha", 1), &wrong)
                .is_err()
        );
        Ok(())
    }
    /// Typed locals count toward the public closure, and a public type cannot publish private layout members.
    #[test]
    fn public_type_context_requires_public_fields_and_no_private_type_leak() -> Result<(), Box<dyn std::error::Error>> {
        let mut type_id = identity("Record", 2);
        type_id.kind = SemanticSourceTargetKind::Model;
        let mut field = identity("private_field", 3);
        field.kind = SemanticSourceTargetKind::Field;
        field.namespace = SymbolNamespace::Member;
        let mut exported = body("exported", 1);
        exported.return_type = IncanType::Named("Record".into());
        let mut module = module(vec![exported]);
        module.nominal_declarations.push(crate::body_ir::NominalDeclaration {
            direct_declaration_id: CompilerNodeId::declaration_span("lib", 200, 299),
            canonical: type_id.clone(),
            name: "Record".into(),
            fields: vec!["private_field".into()],
            field_identities: vec![field.clone()],
            field_types: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
            named_type_identities: Default::default(),
            type_parameter_count: 0,
        });
        let public = BTreeSet::from([identity("exported", 1), type_id.clone()]);
        let bytes = publish(&module, &public)?;
        let reader = SurfaceReader::open(&bytes)?;
        assert!(!reader.covers(&identity("exported", 1)));
        assert!(!reader.covers(&type_id));
        assert!(
            !bytes
                .windows(b"private_field".len())
                .any(|part| part == b"private_field")
        );
        let mut complete_public = public;
        complete_public.insert(field.clone());
        let bytes = publish(&module, &complete_public)?;
        let reader = SurfaceReader::open(&bytes)?;
        assert!(reader.covers(&identity("exported", 1)));
        assert!(reader.covers(&type_id));
        assert!(
            matches!(reader.index().coverage(&field)?, DeclarationCoverage::TypeContext { owner } if owner == &type_id)
        );
        let nominal = module.nominal_declarations.first_mut().ok_or("public model absent")?;
        nominal.field_types[0] = IncanType::Named("private_layout_secret".into());
        let mut private_type = identity("private_layout_secret", 4);
        private_type.kind = SemanticSourceTargetKind::Model;
        nominal
            .named_type_identities
            .insert("private_layout_secret".into(), private_type);
        let bytes = publish(&module, &complete_public)?;
        assert!(!SurfaceReader::open(&bytes)?.covers(&type_id));
        assert!(
            !bytes
                .windows(b"private_layout_secret".len())
                .any(|part| part == b"private_layout_secret")
        );
        Ok(())
    }

    /// Foreign type bindings retain distinct declared origins, while unused private checker bindings stay absent.
    #[test]
    fn checked_nominal_type_bindings_are_pruned_and_contribute_public_requirements()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut first = identity("Product", 2);
        first.kind = SemanticSourceTargetKind::Model;
        first.origin = SymbolOrigin::Package {
            library: "catalog".into(),
            module_path: vec!["lib".into()],
        };
        let mut second = first.clone();
        second.origin = SymbolOrigin::Package {
            library: "other".into(),
            module_path: vec!["lib".into()],
        };
        let mut exported = body("exported", 1);
        exported.return_type = IncanType::Tuple(vec![
            IncanType::Named("First".into()),
            IncanType::Named("Second".into()),
        ]);
        exported.params.push(CallableParam {
            local: crate::body_ir::LocalId(0),
            name: "product".into(),
            ty: IncanType::Named("First".into()),
            span: exported.span,
            default: CallableParamDefault::Required,
        });
        exported.named_type_identities = std::collections::BTreeMap::from([
            ("First".into(), first.clone()),
            ("Second".into(), second.clone()),
            ("unused_private_secret".into(), identity("unused_private_secret", 3)),
        ]);
        let bytes = publish(&module(vec![exported]), &BTreeSet::from([identity("exported", 1)]))?;
        let reader = SurfaceReader::open(&bytes)?;
        let decoded = reader.declaration(&identity("exported", 1))?;
        assert_eq!(
            decoded.named_type_identities,
            std::collections::BTreeMap::from([("First".into(), first.clone()), ("Second".into(), second.clone())])
        );
        let DeclarationCoverage::Covered { requirements, .. } = reader.index().coverage(&identity("exported", 1))?
        else {
            return Err("foreign typed body uncovered".into());
        };
        assert_eq!(
            requirements.iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([first, second])
        );
        assert!(
            !bytes
                .windows(b"unused_private_secret".len())
                .any(|part| part == b"unused_private_secret")
        );
        Ok(())
    }

    /// Every retained Result type identity is public-audited and rebased to the declaring package.
    #[test]
    fn result_type_identities_are_rebased_and_private_identity_payloads_are_not_published()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::body_ir::{Constant, LocalId, Operand, Place, ResultVariant, ResultVariantKind, Rvalue};
        let mut published_type = identity("Record", 2);
        published_type.kind = SemanticSourceTargetKind::Model;
        let mut source_type = published_type.clone();
        source_type.origin = SymbolOrigin::Module(vec!["lib".into()]);
        let mut exported = body("exported", 1);
        exported.block.stmts.push(Statement {
            span: exported.span,
            kind: StatementKind::Assign {
                place: Place::from_local(LocalId(0)),
                rvalue: Rvalue::ResultVariant(ResultVariant {
                    kind: ResultVariantKind::Ok,
                    payload: Operand::Constant(Constant::Unit),
                    ok_type: IncanType::Named("Record".into()),
                    error_type: IncanType::Primitive(IncanPrimitiveType::Unit),
                    canonical_types: std::collections::BTreeMap::from([(vec![0], source_type.clone())]),
                }),
            },
        });
        let mut input = module(vec![exported]);
        input.nominal_declarations.push(crate::body_ir::NominalDeclaration {
            direct_declaration_id: CompilerNodeId::declaration_span("lib", 200, 299),
            canonical: published_type.clone(),
            name: "Record".into(),
            fields: vec![],
            field_identities: vec![],
            field_types: vec![],
            named_type_identities: Default::default(),
            type_parameter_count: 0,
        });
        let public = BTreeSet::from([identity("exported", 1), published_type.clone()]);
        let bytes = publish(&input, &public)?;
        let decoded = SurfaceReader::open(&bytes)?.declaration(&identity("exported", 1))?;
        let Some(Statement {
            kind:
                StatementKind::Assign {
                    rvalue: Rvalue::ResultVariant(variant),
                    ..
                },
            ..
        }) = decoded.block.stmts.first()
        else {
            return Err("Result type context disappeared".into());
        };
        assert_eq!(variant.canonical_types.get(&vec![0]), Some(&published_type));
        let Some(Statement {
            kind:
                StatementKind::Assign {
                    rvalue: Rvalue::ResultVariant(variant),
                    ..
                },
            ..
        }) = input.bodies.first_mut().and_then(|body| body.block.stmts.first_mut())
        else {
            return Err("source Result disappeared".into());
        };
        let mut private = identity("private_type_secret", 3);
        private.kind = SemanticSourceTargetKind::Model;
        variant.canonical_types.insert(vec![0], private);
        let bytes = publish(&input, &public)?;
        assert!(!SurfaceReader::open(&bytes)?.covers(&identity("exported", 1)));
        assert!(
            !bytes
                .windows(b"private_type_secret".len())
                .any(|bytes| bytes == b"private_type_secret")
        );
        Ok(())
    }

    /// Local binding identity is checker-session metadata; fragment-local slots retain execution identity instead.
    #[test]
    fn publication_removes_session_local_binding_discriminants() -> Result<(), Box<dyn std::error::Error>> {
        let mut exported = body("exported", 1);
        let mut local_identity = identity("local", 2);
        local_identity.scope_discriminant = Some(crate::ScopeDiscriminant(917));
        exported.locals.push(crate::body_ir::LocalDecl {
            id: crate::body_ir::LocalId(0),
            name: Some("local".into()),
            identity: Some(local_identity),
            ty: IncanType::Primitive(IncanPrimitiveType::Int),
            origin: crate::body_ir::LocalOrigin::UserBinding,
            scope: ScopeId(0),
            span: exported.span,
        });
        let bytes = publish(&module(vec![exported]), &BTreeSet::from([identity("exported", 1)]))?;
        let decoded = SurfaceReader::open(&bytes)?.declaration(&identity("exported", 1))?;
        assert_eq!(decoded.locals.first().ok_or("local slot disappeared")?.identity, None);
        assert_eq!(
            decoded.locals.first().ok_or("local slot disappeared")?.id,
            crate::body_ir::LocalId(0)
        );
        Ok(())
    }
}
