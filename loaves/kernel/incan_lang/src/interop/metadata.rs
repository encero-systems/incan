//! Incan-native view of Rust items extracted from a Cargo workspace (RFC 041).
//!
//! These types are intentionally free of rust-analyzer or compiler-internal IDs so the typechecker and lowering stages
//! can consume stable, snapshot-friendly metadata.

use serde::{Deserialize, Serialize};

use crate::lang::types::collections::{self, CollectionTypeId};

mod ancestral_paths;
mod signature_rules;
mod source_bounds;
mod type_shapes;

pub use ancestral_paths::*;
pub use signature_rules::*;
pub use source_bounds::*;
pub use type_shapes::*;

/// Return whether a Rust display names the canonical owned `String` type.
///
/// Rust metadata can expose `String` with implementation-only allocator arguments. Classify only the three canonical
/// paths, while deliberately ignoring the generic arguments so `String<alloc::alloc::Global>` retains string
/// semantics without accepting unrelated types that merely end in `String`.
#[must_use]
pub fn rust_display_is_owned_string(display: &str) -> bool {
    let normalized = display.trim().replace(' ', "");
    let base = normalized.split('<').next().unwrap_or(normalized.as_str());
    matches!(base, "String" | "std::string::String" | "alloc::string::String")
}

/// Whether an item is visible across crate boundaries for ordinary `pub` Rust APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RustVisibility {
    /// `pub` — visible outside the defining crate (subject to future path-specific rules).
    Public,
    /// Anything else (`pub(crate)`, `pub(super)`, private, etc.).
    Restricted,
}

/// Top-level classification for a resolved Rust path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RustItemKind {
    /// A Rust module (namespace of nested items).
    Module(RustModuleInfo),
    /// A struct, enum, union, or builtin type surface (methods + associates).
    Type(RustTypeInfo),
    /// A free function, associated function, or method item viewed as callable.
    Function(RustFunctionSig),
    /// A `const` or `static` item.
    Constant {
        /// Pretty-printed Rust type string from the analyzer.
        type_display: String,
    },
    /// A `trait` definition and its associated items.
    Trait(RustTraitInfo),
    /// An exported procedural macro, with any independently observed derive probe implementations.
    Macro(RustMacroInfo),
    /// Placeholder for Rust items not yet represented by a narrower metadata variant.
    Unsupported { description: String },
}

/// Metadata for one resolved Rust item (type, fn, module, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustItemMetadata {
    /// Canonical path as Incan already models it, e.g. `std::collections::HashMap`.
    pub canonical_path: String,
    /// Underlying Rust definition path after resolving re-exports, when rust-analyzer can provide one.
    #[serde(default)]
    pub definition_path: Option<String>,
    pub visibility: RustVisibility,
    pub kind: RustItemKind,
}

/// Rust std/alloc collection families whose lookup methods rely on Rust borrow semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RustCollectionFamily {
    /// Hash-map families keyed by borrowed lookup probes (`get`, `contains_key`).
    HashMap,
    /// Ordered map families keyed by borrowed lookup probes (`get`, `contains_key`).
    BTreeMap,
    /// Hash-set families queried by borrowed element probes (`contains`).
    HashSet,
    /// Ordered set families queried by borrowed element probes (`contains`).
    BTreeSet,
}

impl RustCollectionFamily {
    /// The standard native owner of a source collection projection. Borrowed Dict and Set values use the same
    /// metadata-selected method contracts as their owned Rust counterparts.
    #[must_use]
    pub const fn canonical_path(self) -> &'static str {
        match self {
            Self::HashMap => "std::collections::HashMap",
            Self::BTreeMap => "std::collections::BTreeMap",
            Self::HashSet => "std::collections::HashSet",
            Self::BTreeSet => "std::collections::BTreeSet",
        }
    }

    /// Classify a canonical Rust path into a supported collection family.
    #[must_use]
    pub fn for_canonical_path(path: &str) -> Option<Self> {
        let path = path.split('<').next().unwrap_or(path);
        match path {
            "std::collections::HashMap"
            | "std::collections::hash_map::HashMap"
            | "hashbrown::HashMap"
            | "hashbrown::map::HashMap" => Some(Self::HashMap),
            "std::collections::BTreeMap" | "alloc::collections::btree_map::BTreeMap" => Some(Self::BTreeMap),
            "std::collections::HashSet"
            | "std::collections::hash_set::HashSet"
            | "hashbrown::HashSet"
            | "hashbrown::set::HashSet" => Some(Self::HashSet),
            "std::collections::BTreeSet" | "alloc::collections::btree_set::BTreeSet" => Some(Self::BTreeSet),
            _ => None,
        }
    }

    /// Classify an Incan or imported collection type name into a supported collection family.
    #[must_use]
    pub fn for_type_name(name: &str) -> Option<Self> {
        match collections::from_str(name) {
            Some(CollectionTypeId::Dict) => return Some(Self::HashMap),
            Some(CollectionTypeId::Set) => return Some(Self::HashSet),
            _ => {}
        }
        match name {
            "BTreeMap" => Some(Self::BTreeMap),
            "BTreeSet" => Some(Self::BTreeSet),
            _ => None,
        }
    }

    /// Whether `method` is a borrow-sensitive lookup on this collection family.
    #[must_use]
    pub fn preserves_lookup_arg_shape(self, method: &str) -> bool {
        match self {
            Self::HashMap | Self::BTreeMap => matches!(method, "get" | "contains_key"),
            Self::HashSet | Self::BTreeSet => method == "contains",
        }
    }
}

impl RustItemMetadata {
    /// Classify this metadata item as a supported std/alloc collection family when applicable.
    #[must_use]
    pub fn collection_family(&self) -> Option<RustCollectionFamily> {
        RustCollectionFamily::for_canonical_path(&self.canonical_path)
    }
}

/// A single parameter in a Rust function signature (display strings only for Phase 1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustParam {
    /// Parameter name when rust-analyzer can recover it from the HIR body.
    pub name: Option<String>,
    /// Pretty-printed type suitable for diagnostics and future coercion work.
    pub type_display: String,
}

/// Callable signature extracted from rust-analyzer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustFunctionSig {
    /// Receiver and returned-borrow relationship proven from the original Rust signature, before display elision.
    /// Missing facts (including older caches) must not authorize inferred borrowing.
    #[serde(default)]
    pub receiver_contract: Option<RustReceiverContract>,
    /// Method or function type parameters in source declaration order.
    ///
    /// Rust permits type arguments on a callable independently from generics on its receiver type. Retaining this
    /// declaration-level arity lets Incan reject incomplete explicit method turbofishes before the call reaches
    /// rustc.
    #[serde(default)]
    pub type_params: Vec<String>,
    /// Declared trait predicates, including callable inputs and associated output equalities.
    #[serde(default)]
    pub bounds: Vec<RustTraitBound>,
    pub params: Vec<RustParam>,
    pub return_type: String,
    pub is_async: bool,
    pub is_unsafe: bool,
}

/// Source-proven Rust receiver facts used by conservative Incan alias planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustReceiverContract {
    /// The callable has an ordinary shared `&self` receiver, with no ownership-consuming or mutable receiver.
    pub shared: bool,
    /// Every explicit reference in the output is shared and uses only the receiver's lifetime, explicitly or by
    /// elision. Consumers must also validate the returned container shape before treating matched payloads as
    /// descendants.
    pub returns_receiver_borrow: bool,
}

/// An inherent or trait method surfaced on a type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustMethodSig {
    pub name: String,
    pub signature: RustFunctionSig,
    /// The enclosing impl's type parameters in native argument order, before the method's own parameters.
    #[serde(default)]
    pub impl_type_params: Vec<String>,
    /// The impl's self type with its declared parameters, used to infer arguments from a checked receiver.
    #[serde(default)]
    pub impl_self_type: Option<String>,
}

/// One trait implementation extracted Rust metadata associates with a concrete type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustImplementedTrait {
    /// Canonical Rust trait path, for example `std::fmt::Display`.
    pub path: String,
    /// Whether the implementation is proven for `&mut Self` instead of `Self`.
    #[serde(default)]
    pub mutable_reference: bool,
    /// Impl self template; conditional implementations are candidates, not unconditional proof.
    pub self_type: String,
    /// Native impl type binders, before substitution into the trait reference.
    pub type_params: Vec<String>,
    /// Trait arguments after Self, in native declaration order.
    pub type_arguments: Vec<String>,
    /// Conditions that must hold before this implementation applies.
    pub bounds: Vec<RustTraitBound>,
    /// Associated type definitions instantiated with the impl's identity arguments.
    pub associated_type_bindings: Vec<RustAssociatedTypeBinding>,
}

/// A Rust trait predicate with its exact Self, arguments, and associated type equalities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustTraitBound {
    /// The type constrained by this predicate, usually a declared generic parameter.
    pub self_type: String,
    /// Canonical trait identity from rustc metadata.
    pub path: String,
    /// Type arguments after Self; lifetimes carry no layout.
    pub type_arguments: Vec<String>,
    /// Projection equalities, including FnOnce::Output for closure expressions.
    pub associated_type_bindings: Vec<RustAssociatedTypeBinding>,
}

impl RustImplementedTrait {
    /// Whether this record proves a closed implementation without solving generic predicates.
    pub fn is_unconditional(&self) -> bool {
        self.type_params.is_empty() && self.bounds.is_empty()
    }
}

/// One concrete associated-type value attached to an inspected trait implementation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RustAssociatedTypeBinding {
    /// Associated type name declared by the trait.
    pub name: String,
    /// Canonical Rust path of the concrete assigned type.
    pub value_path: String,
}

/// Trait implementation observed while expanding one derive macro on a compiler-owned probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustExpandedDeriveTrait {
    /// Canonical Rust trait path implemented by the expansion.
    pub path: String,
    /// Concrete associated-type values declared by the generated implementation.
    #[serde(default)]
    pub associated_type_bindings: Vec<RustAssociatedTypeBinding>,
}

/// The invocation form of an exported procedural macro, which selects where a source declaration may name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RustMacroForm {
    /// `#[derive(Name)]` on a type declaration.
    Derive,
    /// `#[name(...)]` on an item declaration.
    Attribute,
}

/// An exported procedural macro and, for a derive, optional semantic output observed on a compiler-owned probe type.
/// An empty output list grants passthrough to rustc only, never an assumed implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustMacroInfo {
    /// The invocation forms this export supports. Facts written before attribute macros were inspected record only
    /// derives, so an absent field means a derive.
    #[serde(default = "RustMacroInfo::derive_forms")]
    pub forms: Vec<RustMacroForm>,
    /// Trait implementations observed from the macro expansion on the compiler-owned probe.
    ///
    /// A derive macro may emit more than one implementation, so this remains a collection rather than assuming the
    /// macro shares a name or canonical path with a single trait. The native build remains authoritative for
    /// any input-sensitive behavior or predicates attached to those implementations on the real Incan-authored type.
    #[serde(default)]
    pub expanded_traits: Vec<RustExpandedDeriveTrait>,
}

impl RustMacroInfo {
    /// The forms of a derive-only export, the default for facts that predate attribute macros.
    fn derive_forms() -> Vec<RustMacroForm> {
        vec![RustMacroForm::Derive]
    }

    /// An export of exactly the given forms, with no observed expansion.
    pub fn with_forms(forms: Vec<RustMacroForm>) -> Self {
        Self {
            forms,
            expanded_traits: Vec::new(),
        }
    }

    /// Whether the export can appear in `#[derive(...)]`.
    pub fn is_derive(&self) -> bool {
        self.forms.contains(&RustMacroForm::Derive)
    }

    /// Whether the export can appear as `#[name(...)]` on an item.
    pub fn is_attribute(&self) -> bool {
        self.forms.contains(&RustMacroForm::Attribute)
    }
}

/// One associated-type equality required by a structural mutable-reference implementation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RustAssociatedTypeRequirement {
    /// Canonical path of the trait carrying the associated type.
    pub trait_path: String,
    /// Associated type name constrained by the implementation.
    pub name: String,
    /// Canonical Rust path required for the associated type value.
    pub value_path: String,
}

/// A public field surfaced on a Rust struct/union-like type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustFieldInfo {
    /// Source-facing Rust field name accepted by Incan, with raw identifier prefixes removed.
    ///
    /// A Rust field declared as `r#type` is surfaced as `type`; an ordinary Rust field declared as `type_` remains
    /// `type_`. Codegen rawifies keyword names when emitting Rust.
    pub name: String,
    /// Pretty-printed type for diagnostics and debug output.
    pub type_display: String,
    /// Semantic type shape used by the typechecker for field access and pattern payload binding.
    pub type_shape: RustTypeShape,
}

/// How a Rust enum variant stores one payload field, relative to the semantic type Incan records for it.
///
/// prost writes recursive `oneof` payloads as `Box<T>`. Incan records the payload as `T` (see [`RustVariantInfo`]), so
/// a constructor call passes `T` and a pattern binds `T`; the carrier is what lowering must reintroduce when it emits
/// the Rust constructor. Recording it beside the shape keeps that decision in metadata rather than in a display
/// heuristic at the emission site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RustPayloadCarrier {
    /// The Rust field stores the semantic type directly.
    #[default]
    Direct,
    /// The Rust field stores `Box<T>`; lowering wraps a constructor argument in `Box::new`.
    Boxed,
}

/// One enum variant and its payload field types.
///
/// Payload shapes retain source-facing pattern types. Exact native displays preserve storage before normalization,
/// and `field_carriers` records wrappers omitted from the shapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustVariantInfo {
    /// Variant name as it appears in Rust.
    pub name: String,
    /// Payload field shapes in declaration order, for both tuple and struct variants.
    pub fields: Vec<RustTypeShape>,
    /// Exact native payload templates, before source shape normalization. Direct-route patterns retain integer
    /// widths, references and native owner binders from these displays; an absent record grants no payload layout.
    #[serde(default)]
    pub field_type_displays: Vec<String>,
    /// Field names in declaration order for a struct variant; empty for tuple and unit variants. Names select the
    /// same payload slots as `fields`, independently of the order written in a source pattern.
    #[serde(default)]
    pub field_names: Vec<String>,
    /// Storage carrier per payload field, parallel to `fields`. Entries an older cache did not record mean
    /// [`RustPayloadCarrier::Direct`].
    #[serde(default)]
    pub field_carriers: Vec<RustPayloadCarrier>,
}

impl RustVariantInfo {
    /// Return how the Rust variant stores payload field `index`.
    pub fn field_carrier(&self, index: usize) -> RustPayloadCarrier {
        self.field_carriers.get(index).copied().unwrap_or_default()
    }
}

/// One foreign-generic parameter that can be satisfied through a mutable Rust reference.
///
/// Rules are emitted only for unparameterized owner bounds. The Rust compiler remains the authority for the complete
/// foreign obligation; consumers use these rules to preserve the source-level ownership shape before the normal Rust
/// build validates the instantiated generic contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustMutableReferenceTypeParam {
    /// Name of the owner generic parameter, for example `T` in `Foreign<T: ForeignData>`.
    pub type_param: String,
    /// Unparameterized trait paths the supplied argument can satisfy without adding a reference.
    #[serde(default)]
    pub direct_trait_bounds: Vec<String>,
    /// Mutable-reference implementations admitted by the owner's direct trait bounds.
    #[serde(default)]
    pub mutable_reference_candidates: Vec<RustMutableReferenceCandidate>,
    /// Tuple arities for which the direct owner trait composes element-by-element.
    ///
    /// Consumers may recurse into tuple leaves only when the foreign trait itself declares the corresponding
    /// structural implementation. An empty list keeps composite arguments fail-closed.
    #[serde(default)]
    pub tuple_composition_arities: Vec<usize>,
}

/// One structural `&mut T` implementation that may satisfy a foreign generic bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustMutableReferenceCandidate {
    /// Trait paths the inner `T` must satisfy for this `&mut T` implementation to apply.
    #[serde(default)]
    pub required_traits: Vec<String>,
    /// Canonical associated-type equalities represented by this metadata record.
    #[serde(default)]
    pub required_associated_type_bindings: Vec<RustAssociatedTypeRequirement>,
    /// Whether the complete source predicate set is represented by `required_traits` and associated-type bindings.
    ///
    /// False is deliberately fail-closed: a direct consumer must defer to the Rust trait solver instead of reducing
    /// an unsupported generic, const, lifetime, or higher-ranked predicate to a trait-name match.
    #[serde(default)]
    pub fallback_is_complete: bool,
}

/// Method, field, and variant surface for a Rust ADT or builtin type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RustTypeInfo {
    /// Whether this owner declares lifetime parameters. Erasing their labels does not authorize a borrowed source
    /// signature: that boundary requires a retained lifetime contract.
    #[serde(default)]
    pub has_lifetime_params: bool,
    /// Source-declared type parameters in declaration order, with lifetimes elided.
    ///
    /// These belong to the Rust type declaration, not to one method. Associated calls such as
    /// `Fft.new[f32](...)` specialize this receiver surface while method-level generic parameters remain recorded on
    /// [`RustFunctionSig`].
    #[serde(default)]
    pub type_params: Vec<String>,
    /// Declared default type argument for each source type parameter, aligned with [`Self::type_params`].
    ///
    /// The vector is empty when no type parameter has a default. Otherwise `None` preserves a required parameter that
    /// precedes a later default, while `Some` carries the canonical Rust type display used when source omits that
    /// argument.
    #[serde(default)]
    pub type_param_defaults: Vec<Option<String>>,
    /// Source-owned rules for generic parameters that admit a mutable-reference implementation.
    ///
    /// They let the frontend distinguish an owned foreign argument from a mutable-reference alternative without
    /// provider-specific matching. Complete foreign obligation checking remains with rustc.
    #[serde(default)]
    pub mutable_reference_type_params: Vec<RustMutableReferenceTypeParam>,
    /// Exact unconditional trait implementations emitted by successfully expanded Rust derive macros.
    ///
    /// rust-analyzer's ordinary trait index can omit proc-macro-generated implementations. Inspection therefore
    /// resolves the generated `impl` syntax through HIR and records its actual trait identity plus concrete
    /// associated-type values here. Derive macro names and unexpanded source attributes supply no implementation
    /// evidence; conditional generated implementations remain solver-only.
    #[serde(default)]
    pub expanded_derive_traits: Vec<RustExpandedDeriveTrait>,
    /// Whether the declaration contains const parameters that occupy Rust turbofish positions.
    ///
    /// Incan v0.5 can specialize type-only receivers. Retaining this fact makes const-bearing owners fail closed
    /// rather than shifting a later type argument into a const slot.
    #[serde(default)]
    pub has_const_params: bool,
    /// Pretty-printed target type when this item is a Rust `type` alias.
    ///
    /// Ordinary structs, enums, traits, and builtins leave this empty. Alias targets are metadata, not a substitute
    /// type identity: callers should use them only when the alias itself is the expected surface and the target shape
    /// is needed for contextual typing or boundary planning.
    #[serde(default)]
    pub alias_target: Option<String>,
    /// Completeness of the recorded type surface.
    #[serde(default)]
    pub metadata_completeness: RustTypeMetadataCompleteness,
    /// Public inherent methods and associated functions.
    pub methods: Vec<RustMethodSig>,
    /// Trait implementations extracted metadata proves for this Rust type.
    #[serde(default)]
    pub implemented_traits: Vec<RustImplementedTrait>,
    /// Native fields for struct-like types. The completeness record restricts source reads of private slots;
    /// their retained types and order exist only to verify native field projections.
    pub fields: Vec<RustFieldInfo>,
    /// Enum variants when the type is an enum; empty for non-enums.
    pub variants: Vec<RustVariantInfo>,
}

/// Which operations an extracted Rust type surface proves independently of readable fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RustTypeMetadataCompleteness {
    /// Extracted Rust metadata, including methods and proven trait impls when available.
    #[default]
    Complete,
    /// Syntax-only metadata from generated source, limited to public fields and enum variants.
    FieldsAndVariantsOnly,
    /// Complete signatures and readable fields, without authority to construct a struct layout.
    /// Includes cross-crate non-exhaustive structs and items that are not structs.
    CompleteWithoutConstruction,
    /// Complete native layout and signatures with the named fields unavailable to source reads or construction.
    CompleteWithPrivateFields(Vec<String>),
}

impl RustTypeMetadataCompleteness {
    /// Return whether this metadata supplies the complete extracted Rust type surface.
    pub const fn is_complete(&self) -> bool {
        matches!(
            self,
            Self::Complete | Self::CompleteWithoutConstruction | Self::CompleteWithPrivateFields(_)
        )
    }

    /// Return whether this metadata can prove inherent methods.
    pub const fn has_methods(&self) -> bool {
        self.is_complete()
    }

    /// Return whether this metadata can prove trait implementations.
    pub const fn has_trait_impls(&self) -> bool {
        self.is_complete()
    }

    /// Return whether the field list proves an externally constructible native layout.
    pub const fn has_constructible_layout(&self) -> bool {
        matches!(self, Self::Complete | Self::FieldsAndVariantsOnly)
    }

    /// Return whether a retained native field is visible to a source read or assignment.
    pub fn field_is_public(&self, name: &str) -> bool {
        match self {
            Self::CompleteWithPrivateFields(private) => !private.iter().any(|field| field == name),
            _ => true,
        }
    }
}

/// One exported name inside a module (lightweight summary for namespace resolution).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustModuleChild {
    pub name: String,
    pub kind_hint: RustModuleChildKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RustModuleChildKind {
    Module,
    Type,
    Function,
    Constant,
    Trait,
    Other,
}

/// Children visible in a module scope (public items when resolved from outside).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustModuleInfo {
    pub children: Vec<RustModuleChild>,
}

/// Associated items declared on a trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RustTraitAssoc {
    Function { name: String, signature: RustFunctionSig },
    TypeAlias { name: String },
    Constant { name: String, type_display: String },
}

/// A native trait's declared callable surface and implementation templates. Templates belong to the trait's ABI
/// too, so a later extension trait can describe implementations on a type whose own ABI was compiled earlier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustTraitInfo {
    /// Whether rustc's Rust 2024 prelude imports this exact trait identity.
    pub is_in_prelude: bool,
    /// Trait type binders after Self, in native declaration order.
    pub type_params: Vec<String>,
    /// Supertrait predicates and associated output equalities on this trait's Self parameter.
    #[serde(default)]
    pub bounds: Vec<RustTraitBound>,
    /// Associated functions, types, and constants declared by this trait.
    pub items: Vec<RustTraitAssoc>,
    /// Positive implementation templates supplied by the native trait's metadata; predicates remain obligations.
    #[serde(default)]
    pub implementations: Vec<RustImplementedTrait>,
    /// Co-located derive-macro namespace metadata for this same Rust path, when invoked by the project.
    ///
    /// Rust permits a trait and derive macro to share a spelling. Keeping the macro expansion separate prevents an
    /// ordinary trait-namespace lookup from hiding a macro that implements a differently named trait.
    #[serde(default)]
    pub derive_macro: Option<RustMacroInfo>,
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ancestral_path_tests;
