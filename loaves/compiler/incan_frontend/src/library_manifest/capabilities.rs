//! Profile-wide facts consulted before parsing or resolving a package name (#2946).
//!
//! Declaration payloads, contract metadata, documentation and native ABI payloads stay independently addressed.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::LibraryManifest;

/// The part of one verified Loaf's metadata used to discover global contributors and disambiguate package names.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Capability {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) manifest_digest: String,
    pub(crate) vocabulary: bool,
    pub(crate) provider_operations: bool,
    pub(crate) native_unions: bool,
    pub(crate) rust_paths: BTreeSet<String>,
    pub(crate) rust_abi_cache_declared: bool,
    #[serde(default)]
    pub(crate) rust_abi_cache_digest: Option<String>,
}

/// A compact projection of the ordinary metadata records already emitted by the installed compiler.
#[derive(Default)]
pub struct CapabilityCatalog(BTreeMap<String, Capability>);

impl CapabilityCatalog {
    pub fn insert(&mut self, name: &str, metadata: &[u8]) -> Result<(), String> {
        let member: Capability = LibraryManifest::decode_compiler_record_value(metadata).map_err(|e| e.to_string())?;
        if member.name != name || self.0.insert(name.to_owned(), member).is_some() {
            return Err(format!("invalid or duplicated installed capability for {name}"));
        }
        Ok(())
    }

    pub fn key(&self, profile: &str) -> String {
        key(profile, &self.0.keys().cloned().collect())
    }

    pub fn encode(self, profile: &str) -> Result<Vec<u8>, String> {
        let members: Vec<Fields> = self.0.into_values().map(Fields::from).collect();
        LibraryManifest::encode_compiler_record_value(&("incan.compiler-capabilities/1", profile, members))
            .map_err(|error| error.to_string())
    }

    pub(crate) fn decode(bytes: &[u8], profile: &str) -> Result<Vec<Capability>, String> {
        let (schema, stored, rows): (String, String, Vec<Fields>) =
            LibraryManifest::decode_compiler_record_value(bytes).map_err(|error| error.to_string())?;
        let members: Vec<Capability> = rows.into_iter().map(Capability::from).collect();
        if schema != "incan.compiler-capabilities/1"
            || stored != profile
            || members.is_empty()
            || members.windows(2).any(|pair| pair[0].name >= pair[1].name)
        {
            return Err("invalid installed compiler capabilities profile or members".into());
        }
        Ok(members)
    }
}

pub fn prefix(profile: &str) -> String {
    format!("loaf/{profile}/@compiler/capabilities/")
}

pub fn key(profile: &str, names: &BTreeSet<String>) -> String {
    let scope = names.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
    format!("{}00000000/{:x}", prefix(profile), Sha256::digest(scope.as_bytes()))
}

/// Preserve the ordinary compiler-record encoding while writing each capability's schema once per family.
#[derive(Serialize, Deserialize)]
struct Fields(
    String,
    String,
    String,
    bool,
    bool,
    bool,
    BTreeSet<String>,
    bool,
    Option<String>,
);

impl From<Capability> for Fields {
    fn from(member: Capability) -> Self {
        Self(
            member.name,
            member.version,
            member.manifest_digest,
            member.vocabulary,
            member.provider_operations,
            member.native_unions,
            member.rust_paths,
            member.rust_abi_cache_declared,
            member.rust_abi_cache_digest,
        )
    }
}

impl From<Fields> for Capability {
    fn from(
        Fields(
            name,
            version,
            manifest_digest,
            vocabulary,
            provider_operations,
            native_unions,
            rust_paths,
            rust_abi_cache_declared,
            rust_abi_cache_digest,
        ): Fields,
    ) -> Self {
        Self {
            name,
            version,
            manifest_digest,
            vocabulary,
            provider_operations,
            native_unions,
            rust_paths,
            rust_abi_cache_declared,
            rust_abi_cache_digest,
        }
    }
}
