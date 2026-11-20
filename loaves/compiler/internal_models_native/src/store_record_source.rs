//! Compiler record keys and selection of bytes from an authenticated store binding.

use crate::record_scope::{RecordKey, RecordRequest, RecordSelection};

/// Encode all logical identity fields in their store order.
pub fn encode_record_key(key: &RecordKey) -> Result<String, String> {
    let fields = [
        key.family.as_str(),
        &key.identity_profile,
        &key.origin,
        &key.identity,
        &key.facet,
        &key.context,
    ];
    if fields.iter().any(|s| s.contains(['\t', '\n', '\r'])) {
        return Err("compiler record key fields cannot contain store delimiters".to_owned());
    }
    Ok(fields.join("\t"))
}

/// Select a field or payload range and return bytes owned independently of the snapshot.
pub fn selected_record_bytes(bytes: &[u8], request: &RecordRequest) -> Option<Vec<u8>> {
    match &request.selection {
        RecordSelection::Field(_) => Some(bytes.to_vec()),
        RecordSelection::Payload(p) => {
            let start = usize::try_from(p.offset).ok()?;
            let length = usize::try_from(p.length).ok()?;
            bytes.get(start..start.checked_add(length)?).map(<[u8]>::to_vec)
        }
        RecordSelection::Members(_) => None,
    }
}

use crate::record_scope::{
    AbsentFact, DeletedFact, MemberFact, MemberSelection, ReadFault, ReadScope, RecordRead, RecordSource,
    RevisionEvidence, SelectedFact,
};
use internal_models_bindings::{BindingLookup, Snapshot, lookup_binding, lookup_members};
use sha2::{Digest, Sha256};

/// An authenticated store view plus the exact staged roots admitted to one activation.
pub struct StoreRecordSource {
    snapshot: Snapshot,
    scope: ReadScope,
    allowed_roots: Vec<String>,
}

impl StoreRecordSource {
    /// Retain the platform snapshot lease and the activation's independently owned permissions.
    pub fn open(snapshot: &Snapshot, scope: &ReadScope, allowed_roots: &[String]) -> Self {
        // The returned source retains a lease on the same snapshot after its caller releases this argument.
        Self {
            snapshot: snapshot.clone(),
            scope: scope.clone(),
            allowed_roots: allowed_roots.to_vec(),
        }
    }

    /// Read an ordered member page and hash its selected names and continuation.
    fn members(
        &self,
        scope: &ReadScope,
        request: &RecordRequest,
        key: String,
        selection: &MemberSelection,
    ) -> RecordRead {
        if selection.order != "canonical" {
            return RecordRead::Failed(ReadFault::SelectionMismatch);
        }
        let prefix = key.len() + 1;
        // The platform takes ownership of the bounds. The returned witness still needs the original selection.
        let Ok(page) = lookup_members(
            &self.snapshot,
            key,
            selection.lower.clone(),
            selection.upper.clone(),
            selection.continuation.clone(),
            request.byte_limit,
        ) else {
            return RecordRead::Failed(ReadFault::DamagedRecord);
        };
        let mut value = Vec::new();
        // Platform lookup_members returns only full keys below key + tab in canonical order.
        for (index, member) in page.members.iter().enumerate() {
            let Some(name) = member.key.get(prefix..) else {
                return RecordRead::Failed(ReadFault::InvalidEvidence);
            };
            if index > 0 {
                value.push(b'\n')
            }
            value.extend_from_slice(name.as_bytes());
        }
        if i64::try_from(value.len()).map_or(true, |size| size > request.byte_limit) {
            return RecordRead::Failed(ReadFault::InvalidEvidence);
        }
        let digest = selection_digest(
            &[
                &selection.selector,
                &selection.order,
                &selection.lower,
                &selection.upper,
                &page.continuation,
            ],
            &value,
        );
        // Each returned fact owns its scope and request beyond the read.
        RecordRead::Members(MemberFact {
            scope: scope.clone(),
            request: request.clone(),
            value,
            membership_digest: digest,
            continuation: page.continuation,
        })
    }
}

/// Hash the ordered selection components and the lowercase hex digest of its bytes.
fn selection_digest(parts: &[&str], value: &[u8]) -> String {
    let mut digest = Sha256::new();
    for s in parts {
        digest.update(s.as_bytes());
        digest.update(b"\n");
    }
    digest.update(format!("{:x}", Sha256::digest(value)).as_bytes());
    format!("{:x}", digest.finalize())
}

impl StoreRecordSource {
    /// Return authenticated evidence for the exact request under this activation.
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
        if !self
            .allowed_roots
            .iter()
            .any(|root| key == *root || key.strip_prefix(root).is_some_and(|tail| tail.starts_with('\t')))
        {
            return RecordRead::Failed(ReadFault::OutsidePreparation);
        }
        if let RecordSelection::Members(s) = &request.selection {
            return self.members(scope, request, key, s);
        }
        let Ok(binding) = lookup_binding(&self.snapshot, key) else {
            return RecordRead::Failed(ReadFault::DamagedRecord);
        };
        // Returned evidence outlives the borrowed request and scope, including tombstone and exclusion results.
        match binding {
            BindingLookup::Deleted(deleted) => RecordRead::Deleted(DeletedFact {
                scope: scope.clone(),
                request: request.clone(),
                revision: deleted.revision,
            }),
            BindingLookup::Absent(absent) => RecordRead::Absent(AbsentFact {
                scope: scope.clone(),
                request: request.clone(),
                exclusion_digest: absent.exclusion_digest,
            }),
            BindingLookup::Present(present) => {
                let value = present.value;
                let (label, bytes) = match &request.selection {
                    RecordSelection::Field(s) => (format!("field\t{s}"), value),
                    RecordSelection::Payload(p) => {
                        let Some(selected) = selected_record_bytes(&value, request) else {
                            return RecordRead::Failed(ReadFault::InvalidEvidence);
                        };
                        (
                            format!("payload\t{}\t{}\t{}", p.reference, p.offset, p.length),
                            selected,
                        )
                    }
                    RecordSelection::Members(_) => {
                        return RecordRead::Failed(ReadFault::InvalidEvidence);
                    }
                };
                let digest = selection_digest(&[&present.selection_digest, &label], &bytes);
                RecordRead::Found(SelectedFact {
                    evidence: RevisionEvidence {
                        scope: scope.clone(),
                        request: request.clone(),
                        revision: present.revision,
                        semantic_digest: present.semantic_digest,
                    },
                    value: bytes,
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
