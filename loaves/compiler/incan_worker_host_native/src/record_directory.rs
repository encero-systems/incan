//! Serve installed compiler membership and absence proofs from an independently pinned store index.

use incan_worker_host_bindings::{disk_root, find, limited_entries_with_prefix, page_checked_bytes};

use crate::record_snapshot::valid_digest;

/// One installed compiler record member and its revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryMember {
    pub key: String,
    pub revision: String,
}

/// The members of one key range and the index bytes checked to read them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryAnswer {
    pub members: Vec<DirectoryMember>,
    pub checked_bytes: i64,
}

/// Order overlays by exact named facet, then declaration ordinal, retaining the original compiler key.
pub fn directory_member_key(seal: &str, key: &str, revision: &str) -> Result<String, String> {
    let parts: Vec<&str> = key.split('/').collect();
    if parts.len() != 6 || parts[0] != "loaf" || !valid_digest(parts[5]) || !valid_digest(revision) {
        return Err("invalid compiler directory member identity".to_owned());
    }
    if parts[4].len() < 8 || !parts[4].bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid compiler directory ordinal".to_owned());
    }
    Ok(format!(
        "{seal}\t{}/{}/{}/{revision}",
        parts[..4].join("/"),
        parts[5],
        parts[4]
    ))
}

/// Read just the index pages containing this key range, checking the seal anchor and each traversed page.
pub fn read_directory(
    directory: &str,
    top: &str,
    seal: &str,
    prefix: &str,
    maximum: i64,
) -> Result<DirectoryAnswer, String> {
    // A root is a single content-named page or a bounded page in a content-named pack.
    let fields: Vec<&str> = top.split(':').collect();
    let packed = fields.len() == 4 && valid_digest(fields[0]) && valid_digest(fields[3]);
    if !(valid_digest(top) || packed) || !valid_digest(seal) || maximum < 0 {
        return Err("invalid installed compiler directory pin".to_owned());
    }
    let path = disk_root(directory.to_owned());
    let before = page_checked_bytes();
    match find(&path, top.to_owned(), &format!("@{seal}")).map_err(|error| error.detail)? {
        Some(anchor) if anchor.tombstone => {
            return Err("installed compiler directory seal anchor is deleted".to_owned());
        }
        Some(_) => {}
        None => return Err("installed compiler directory belongs to another sealed set".to_owned()),
    }
    let mut members = Vec::new();
    let rows = limited_entries_with_prefix(&path, &top.to_owned(), &format!("{seal}\t{prefix}"), maximum)
        .map_err(|error| error.detail)?;
    for row in rows {
        match &row.location {
            Some(location) if location.tombstone => {
                return Err("unexpected deleted installed compiler directory member".to_owned());
            }
            Some(_) => {}
            None => return Err("missing installed compiler directory leaf location".to_owned()),
        }
        let parts: Vec<&str> = row.key.get(seal.len() + 1..).unwrap_or_default().split('/').collect();
        if parts.len() != 7 {
            return Err("invalid installed compiler directory leaf".to_owned());
        }
        let key = format!("{}/{}/{}", parts[..4].join("/"), parts[5], parts[4]);
        if directory_member_key(seal, &key, parts[6])? != row.key {
            return Err("noncanonical installed compiler directory leaf".to_owned());
        }
        members.push(DirectoryMember {
            key,
            revision: parts[6].to_owned(),
        });
    }
    Ok(DirectoryAnswer {
        members,
        checked_bytes: page_checked_bytes() - before,
    })
}
