//! Type parameter, bound, nominal origin, native union, type reference, field, property and receiver exports.

use super::*;

/// One exported generic type parameter and its bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeParamExport {
    pub name: String,
    pub bounds: Vec<TypeBoundExport>,
}

/// One exported generic bound attached to a type parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeBoundExport {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_path: Option<Vec<String>>,
    pub type_args: Vec<TypeRef>,
    /// Compiler-resolved generic header required by this exact trait implementation.
    ///
    /// These are deliberately separate from the owning type's declared generic bounds: they constrain use of this
    /// implementation, not construction or every other operation on the type. The header can repeat declared owner
    /// bounds alongside backend-inferred requirements. Older manifests omit the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implementation_type_params: Vec<ImplementationTypeParamExport>,
    /// An `Eq` or `Hash` bound the compiler inferred because the callable's body hashes the type parameter (#1758),
    /// rather than one the source declared. A consumer refuses a call for it only when the type argument is known to
    /// lack the derive. Older manifests omit the field.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inferred: bool,
}

/// One implementation-header type parameter published with a checked trait adoption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplementationTypeParamExport {
    pub name: String,
    pub bounds: Vec<ImplementationTraitBoundExport>,
}

/// One exact requirement on an implementation type parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplementationTraitBoundExport {
    pub trait_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_args: Vec<TypeRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub associated_types: Vec<ImplementationAssociatedTypeExport>,
    #[serde(default)]
    pub origin: ImplementationTraitBoundOriginExport,
}

/// One associated-type equality carried by an inferred implementation bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImplementationAssociatedTypeExport {
    pub name: String,
    pub ty: TypeRef,
}

/// Stable origin classification for an inferred implementation bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationTraitBoundOriginExport {
    #[default]
    Standard,
    RustCapability,
    SourceCallable,
}

/// Checked identity of a foreign nominal type referenced by a public API type leaf.
///
/// The artifact selection and canonical declaration authorize semantics. Consumer dependency aliases and generated
/// Rust bridge routes are separate projections. Own-package types omit this record to avoid self-digest cycles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NominalTypeOriginExport {
    /// Exact selected version, digest and active public feature projection of the declaring artifact.
    pub provider: crate::loaf_index::LoafIdentity,
    /// Declaring public type identity retained from that artifact's checked identity graph.
    pub canonical: CanonicalIdentityExport,
}

impl NominalTypeOriginExport {
    /// Return an opaque checker binding key; this is never emitted as a Rust path or resolved by source spelling.
    pub fn binding_key(&self) -> String {
        format!("__incan_nominal::{}::{:?}", self.provider.stable_key(), self.canonical)
    }
}

impl TypeRef {
    /// Return whether this typed position contains an explicit emitted-union carrier at any nesting depth.
    pub fn has_native_union(&self) -> bool {
        super::type_projection::contains_native_union(self)
    }
}

/// Artifact ownership of an emitted anonymous-union wrapper.
///
/// A producer cannot include its own final artifact digest in that artifact. Admission binds `ContainingArtifact`
/// to the selected containing provider; forwarding retains that exact selection as `SelectedArtifact`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeUnionOwnerExport {
    ContainingArtifact,
    SelectedArtifact(crate::loaf_index::LoafIdentity),
}

/// Native representation captured from the producer's final emitted union definition.
///
/// `members` retain producer payload order: their indices are the emitted `V0`, `V1`, ... variants. A consumer may
/// project their nominal leaves to different physical Rust routes, but may neither reorder them nor recompute the
/// wrapper name from those routes. The containing selected artifact authenticates this representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeUnionExport {
    pub owner: NativeUnionOwnerExport,
    pub rust_name: String,
    pub members: Vec<TypeRef>,
    /// Exact callable member signatures that own captured environments, in physical variant order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closure_members: Vec<incan_semantics_core::IncanType>,
    /// Producer-owned callable object aliases, ordered as `closure_members`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closure_items: Vec<String>,
    /// Checked union carrier before native payload ordering, retained by the native Loaf publisher. Nominal leaves
    /// already encode their canonical declaration identities; consumers never derive a carrier from a Rust name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_type: Option<incan_semantics_core::IncanType>,
    /// Producer-local nominal bindings captured from checked declarations, without a self artifact digest.
    ///
    /// Keys are the producer's actual lowered spellings. Admission validates every canonical declaration against
    /// the selected owner's public surface before binding these leaves to that exact artifact.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub local_nominals: BTreeMap<String, CanonicalIdentityExport>,
    /// Checked consumer-only physical projection; never accepted from or written to the manifest wire.
    #[serde(skip)]
    pub checked_projection: Option<Box<NativeUnionProjection>>,
}

/// Physical projection of an admitted union, separate from its immutable producer representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeUnionProjection {
    /// Direct admitted dependency through which this exact native owner was selected.
    pub dependency_root: String,
    pub rust_owner: String,
    pub members: Vec<TypeRef>,
    /// Exact accepted nominal bindings under the Rust spellings selected by the consumer's lowering pass.
    pub nominal_origins: BTreeMap<String, NominalTypeOriginExport>,
}

impl NativeUnionExport {
    /// Remove consumer-only physical projections while preserving the admitted producer's wire representation.
    pub fn for_publication(&self) -> Self {
        let mut native = self.clone();
        native.checked_projection = None;
        super::type_projection::VisitTypeRefs::visit_type_refs(&mut native.members, &mut |ty| {
            if let TypeRef::NativeUnion(nested) = ty {
                nested.checked_projection = None;
            }
        });
        native
    }
}

/// Stable manifest-level type reference used by library exports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeRef {
    /// A named non-generic type such as `User` or `int`.
    Named {
        name: String,
        /// Checked foreign nominal origin. Absence retains the legacy local-name contract.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<NominalTypeOriginExport>,
    },
    /// A generic application such as `List[str]`.
    Applied {
        name: String,
        args: Vec<TypeRef>,
        /// Identity of a foreign generic nominal head, independent of its argument identities.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<NominalTypeOriginExport>,
    },
    /// A function type with positional parameter and return types.
    Function {
        params: Vec<TypeRef>,
        return_type: Box<TypeRef>,
    },
    /// A value-level type token such as `Type[int]`.
    TypeToken { inner: Box<TypeRef> },
    /// A tuple type.
    Tuple { elements: Vec<TypeRef> },
    /// A generic type parameter reference.
    TypeParam { name: String },
    /// The receiver type used in methods/traits.
    SelfType,
    /// A reference type.
    Ref { inner: Box<TypeRef> },
    /// A canonical Rust path imported through `rust::...`.
    RustPath { path: String },
    /// A placeholder used when the manifest intentionally preserves unknown type information.
    Unknown,
    /// An ordinary union with its producer-owned native wrapper and ordered semantic payloads.
    ///
    /// Legacy manifests retain `Applied { name: "Union", .. }`; this explicit carrier is emitted only when the
    /// producer supplied actual native representation evidence. Appending the variant preserves earlier positional
    /// discriminants; readers still need the containing payload's version to accept new representation evidence.
    NativeUnion(NativeUnionExport),
    /// A `mut`-marked parameter of a function type, `mut T` in `(mut T, int) -> R` (#1790).
    ///
    /// It appears only as an element of [`TypeRef::Function`]'s `params` and carries the parameter's type in `inner`.
    /// Appended after every earlier variant so their discriminants are unchanged.
    MutParam { inner: Box<TypeRef> },
    /// Canonical explicit ownership marker. The containing signature must carry matching checked contract metadata.
    Ownership {
        mode: incan_lang::lang::types::ownership::OwnershipMode,
        inner: Box<TypeRef>,
    },
}

/// Exported field metadata for models and classes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldExport {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalIdentityExport>,
    pub ty: TypeRef,
    /// Source-level Incan type spelling used by reflection and documentation.
    ///
    /// Older manifests did not retain this presentation-only value, so consumers fall back to the semantic type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_type_name: Option<String>,
    /// Source-level field visibility enforced for compiled-library consumers.
    ///
    /// Older manifests omitted this field and exposed every recorded field to consumers, so absence remains public for
    /// backward compatibility. New manifests only need to serialize the private case.
    #[serde(default, skip_serializing_if = "FieldVisibilityExport::is_public")]
    pub visibility: FieldVisibilityExport,
    /// Whether the field has a declared default value.
    pub has_default: bool,
    /// Materializable source default used when a consumer constructs this type through the artifact boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ParamDefaultExport>,
    /// Optional field alias published by the library surface.
    pub alias: Option<String>,
    /// Optional human-readable field description.
    pub description: Option<String>,
}

/// Exported public computed-property metadata for models and classes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyExport {
    /// Source-level property name.
    pub name: String,
    /// Canonical member declaration identity, absent only for legacy manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalIdentityExport>,
    /// Property result type available to compiled-library consumers.
    pub return_type: TypeRef,
}

/// Source-level visibility of a model or class field published through a library manifest.
///
/// Private visibility is valid for model and class fields. Legacy manifests that omit this value retain public
/// visibility through the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldVisibilityExport {
    /// Access is limited to the declaring model or class's own methods.
    Private,
    /// Access is available to source and compiled-library consumers.
    #[default]
    Public,
}

impl FieldVisibilityExport {
    /// Return whether this field retains the legacy public consumer behavior.
    fn is_public(&self) -> bool {
        matches!(self, Self::Public)
    }
}

/// Receiver mutability for an exported method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiverExport {
    /// Method takes an immutable receiver.
    Immutable,
    /// Method takes a mutable receiver.
    Mutable,
}
