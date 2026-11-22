//! A namespace frame read through the worker's frame reader is authenticated once, by the platform store's frame
//! check, and a payload whose bytes changed in the segment is refused there.

use incan_worker_host_bindings::{begin, commit, disk_root, find, owned, put, store_error_text, store_snapshot};
use internal_models_native::namespace_references::NamespaceReference;
use internal_models_native::records::{RecordKey, encode_record_key};
use sha2::{Digest, Sha256};

use crate::namespace_frames::{namespace_frame_reader, read_namespace_frame};

/// One test's private store directory, removed when the test ends.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn namespace_key() -> RecordKey {
    RecordKey {
        family: "namespace".into(),
        identity: "src/domain.incn".into(),
        identity_profile: "incan.identity/1".into(),
        origin: "project".into(),
        facet: "checked".into(),
        context: "current".into(),
    }
}

/// Commit `payload` as the namespace record of a fresh store and return the store and the reference Oven would hand
/// over for it.
fn stored_namespace(name: &str, payload: &[u8]) -> Result<(Scratch, NamespaceReference), String> {
    let directory = std::env::temp_dir().join(format!("iwhn-frames-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let scratch = Scratch(directory);
    let root = scratch.0.to_string_lossy().into_owned();
    let view = store_snapshot(disk_root(root.clone()), "view".to_owned()).map_err(|error| store_error_text(&error))?;
    let key = encode_record_key(&namespace_key())?;
    let committed =
        commit(put(begin(&view), key.clone(), &payload.to_vec(), 0)).map_err(|error| store_error_text(&error))?;
    let location = find(&committed.root, committed.index.clone(), &key)
        .map_err(|error| error.detail)?
        .ok_or("the committed namespace record has no location")?;
    let reference = NamespaceReference {
        key: namespace_key(),
        segment: location.segment.clone(),
        offset: location.offset,
        length: location.length,
        frame_digest: location.digest.clone(),
        value_digest: hex(&Sha256::digest(payload)),
    };
    Ok((scratch, reference))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// An untouched frame reads back as its payload; the same frame with one payload byte changed in its segment is
/// refused by the store's frame check before any consumer sees it.
#[test]
fn a_namespace_frame_whose_payload_changed_is_refused() -> Result<(), String> {
    let payload = b"namespace interface and body".to_vec();
    let (scratch, reference) = stored_namespace("tampered", &payload)?;
    let root = scratch.0.to_string_lossy().into_owned();
    let view = read_namespace_frame(&mut namespace_frame_reader(&root), &reference)?;
    assert_eq!(owned(&view), payload);

    let segment = scratch.0.join("segments").join(format!("{}.seg", reference.segment));
    let mut bytes = std::fs::read(&segment).map_err(|error| error.to_string())?;
    let last = usize::try_from(reference.offset + reference.length - 1).map_err(|error| error.to_string())?;
    *bytes.get_mut(last).ok_or("the frame runs past its segment")? ^= 0x01;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&segment, std::fs::Permissions::from_mode(0o644)).map_err(|error| error.to_string())?;
    std::fs::write(&segment, bytes).map_err(|error| error.to_string())?;
    let refused = read_namespace_frame(&mut namespace_frame_reader(&root), &reference).err();
    assert!(
        refused
            .as_deref()
            .is_some_and(|error| error.contains("frame does not match its digest")),
        "{refused:?}"
    );
    Ok(())
}
