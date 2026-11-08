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
    /// A canonical source trait represented by a type token, never a standalone runtime type.
    ///
    /// Appended so every earlier serialized discriminant remains unchanged.
    Trait {
        identity: incan_semantics_core::CanonicalSymbolId,
    },
}

/// Exported field metadata for models and classes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldExport {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<CanonicalIdentityExport>,
    /// Lowercase hexadecimal stored-record identity derived from the earliest field name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub record_identity: String,
    /// Whether the field participates in the canonical stored hash.
    #[serde(
        default = "default_record_canonical",
        skip_serializing_if = "record_canonical_is_default"
    )]
    pub record_canonical: bool,
    /// Earlier declared names in chronological order, earliest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renamed_from: Vec<String>,
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
    #[serde(default, skip_serializing_if = "MemberVisibilityExport::is_public")]
    pub visibility: MemberVisibilityExport,
    /// Whether the field has a declared default value.
    pub has_default: bool,
    /// Materializable source default used when a consumer constructs this type through the artifact boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ParamDefaultExport>,
    /// Optional field alias published by the library surface.
    pub alias: Option<String>,
    /// Optional human-readable field description.
    pub description: Option<String>,
    /// Migration facts used by the ordinary private-member refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_private: Option<MadePrivateExport>,
}

/// Manifest-safe lifecycle facts for a declaration or member that used to be public.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MadePrivateExport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retire_in: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

impl From<&crate::symbols::MadePrivateInfo> for MadePrivateExport {
    fn from(value: &crate::symbols::MadePrivateInfo) -> Self {
        Self {
            since: value.since.clone(),
            retire_in: value.retire_in.clone(),
            message: value.message.clone(),
            by: value.by.clone(),
            issue: value.issue.clone(),
            replacement: value.replacement.clone(),
        }
    }
}

impl From<&MadePrivateExport> for crate::symbols::MadePrivateInfo {
    fn from(value: &MadePrivateExport) -> Self {
        Self {
            since: value.since.clone(),
            retire_in: value.retire_in.clone(),
            message: value.message.clone(),
            by: value.by.clone(),
            issue: value.issue.clone(),
            replacement: value.replacement.clone(),
        }
    }
}

/// Supply the RFC 085 default for manifests written before the canonical field flag existed.
fn default_record_canonical() -> bool {
    true
}

/// Omit the default canonical-field value from the additive manifest representation.
fn record_canonical_is_default(value: &bool) -> bool {
    *value
}

impl FieldExport {
    /// Derive the stored-record identity required by this field's retained rename history.
    pub(crate) fn expected_record_identity(&self) -> Vec<u8> {
        let earliest_name = self.renamed_from.first().map_or(self.name.as_str(), String::as_str);
        incan_semantics_core::record_identity::record_member_identity(earliest_name).to_vec()
    }

    /// Decode the validated hexadecimal identity, deriving the legacy default when the field omitted it.
    pub(crate) fn hydrated_record_identity(&self) -> Vec<u8> {
        if self.record_identity.is_empty() {
            return self.expected_record_identity();
        }
        match hex::decode(&self.record_identity) {
            Ok(identity) => identity,
            Err(_) => self.expected_record_identity(),
        }
    }
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

/// Source-level visibility of a field or method published through a library manifest.
///
/// Private visibility is valid for model and class fields and for methods (#2570). Legacy manifests that omit this
/// value retain public visibility through the default, as every member was public to their consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberVisibilityExport {
    /// Access is limited to the declaring type's own methods.
    Private,
    /// Access is available to source and compiled-library consumers.
    #[default]
    Public,
}

impl MemberVisibilityExport {
    /// Return whether this member retains the legacy public consumer behavior.
    pub(crate) fn is_public(&self) -> bool {
        matches!(self, Self::Public)
    }
}

impl From<crate::ast::Visibility> for MemberVisibilityExport {
    fn from(visibility: crate::ast::Visibility) -> Self {
        match visibility {
            crate::ast::Visibility::Private => Self::Private,
            crate::ast::Visibility::Public => Self::Public,
        }
    }
}

impl From<MemberVisibilityExport> for crate::ast::Visibility {
    fn from(visibility: MemberVisibilityExport) -> Self {
        match visibility {
            MemberVisibilityExport::Private => Self::Private,
            MemberVisibilityExport::Public => Self::Public,
        }
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
