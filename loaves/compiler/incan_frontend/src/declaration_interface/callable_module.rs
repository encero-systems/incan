//! Complete candidate inventories for modules containing only checked free functions.
//!
//! Inert module docstrings are ignored; other declaration kinds and imports refuse capture. A lookup witness applies to
//! this exact admitted namespace, never to a member, builtin or dependency overlay. These records do not replace the
//! checker's lookup recorder or authorize selective body checking; they provide a checked replay boundary for its
//! callable candidate sets.

use std::collections::{BTreeMap, BTreeSet};

use incan_semantics_core::stable_identity::{DeclarationSignature, StableDeclarationId, StableIdentityProjection};
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};
use serde::{Deserialize, Serialize};

use super::{CheckedCallableInterface, encode_record, invalid};
use crate::ast::{Declaration, Program};
use crate::library_exports::CheckedNamedExport;
use crate::library_manifest::LibraryManifestError;
use crate::typechecker::TypeChecker;

/// One accepted source-only callable namespace, retaining private declarations and every overload candidate.
/// This deliberately narrower name prevents treating an admitted free-function table as a general module interface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckedCallableModule {
    schema: u32,
    origin: SymbolOrigin,
    callables: Vec<CallableEntry>,
}

/// Keep the complete contract digest beside the header whose producer established its nominal correspondence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallableEntry {
    header: CheckedCallableInterface,
    signature: String,
}

/// The complete result of a callable-namespace query, including an empty candidate set.
/// A new or removed overload invalidates this observation even when a previously selected callee is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallableLookupWitness {
    origin: SymbolOrigin,
    name: String,
    candidates: Vec<(StableDeclarationId, String)>,
}

/// One producer-owned positive lookup with the checked callable signatures its body digest consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckedCallableRead {
    /// Every candidate, including private declarations and overloads, in canonical order.
    pub(crate) witness: CallableLookupWitness,
    /// Existing canonical roots paired with their accepted stable signature projections.
    pub(crate) signatures: Vec<(CanonicalSymbolId, DeclarationSignature)>,
}

impl CheckedCallableModule {
    /// Capture every callable from an accepted module with no imports, nominal declarations or module directive.
    /// Unsupported declarations refuse the entire namespace rather than making a negative lookup unsound.
    pub fn from_checked(program: &Program, checker: &TypeChecker) -> Result<Self, LibraryManifestError> {
        if program.rust_module_path.is_some() || !checker.errors.is_empty() {
            return Err(invalid("callable module requires an accepted source-only namespace"));
        }
        let mut callables = Vec::new();
        for declaration in &program.declarations {
            if matches!(declaration.node, Declaration::Docstring(_)) {
                continue;
            }
            let Declaration::Function(function) = &declaration.node else {
                return Err(invalid(
                    "callable module does not encode this declaration kind or import",
                ));
            };
            let header = CheckedCallableInterface::from_checked(function, declaration.span, checker)?;
            let signature = header.signature_digest(
                &checker.type_info().declarations.named_type_identities,
                &StableIdentityProjection::default(),
            )?;
            callables.push(CallableEntry { header, signature });
        }
        callables.sort_by(|left, right| left.header.identity.cmp(&right.header.identity));
        let origin = callables
            .first()
            .ok_or_else(|| invalid("empty callable module has no checked producer identity"))?
            .header
            .identity
            .origin
            .clone();
        let module = Self {
            schema: 1,
            origin,
            callables,
        };
        module.validate()?;
        Ok(module)
    }

    /// Record all candidates of this namespace, retaining absence as a real read dependency.
    pub fn lookup(&self, name: &str) -> CallableLookupWitness {
        CallableLookupWitness {
            origin: self.origin.clone(),
            name: name.to_owned(),
            candidates: self
                .callables
                .iter()
                .filter(|entry| entry.header.identity.declaration_name == name)
                .map(|entry| (entry.header.identity.clone(), entry.signature.clone()))
                .collect(),
        }
    }

    /// Index positive reads once for item producers. Signatures come from accepted headers, never from parsed
    /// spellings; consumers retain the complete candidate set rather than guessing a single overload.
    pub(crate) fn read_inventory(&self) -> Result<BTreeMap<String, CheckedCallableRead>, LibraryManifestError> {
        self.validate()?;
        let mut reads = BTreeMap::new();
        for entry in &self.callables {
            let name = entry.header.identity.declaration_name.clone();
            let read = reads.entry(name.clone()).or_insert_with(|| CheckedCallableRead {
                witness: CallableLookupWitness {
                    origin: self.origin.clone(),
                    name,
                    candidates: Vec::new(),
                },
                signatures: Vec::new(),
            });
            let canonical = entry
                .header
                .replay
                .to_checked()?
                .identity
                .canonical
                .ok_or_else(|| invalid("callable read has no accepted canonical root"))?;
            let signature = entry
                .header
                .identity
                .signature
                .clone()
                .ok_or_else(|| invalid("callable read has no accepted signature projection"))?;
            read.witness
                .candidates
                .push((entry.header.identity.clone(), entry.signature.clone()));
            read.signatures.push((canonical, signature));
        }
        Ok(reads)
    }

    /// Validate the complete candidate inventory against a current independently accepted namespace.
    /// Unobserved declarations may change; a query in another module cannot satisfy this witness.
    pub fn validates(&self, witness: &CallableLookupWitness) -> bool {
        self.lookup(&witness.name) == *witness
    }

    /// Replay every retained callable at its current root identity after matching complete contracts. This is not
    /// checker installation: current headers must come from a separately accepted declaration environment.
    pub fn replay_at(&self, current: &Self) -> Result<Vec<CheckedNamedExport>, LibraryManifestError> {
        self.validate()?;
        current.validate()?;
        if self.origin != current.origin || self.callables.len() != current.callables.len() {
            return Err(invalid("callable module replay changed its declaration inventory"));
        }
        self.callables
            .iter()
            .zip(&current.callables)
            .map(|(retained, current)| {
                if retained.signature != current.signature {
                    return Err(invalid("callable module replay changed a checked signature"));
                }
                retained.header.replay_at(&current.header)
            })
            .collect()
    }

    /// Encode a validated namespace with the same authenticated framing as its retained callable headers.
    pub fn to_bytes(&self) -> Result<Vec<u8>, LibraryManifestError> {
        self.validate()?;
        encode_record(&serde_json::to_vec(self).map_err(|error| invalid(&error.to_string()))?)
    }

    /// Decode only an authenticated, complete candidate inventory. A damaged private record is an ordinary miss.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LibraryManifestError> {
        let payload = super::decode_record(bytes)?;
        let module: Self = serde_json::from_slice(payload).map_err(|error| invalid(&error.to_string()))?;
        module.validate()?;
        Ok(module)
    }

    /// Refuse ambiguous identity ownership, missing digests and noncanonical ordering before exposing a namespace.
    fn validate(&self) -> Result<(), LibraryManifestError> {
        if self.schema != 1 || self.callables.is_empty() {
            return Err(invalid("unsupported or empty checked callable module"));
        }
        let mut identities = BTreeSet::new();
        let mut previous = None;
        for entry in &self.callables {
            entry.header.validate()?;
            let identity = entry.header.identity();
            if identity.origin != self.origin
                || !identities.insert(identity)
                || previous.is_some_and(|previous| previous >= identity)
                || !valid_digest(&entry.signature)
            {
                return Err(invalid(
                    "callable module lost its complete ordered declaration inventory",
                ));
            }
            previous = Some(identity);
        }
        Ok(())
    }
}

/// Accept the semantic digest owner's exact representation, never a missing or truncated signature witness.
fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
