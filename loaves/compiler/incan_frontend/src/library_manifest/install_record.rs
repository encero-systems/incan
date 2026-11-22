//! What installing a compiled Loaf reads of it (DD-0015 §9, step 2), as one compact record of the Loaf.
//!
//! A consumer's checker installs a compiled Loaf a dependency interface names. Installing reads the Loaf's nominal
//! public types with their declarations' identities, its root exports by kind and the foreign type origins its
//! signatures carry; every layout is built later, by name. The library compile writes this record beside the Loaf's
//! declaration records so that installing reads it instead of decoding the whole manifest. The record is in the
//! manifest's own published forms: names, paths, canonical identities and type origins.

use serde::{Deserialize, Serialize};

use super::{CanonicalIdentityExport, NominalTypeOriginExport};

/// Schema of the install record.
pub const INSTALL_RECORD_SCHEMA: u32 = 1;

/// What installing one compiled Loaf reads of it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
