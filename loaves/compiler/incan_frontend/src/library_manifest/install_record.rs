//! What installing a compiled Loaf reads of it (DD-0015 §9, step 2), as one compact record of the Loaf.
//!
//! A consumer's checker installs a compiled Loaf a dependency interface names. Installing reads the Loaf's nominal
//! public types with their declarations' identities, its root exports by kind and the foreign type origins its
//! signatures carry; every layout is built later, by name. The library compile writes this record beside the Loaf's
//! declaration records so that installing reads it instead of decoding the whole manifest. The record is in the
//! manifest's own published forms: names, paths, canonical identities and type origins.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    AliasExport, CanonicalIdentityExport, LibraryIdentityGraph, NominalTypeOriginExport, TypeParamExport, TypeRef,
};

/// Schema of the install record.
pub const INSTALL_RECORD_SCHEMA: u32 = 1;

/// What installing one compiled Loaf reads of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LoafInstallRecord {
    pub schema: u32,
    /// Each unique top-level public name of the Loaf that names a nominal type, in identity-graph order: the public
    /// name, the source path of the declaration it names and that declaration's canonical identity. A consumer binds
    /// each under its own canonical spelling and provider.
    pub canonical_types: Vec<InstallCanonicalType>,
    /// The Loaf's root exports by kind, in manifest order.
    pub root_exports: InstallRootExports,
    /// The foreign nominal type origins the Loaf's published signatures carry, in binding-key order.
    pub type_origins: Vec<NominalTypeOriginExport>,
    /// Each checked API module with its model, class, enum, newtype, type alias and trait declarations, in module
    /// and declaration order. A consumer registers them under its declaration keys and replays their lexical scopes.
    pub modules: Vec<InstallModule>,
    /// The Loaf's identity graph, as its manifest publishes it. Install lays out each nominal export under its
    /// canonical spelling, and the foreign preparation of a Loaf that reaches this one reads it.
    pub identity_graph: LibraryIdentityGraph,
    /// For each public name the identity graph publishes, in first-occurrence order, the kind of root export the
    /// manifest gives that name first, in the order install searches the root exports.
    pub export_kinds: Vec<InstallNamedExport>,
    /// The Loaf's root alias exports, which install resolves when it lays them out.
    pub aliases: Vec<AliasExport>,
    /// Nested identity-graph sources that describe a type layout or a type alias.
    pub nested_layouts: Vec<InstallNestedLayout>,
}

impl LoafInstallRecord {
    /// The kind of root export each public name the identity graph publishes finds first, by name.
    pub fn export_kinds_by_name(&self) -> std::collections::HashMap<&str, Option<InstallExportKind>> {
        self.export_kinds
            .iter()
            .map(|named| (named.public_name.as_str(), named.kind))
            .collect()
    }
}

/// Whether a root export of this kind is a model, class, enum or newtype.
pub fn is_nominal_export(kind: Option<InstallExportKind>) -> bool {
    matches!(
        kind,
        Some(
            InstallExportKind::Model | InstallExportKind::Class | InstallExportKind::Enum | InstallExportKind::Newtype
        )
    )
}

/// The kind of root export a public name finds first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallNamedExport {
    pub public_name: String,
    /// `None` when no root export has the name.
    pub kind: Option<InstallExportKind>,
}

/// The kinds of a manifest's root exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallExportKind {
    Alias,
    Model,
    Class,
    Function,
    Partial,
    Trait,
    Enum,
    EnumVariant,
    TypeAlias,
    Newtype,
    Const,
    Static,
}

/// One checked API module of a compiled Loaf and the nominal declarations install registers from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstallModule {
    pub module_path: Vec<String>,
    pub declarations: Vec<InstallDeclaration>,
    /// Spellings in this module that resolve through a declaration rather than a root export.
    pub type_scope: BTreeMap<String, InstallScopeTarget>,
}

/// A declaration reached by a spelling in a checked module's type scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallScopeTarget {
    /// The declaring module's authored path within the package.
    pub module: Vec<String>,
    /// The declaration's name in its module.
    pub name: String,
}

/// The layout recipe available at one nested identity export's source path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstallNestedLayout {
    /// The checked declaration path named by the identity graph.
    pub source_path: Vec<String>,
    /// How this declaration yields a type layout.
    pub kind: InstallNestedLayoutKind,
}

/// How a nested export describes a type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum InstallNestedLayoutKind {
    /// A model, class, enum or newtype whose fields and methods are read on demand.
    Nominal,
    /// A checked type alias.
    TypeAlias,
    /// A forwarded type whose projection identifies the type it names.
    Forwarded(TypeRef),
}

/// One model, class, enum, newtype, type alias or trait of a checked API module.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstallDeclaration {
    /// Its position among the module's checked declarations, where its layout is built from.
    pub position: usize,
    pub name: String,
    pub kind: InstallDeclarationKind,
    /// The canonical identity the Loaf's identity graph publishes at the declaration's public path.
    pub canonical: Option<CanonicalIdentityExport>,
    /// A type alias's target.
    pub alias_target: Option<TypeRef>,
    /// The type parameters of a type alias, model or class.
    pub type_params: Vec<TypeParamExport>,
}

/// The kinds of declaration install registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallDeclarationKind {
    Model,
    Class,
    Enum,
    Newtype,
    TypeAlias,
    Trait,
}

/// One nominal public type of a compiled Loaf.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallCanonicalType {
    pub public_name: String,
    pub source_path: Vec<String>,
    pub canonical: Option<CanonicalIdentityExport>,
}

/// The names of a compiled Loaf's root exports by kind, in manifest order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRootExports {
    pub models: Vec<String>,
    pub classes: Vec<String>,
    pub enums: Vec<String>,
    pub newtypes: Vec<String>,
    pub traits: Vec<String>,
}
