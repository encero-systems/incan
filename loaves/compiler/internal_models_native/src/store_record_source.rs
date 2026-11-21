//! Store-backed compiler record reads over one pinned, allowed-root snapshot.

use crate::hex_text::hexlify;
use crate::overload_members::{OVERLOAD_MEMBERS, read_method_members};
use crate::record_scope::{
    AbsentFact, DeletedFact, MemberFact, MemberSelection, ReadFault, ReadScope, RecordKey, RecordRead, RecordRequest,
    RecordSelection, RecordSource, RevisionEvidence, SelectedFact,
};
use crate::source_value_records::{SOURCE_PROFILE, source_value};
use internal_models_bindings::{BindingLookup, Snapshot, lookup_binding, lookup_binding_metadata, lookup_members};
use sha2::{Digest, Sha256};

/// Encode the full compiler identity once for store keys and in-memory indexes, refusing delimiters.
pub fn encode_record_key(key: &RecordKey) -> Result<String, String> {
    let fields = [
        &key.family,
        &key.identity_profile,
        &key.origin,
        &key.identity,
        &key.facet,
        &key.context,
    ];
    if fields.iter().any(|field| field.contains(['\t', '\n', '\r'])) {
        return Err("compiler record key fields cannot contain store delimiters".to_owned());
    }
    Ok(fields.map(String::as_str).join("\t"))
}

/// Whether the exact canonical key is inside one staged family or subfamily root.
fn allowed(roots: &[String], key: &str) -> bool {
    roots.iter().any(|root| {
        key == root
            || key
                .strip_prefix(root.as_str())
                .is_some_and(|tail| tail.starts_with('\t'))
    })
}

/// Select a complete field answer or one exact bounded payload range without interpreting family schemas.
pub fn selected_record_bytes(value: &[u8], request: &RecordRequest) -> Option<Vec<u8>> {
    match &request.selection {
        RecordSelection::Field(_) => Some(value.to_vec()),
        RecordSelection::Payload(payload) => {
            let start = usize::try_from(payload.offset).ok()?;
            let length = usize::try_from(payload.length).ok()?;
            value.get(start..start.checked_add(length)?).map(<[u8]>::to_vec)
        }
        RecordSelection::Members(_) => None,
    }
}

/// Bind selected bytes to the complete selector spelling retained independently in the request evidence.
fn selection_name(request: &RecordRequest) -> String {
    match &request.selection {
        RecordSelection::Field(name) => format!("field\t{name}"),
        RecordSelection::Payload(p) => format!("payload\t{}\t{}\t{}", p.reference, p.offset, p.length),
        RecordSelection::Members(m) => {
            format!(
                "members\t{}\t{}\t{}\t{}\t{}",
                m.selector, m.order, m.lower, m.upper, m.continuation
            )
        }
    }
}

/// Hash the newline-joined parts followed by the lowercase hex digest of the selected bytes.
fn selection_digest(parts: &[&str], value: &[u8]) -> String {
    let mut preimage = String::new();
    for part in parts {
        preimage.push_str(part);
        preimage.push('\n');
    }
    preimage.push_str(&hexlify(&Sha256::digest(value)));
    hexlify(&Sha256::digest(preimage.as_bytes()))
}

/// Authenticate compiler reads against one exact store view and its staged family-root envelope.
pub struct StoreRecordSource {
    snapshot: Snapshot,
    scope: ReadScope,
    allowed_roots: Vec<String>,
}

impl StoreRecordSource {
    /// Retain the leased snapshot, activation identity and allowed record families for one worker activation.
    pub fn open(snapshot: &Snapshot, scope: &ReadScope, allowed_roots: &[String]) -> Self {
        // The returned source retains a lease on the same snapshot after its caller releases this argument.
        Self {
            snapshot: snapshot.clone(),
            scope: scope.clone(),
            allowed_roots: allowed_roots.to_vec(),
        }
    }

    /// Read a canonical bounded member page and retain its semantic selection independently of the store's index
    /// root. The witness hashes only the selected names, bounds and continuation, so an unrelated store write does
    /// not invalidate an unchanged dependency.
    fn members_read(&self, scope: &ReadScope, request: &RecordRequest, key: String, s: &MemberSelection) -> RecordRead {
        if s.order != "canonical" {
            return RecordRead::Failed(ReadFault::SelectionMismatch);
        }
        let prefix = key.chars().count() + 1;
        let Ok(found) = lookup_members(
            &self.snapshot,
            key,
            s.lower.clone(),
            s.upper.clone(),
            s.continuation.clone(),
            request.byte_limit,
        ) else {
            return RecordRead::Failed(ReadFault::DamagedRecord);
        };
        let names: Vec<String> = found
            .members
            .iter()
            .map(|member| member.key.chars().skip(prefix).collect())
            .collect();
        let value = names.join("\n").into_bytes();
        if value.len() as i64 > request.byte_limit {
            return RecordRead::Failed(ReadFault::InvalidEvidence);
        }
        let digest = selection_digest(
            &[&s.selector, &s.order, &s.lower, &s.upper, &found.continuation],
            &value,
        );
        RecordRead::Members(MemberFact {
            scope: scope.clone(),
            request: request.clone(),
            value,
            membership_digest: digest,
            continuation: found.continuation,
        })
    }

    /// Answer a receiver-scoped overload member page from its own authenticated row.
    fn overload_read(&self, scope: &ReadScope, request: &RecordRequest, key: String) -> RecordRead {
        match lookup_binding(&self.snapshot, key) {
            Ok(BindingLookup::Present(present)) => read_method_members(scope, request, &present.value),
            Ok(BindingLookup::Absent(absent)) => RecordRead::Absent(AbsentFact {
                scope: scope.clone(),
                request: request.clone(),
                exclusion_digest: absent.exclusion_digest,
            }),
            Ok(BindingLookup::Deleted(deleted)) => RecordRead::Deleted(DeletedFact {
                scope: scope.clone(),
                request: request.clone(),
                revision: deleted.revision,
            }),
            Err(_) => RecordRead::Failed(ReadFault::DamagedRecord),
        }
    }

    /// Read exactly one field, payload range or bounded member page and return its authenticated evidence.
    pub fn read(&self, scope: &ReadScope, request: &RecordRequest) -> RecordRead {
        if scope.snapshot != self.scope.snapshot {
            return RecordRead::Failed(ReadFault::SnapshotMismatch);
        }
        if scope.profile != self.scope.profile {
            return RecordRead::Failed(ReadFault::ProfileMismatch);
        }
        let Ok(key) = encode_record_key(&request.key) else {
            return RecordRead::Failed(ReadFault::InvalidEvidence);
        };
        if !allowed(&self.allowed_roots, &key) {
            return RecordRead::Failed(ReadFault::OutsidePreparation);
        }
        let mut digest_only = false;
        match &request.selection {
            RecordSelection::Payload(payload) => {
                // Source projections must still decode their logical value to compute the semantic digest.
                digest_only =
                    request.key.identity_profile != SOURCE_PROFILE && payload.offset == 0 && payload.length == 0;
            }
            RecordSelection::Members(selection) => {
                if request.key.family == OVERLOAD_MEMBERS {
                    return self.overload_read(scope, request, key);
                }
                return self.members_read(scope, request, key, selection);
            }
            RecordSelection::Field(_) => {}
        }
        let selected = if digest_only {
            lookup_binding_metadata(&self.snapshot, key)
        } else {
            lookup_binding(&self.snapshot, key)
        };
        // Returned evidence outlives the borrowed request and scope, including tombstone and exclusion results.
        match selected {
            Err(_) => RecordRead::Failed(ReadFault::DamagedRecord),
            Ok(BindingLookup::Deleted(deleted)) => RecordRead::Deleted(DeletedFact {
                scope: scope.clone(),
                request: request.clone(),
                revision: deleted.revision,
            }),
            Ok(BindingLookup::Absent(absent)) => RecordRead::Absent(AbsentFact {
                scope: scope.clone(),
                request: request.clone(),
                exclusion_digest: absent.exclusion_digest,
            }),
            Ok(BindingLookup::Present(present)) => {
                let Ok(logical) = source_value(&request.key, &present.value) else {
                    return RecordRead::Failed(ReadFault::DamagedRecord);
                };
                let Some(value) = selected_record_bytes(&logical, request) else {
                    return RecordRead::Failed(ReadFault::InvalidEvidence);
                };
                let digest = selection_digest(&[&present.selection_digest, &selection_name(request)], &value);
                let semantic = if request.key.identity_profile == SOURCE_PROFILE {
                    hexlify(&Sha256::digest(&logical))
                } else {
                    present.semantic_digest
                };
                RecordRead::Found(SelectedFact {
                    evidence: RevisionEvidence {
                        scope: scope.clone(),
                        request: request.clone(),
                        revision: present.revision,
                        semantic_digest: semantic,
                    },
                    value,
                    selection_digest: digest,
                })
            }
        }
    }
}

impl RecordSource for StoreRecordSource {
    /// Use the same authenticated read when the caller abstracts over record sources.
    fn read(&self, scope: &ReadScope, request: &RecordRequest) -> RecordRead {
        Self::read(self, scope, request)
    }
}
