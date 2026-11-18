//! Exact roster and global parser/checker contribution facts of an installed standard package scope (#2946).
use super::LibraryManifest;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// These global registrations affect units even when their source never imports the owning package.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Contributions {
    pub vocabulary: bool,
    pub provider_operations: bool,
}

/// Values carried by the generation-matched Incan Crumb codec in internal_models.
#[derive(Debug, Default)]
pub struct Contributors {
    pub profile: String,
    pub scope: String,
    pub vocabulary: Vec<String>,
    pub operations: Vec<String>,
}

pub type Encoder = fn(Contributors) -> Result<Vec<u8>, String>;
pub type Decoder = fn(&[u8]) -> Result<Contributors, String>;
static CODEC: OnceLock<(Encoder, Decoder)> = OnceLock::new();
pub fn install_codec(encode: Encoder, decode: Decoder) {
    let _ = CODEC.set((encode, decode));
}

#[derive(Default)]
pub struct CapabilityCatalog(BTreeMap<String, Contributions>);

impl CapabilityCatalog {
    pub fn insert(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let facts = LibraryManifest::decode_compiler_record_value(bytes).map_err(|e| e.to_string())?;
        if self.0.insert(name.to_owned(), facts).is_some() {
            return Err(format!("duplicated installed package {name}"));
        }
        Ok(())
    }

    pub fn encode(self, profile: &str) -> Result<(String, Vec<u8>), String> {
        let names = self.0.keys().cloned().collect();
        let mut facts = Contributors {
            profile: profile.into(),
            scope: scope(&names),
            ..Default::default()
        };
        for (name, row) in self.0 {
            if row.vocabulary {
                facts.vocabulary.push(name.clone());
            }
            if row.provider_operations {
                facts.operations.push(name.clone());
            }
        }
        let (encode, _) = CODEC.get().ok_or("compiler contributors Crumb codec not installed")?;
        Ok((key(profile, &names), encode(facts)?))
    }
}

pub(crate) fn decode(
    bytes: &[u8],
    profile: &str,
    names: &BTreeSet<String>,
) -> Result<BTreeMap<String, Contributions>, String> {
    let (_, decode) = CODEC.get().ok_or("compiler contributors Crumb codec not installed")?;
    let value = decode(bytes)?;
    select(value, profile, names)
}

pub(crate) fn select(
    value: Contributors,
    profile: &str,
    names: &BTreeSet<String>,
) -> Result<BTreeMap<String, Contributions>, String> {
    if value.profile != profile || value.scope != scope(names) {
        return Err("compiler contributor record differs from requested package roster".into());
    }
    let mut admitted: BTreeMap<_, _> = names.iter().map(|n| (n.clone(), Contributions::default())).collect();
    for (vocabulary, members) in [(true, value.vocabulary), (false, value.operations)] {
        for name in members {
            let facts = admitted
                .get_mut(&name)
                .ok_or("compiler contributor outside selected package roster")?;
            if vocabulary {
                facts.vocabulary = true;
            } else {
                facts.provider_operations = true;
            }
        }
    }
    Ok(admitted)
}

fn scope(names: &BTreeSet<String>) -> String {
    let text = names.iter().map(String::as_str).collect::<Vec<_>>().join("\n");
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
pub fn prefix(profile: &str) -> String {
    format!("loaf/{profile}/@compiler/capabilities/")
}
pub fn key(profile: &str, names: &BTreeSet<String>) -> String {
    format!("{}00000000/{}", prefix(profile), scope(names))
}
