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

mod module_codec;
mod publication;
mod surface_build;
mod surface_reading;

pub use module_codec::*;
pub use surface_build::*;

use crate::CanonicalSymbolId;
use crate::CompilerNodeId;
use crate::body_ir::{
    Body, BodyIrModule, EnumDeclaration, FieldlessEnumDeclaration, NominalDeclaration, TraitFieldImplementation,
    ValueEnumDeclaration,
};

/// Version of the encoded representation, independent of the manifest that ships beside it.
///
/// RFC 123 versions the representation separately so a consumer can refuse an interpretation it does not support
/// without refusing the package: a package remains valid for every route that does not need to execute it.
///
/// Every incompatible encoded shape needs a new version before consumers decode a payload. Postcard struct fields
/// are positional: adding a serde-defaulted field does not authorize an older consumer to reinterpret that payload.
/// Version 19 appends checked numeric newtype constants and their canonical constructor/layout requirements.
/// Version 20 appends the canonical diverging ValueError helper operation.
/// Version 21 is the Loaf representation (DD-0007): Loaf callables replace standard-library delegations, enum
/// declarations name their compiled item, and modules no longer carry re-exported names.
/// Version 22 appends metadata-selected native Rust constants with their checked carrier types.
/// Version 23 appends builtin codec label guards; their conditional evaluation is retained in ordinary Body IR blocks.
/// Version 24 distinguishes compiled callable contracts from caller-instantiated executable bodies.
/// Version 25 retains callable union payload signatures and producer-owned captured environment aliases.
/// Version 26 appends native fixed-size arrays with their checked element storage and evaluated length.
/// Version 27 retains checked native callable signatures with deferred executable bodies.
/// Version 28 retains native default callees alongside compiled callable frames and appends checked panic-hook
/// registration.
/// Version 29 retains checked owned callback storage and native Send/Sync capture obligations.
/// Version 30 carries checked required-field witnesses in nominal fragment trailers.
/// Version 31 appends the pure generated field-table operation for closed type parameters.
/// Version 32 retains owned native collection parameter ABIs independently of source callable mutability markers.
/// Version 33 retains verified foreign call descriptors independently of source class layouts.
/// Version 34 retains checked private pointer carriers and foreign bridge operations.
/// Version 35 retains expansion contexts in every checked source span and canonical identity.
/// Version 36 appends the typed C span bridge operations.
/// Version 37 carries explicit input access independently of binding mutability and native collection ownership.
/// Version 38 retains checked generic value metadata; version 39 adds a source-owned explicit-input body profile.
/// Version 40 combines the input and reflection contracts. Their independently developed positional layouts are
/// incompatible with this combined representation and require regenerated providers.
/// Version 41 retains canonical whole-owner initializer transfers independently of ordinary native assignments.
/// Every provider must be rebuilt; an older profile cannot silently discard the added positional field.
/// Version 42 retains each explicit local binding on its body, so a body that loses its checked profile still
/// exposes the explicit-input obligation.
/// Version 43 retains a body's returned view: the parameter slot its result borrows and the view's access.
/// Version 44 executes conditional explicit bodies: a checked input profile 2 is admitted, and its owned initializer
/// transfers travel with the body.
/// Version 45 marks a returned view carried in a `Result` or `Option` payload, so a caller keeps the source loan
/// through `?` instead of taking the carrier as an owned value.
/// Version 46 records which payload carries a returned view, so a view may travel in a `Result` error as well as in
/// its success payload.
/// Version 47 lets a tuple element carry a returned view, so a result can pair a view with owned values.
/// Version 48 distinguishes body-inferred shared inputs from explicit `Borrowed` inputs while preserving their shared
/// native ABI. Older representations carry no such fact and retain their existing inferred meaning.
/// Version 49 carries the checker's typed builtin-method operation on every method target. Older representations are
/// refused because an absent operation would force consumers to reconstruct builtin meaning from canonical text.
/// Version 50 carries the primary source label in persisted checker diagnostics. Older representations are refused
/// because decoding the prior diagnostic row shape would shift every following structured field.
/// Version 51 closes the builtin-method operation over text, iterator, carrier, clone, and numeric methods as well, so
/// partial operation registries are refused rather than reintroducing name-based dispatch.
pub const EXECUTABLE_REPRESENTATION_VERSION: u32 = 51;

/// Only the binding-aware combined layout is admitted. Missing input authority cannot be reconstructed from a
/// native ABI, and a prior layout cannot silently omit initializer transfers, explicit local obligations, returned
/// views, conditional profiles or the component carrying a view.
const OLDEST_SUPPORTED_VERSION: u32 = 51;

/// Required-field witnesses transported separately from the legacy positional nominal layout.
#[derive(Serialize, Deserialize)]
struct TraitFieldBindings {
    owner: CanonicalSymbolId,
    trait_owner: CanonicalSymbolId,
    fields: Vec<TraitFieldImplementation>,
}

/// Largest index this format admits, in bytes.
///
/// The index is read before anything about the file has been trusted, so its declared length is the one number an
/// attacker controls that decides an allocation. Bounding it here, in the module that writes the frame, keeps
/// every reader agreeing on what is admissible: a streaming reader and an in-memory one that disagree would
/// accept and refuse the same artifact depending only on how it was opened.
pub const MAX_SURFACE_INDEX_BYTES: u64 = 16 * 1024 * 1024;

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

/// Stable reason a public export has no executable fragment. Reasons never disclose private declaration names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoverageReason {
    /// Checked lowering retained an operation with no portable execution contract.
    UnsupportedConstruct,
    /// Execution needs a same-package declaration not retained by the declaring compilation.
    PrivateDependency,
    /// The checked body lacks a portable canonical reference or a concrete type fact.
    UnresolvedReference,
    /// A public callee or required type is itself uncovered.
    RequiredDeclarationUnavailable,
    /// This exported declaration has no retained executable body or supported type context.
    NoExecutableDeclaration,
}

/// One individually decodable execution declaration. Only public index entries are importable roots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExecutableDeclaration {
    /// Checked executable meaning of one selected function or method.
    Body(Box<Body>),
    /// Checked source field order and canonical members for a plain model.
    Nominal(NominalDeclaration),
    /// Checked fieldless enum and canonical variants.
    FieldlessEnum(FieldlessEnumDeclaration),
    /// Checked value enum and canonical scalar variants.
    ValueEnum(ValueEnumDeclaration),
    /// Checked scalar variants together with the identical native carrier and its derive selections.
    ValueEnumWithLayout(ValueEnumDeclaration, EnumDeclaration),
    /// Checked normal enum, including its positional payload layouts.
    Enum(EnumDeclaration),
    /// Checked trait identity; method bodies are independently addressed fragments.
    Trait(CanonicalSymbolId),
    /// Nominal layout with checker-proven concrete trait dispatch slots.
    NominalWithTraits(NominalDeclaration, Vec<crate::body_ir::TraitImplementation>),
    /// Checker-evaluated immutable scalar or text, with no runtime storage.
    Constant(crate::body_ir::ConstantDeclaration),
    /// Alias-expanded semantic type, including union members and their canonical dependencies.
    TypeAlias(crate::body_ir::TypeAliasDeclaration),
    /// Canonical storage and its checked effect-free initialization, loaded once for requiring bodies.
    Static(crate::body_ir::StaticDeclaration),
    /// The visible callable and its checked original and once-initialized decorator replacement are indivisible.
    DecoratedBody {
        /// Ordinary calls select this checked wrapper, never the original body.
        body: Box<Body>,
        /// Only an initializer's explicit original-callable read may select this source implementation.
        binding: Box<crate::body_ir::DecoratedBinding>,
        /// Checked replacement storage and computation under the same canonical source identity.
        storage: Box<crate::body_ir::StaticDeclaration>,
    },
    /// Retired serialized discriminant; identity validation refuses the former per-call decorator fragment.
    InvocationDecoratedBody {
        /// Ordinary calls select this wrapper using the call site's checked type arguments.
        body: Box<Body>,
        /// Only explicit original-callable reads select this implementation under the same declared binders.
        binding: Box<crate::body_ir::DecoratedBinding>,
    },
    /// Deferred checked meaning and the exact native instantiations its calls select.
    NativeBody {
        /// The generic or otherwise caller-instantiated source frame.
        body: Box<Body>,
        /// Checker-produced signatures, matched by canonical identity and native type arguments.
        callables: Vec<crate::body_ir::LoafCallable>,
    },
    /// Producer-authenticated passing modes and defaults of a compiled callable, with no executable implementation.
    /// Consumers call the canonical rlib export; this frame never joins their executable body queue.
    CompiledCallableFrame {
        /// Exact declaration-local parameter passing, representation and default computations.
        frame: Box<Body>,
        /// Producer-selected native callees used by retained default computations.
        callables: Vec<crate::body_ir::LoafCallable>,
    },
    /// A generic wrapper, original and initializer template close together and use the checked per-instance policy.
    GenericDecoratedBody {
        /// Ordinary calls select this wrapper with their checked type arguments.
        body: Box<Body>,
        /// The original and initializer retain the same canonical declaration and exact type parameters.
        binding: Box<crate::body_ir::DecoratedBinding>,
        /// Producer-selected native callees required by any of the three retained frames.
        callables: Vec<crate::body_ir::LoafCallable>,
    },
}

impl ExecutableDeclaration {
    /// Canonical identity carried by this fragment; it must agree with the selecting index. Native-body records may
    /// transport only exact Rust callable identities, never authorize a package declaration outside that index.
    pub fn identity(&self) -> Option<&CanonicalSymbolId> {
        match self {
            Self::Body(body) => body.canonical.as_ref(),
            Self::NativeBody { body, callables } => (!callables.is_empty() && valid_native_calls(callables))
                .then_some(body.canonical.as_ref())
                .flatten(),
            Self::CompiledCallableFrame { frame, callables } => (frame.block.stmts.is_empty()
                && valid_native_calls(callables)
                && crate::body_ir::LoafCallable::from_body(frame).is_some())
            .then_some(frame.canonical.as_ref())
            .flatten(),
            Self::InvocationDecoratedBody { .. } => None,
            Self::GenericDecoratedBody {
                body,
                binding,
                callables,
            } => {
                let identity = body.canonical.as_ref()?;
                let mut module = BodyIrModule::declarations_only(crate::canonical_module_identity(identity)?);
                module.bodies.push(body.as_ref().clone());
                (valid_native_calls(callables) && module.is_well_formed_decorated_binding(binding)).then_some(identity)
            }
            Self::DecoratedBody { body, binding, storage } => {
                let identity = body.canonical.as_ref()?;
                let crate::body_ir::StaticInitializer::Computed(initializer) = &storage.initial else {
                    return None;
                };
                (binding.initialization == crate::body_ir::DecoratedInitialization::ModuleStorage
                    && binding.initializer.is_none()
                    && binding.original_body.canonical.as_ref() == Some(identity)
                    && &storage.canonical == identity
                    && initializer.canonical.as_ref() == Some(identity)
                    && binding.original_body.direct_call_id == body.direct_call_id
                    && initializer.decl_id == body.decl_id)
                    .then_some(identity)
            }
            Self::Nominal(value) => Some(&value.canonical),
            Self::FieldlessEnum(value) => Some(&value.canonical),
            Self::ValueEnum(value) | Self::ValueEnumWithLayout(value, _) => Some(&value.canonical),
            Self::Enum(value) => Some(&value.canonical),
            Self::Trait(value) => Some(value),
            Self::NominalWithTraits(value, _) => Some(&value.canonical),
            Self::Constant(value) => Some(&value.canonical),
            Self::TypeAlias(value) => Some(&value.canonical),
            Self::Static(value) => Some(&value.canonical),
        }
    }
}

/// Native records carry exact Rust identities and closed signatures, never another source declaration's executable
/// authority.
fn valid_native_calls(callables: &[crate::body_ir::LoafCallable]) -> bool {
    callables.iter().all(|callable| {
        matches!(callable.canonical.origin, crate::SymbolOrigin::RustCrate(_))
            && crate::body_ir::rust_item_identity(&callable.item).as_ref() == Some(&callable.canonical)
            && callable.type_parameters.is_empty()
            && callable.parameter_defaults.is_empty()
            && callable.declaration_frame.is_none()
    })
}

/// Explicit admission and payload address for one public root or internal execution dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeclarationCoverage {
    /// Covered content and its complete direct requirements; consumers resolve the authorized transitive closure.
    Covered {
        /// Offset relative to the first payload byte.
        offset: u64,
        /// Exact encoded fragment length.
        length: u64,
        /// Canonical declarations whose executable content or type context this fragment requires.
        requirements: Vec<CanonicalSymbolId>,
    },
    /// Deliberate absence of a fragment, distinct from a covered empty function.
    Uncovered(CoverageReason),
    /// A public member executes through its separately addressed declaring type context.
    TypeContext {
        /// Canonical checked type declaration that owns this member.
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
    /// Declared coverage for exactly the finalized public identities owned by this package.
    pub declarations: BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
    /// Reachable implementation dependencies; these identities are never public resolution roots.
    pub implementation: BTreeMap<CanonicalSymbolId, DeclarationCoverage>,
}

/// One declaration that survived projection, with the canonical identities its execution needs.
///
/// Kept as a named pair rather than a tuple because both halves are read in three later phases, and
/// `(declaration, requirements)` does not say which set the identities belong to at the point of use.
struct AdmittedDeclaration {
    /// The projected declaration, in the form a consumer executes.
    declaration: ExecutableDeclaration,
    /// Canonical identities this declaration's execution depends on, including private implementation dependencies.
    requirements: BTreeSet<CanonicalSymbolId>,
}

/// In-memory reader for already-loaded artifact bytes. File consumers use the same index with bounded range reads.
#[derive(Debug, Clone)]
pub struct SurfaceReader<'bytes> {
    index: SurfaceIndex,
    payload: &'bytes [u8],
}

#[cfg(test)]
mod tests;
