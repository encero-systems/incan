//! Digest-first admission of deterministic, uncompressed Loaf source archives.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::digest_bytes;

/// Verified regular files from a Loaf archive. Links and non-file entries are refused.
pub struct VerifiedLoafArchive {
    files: BTreeMap<PathBuf, Vec<u8>>,
}

impl VerifiedLoafArchive {
    /// Verify the complete archive before interpreting any header or source bytes.
    pub fn read(path: &Path, expected_digest: &str) -> io::Result<Self> {
        let bytes = std::fs::read(path)?;
        if digest_bytes(&bytes) != expected_digest {
            return Err(invalid("Loaf source archive digest mismatch"));
        }
        let mut files = BTreeMap::new();
        let mut offset = 0usize;
        while let Some(header) = bytes.get(offset..offset.saturating_add(512)) {
            if header.iter().all(|byte| *byte == 0) {
                if bytes[offset..].iter().any(|byte| *byte != 0) {
                    return Err(invalid("nonzero bytes after archive terminator"));
                }
                return Ok(Self { files });
            }
            let checksum = octal(&header[148..156])?;
            let observed: usize = header
                .iter()
                .enumerate()
                .map(|(index, byte)| {
                    if (148..156).contains(&index) {
                        32
                    } else {
                        usize::from(*byte)
                    }
                })
                .sum();
            if checksum != observed {
                return Err(invalid("invalid archive header checksum"));
            }
            let name = text(&header[..100])?;
            let prefix = text(&header[345..500])?;
            let path = if prefix.is_empty() {
                PathBuf::from(name)
            } else {
                Path::new(prefix).join(name)
            };
            if path.as_os_str().is_empty() || path.components().any(|part| !matches!(part, Component::Normal(_))) {
                return Err(invalid("archive member must be owner-relative"));
            }
            let size = octal(&header[124..136])?;
            let start = offset
                .checked_add(512)
                .ok_or_else(|| invalid("archive offset overflow"))?;
            let end = start
                .checked_add(size)
                .ok_or_else(|| invalid("archive size overflow"))?;
            let body = bytes
                .get(start..end)
                .ok_or_else(|| invalid("truncated archive member"))?;
            match header[156] {
                0 | b'0' => {
                    if files.insert(path, body.to_vec()).is_some() {
                        return Err(invalid("duplicate archive member"));
                    }
                }
                b'5' if size == 0 => {}
                _ => {
                    return Err(invalid(
                        "unsupported archive member: only regular files and directories are admitted",
                    ));
                }
            }
            offset = end
                .checked_add(511)
                .map(|value| value / 512 * 512)
                .ok_or_else(|| invalid("archive padding overflow"))?;
        }
        Err(invalid("archive has no complete terminator"))
    }

    /// Read the adopted manifest without consulting upstream Cargo declarations.
    pub fn manifest(&self) -> io::Result<&[u8]> {
        self.files
            .get(Path::new("loaf.toml"))
            .map(Vec::as_slice)
            .ok_or_else(|| invalid("archive has no loaf.toml"))
    }

    /// Report the presence of an inert build script without reading its contents.
    pub fn has_build_script(&self) -> bool {
        self.files.contains_key(Path::new("build.rs"))
    }

    /// Materialize verified source files into a fresh private root, omitting the package root's inert Cargo and
    /// build-script files.
    ///
    /// Only files at the package root are withheld: a nested `src/build.rs` is an ordinary module (`mod build;`), and a
    /// nested `Cargo.toml` is a fixture or a vendored crate's data, so both are sources like any other file.
    pub fn materialize(&self, root: &Path) -> io::Result<()> {
        std::fs::create_dir(root)?;
        for (relative, bytes) in &self.files {
            let at_package_root = relative.parent().is_some_and(|parent| parent.as_os_str().is_empty());
            if at_package_root
                && matches!(
                    relative.to_str(),
                    Some("Cargo.toml" | "Cargo.toml.orig" | "Cargo.lock" | "build.rs")
                )
            {
                continue;
            }
            let destination = root.join(relative);
            let parent = destination
                .parent()
                .ok_or_else(|| invalid("archive member has no parent"))?;
            std::fs::create_dir_all(parent)?;
            std::fs::write(destination, bytes)?;
        }
        Ok(())
    }
}

/// Decode one NUL-terminated UTF-8 archive field.
fn text(bytes: &[u8]) -> io::Result<&str> {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).map_err(|error| invalid(&error.to_string()))
}

/// Decode the deterministic archive's octal size and checksum fields.
fn octal(bytes: &[u8]) -> io::Result<usize> {
    usize::from_str_radix(text(bytes)?.trim(), 8).map_err(|error| invalid(&error.to_string()))
}

/// Construct a malformed-source diagnostic without accepting a partial archive.
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct a minimal deterministic tar with a caller-selected path and regular-file payload.
    fn fixture(name: &str, contents: &[u8], kind: u8) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        let size = format!("{:011o}\0", contents.len());
        header[124..136].copy_from_slice(size.as_bytes());
        header[148..156].fill(b' ');
        header[156] = kind;
        let checksum: usize = header.iter().map(|byte| usize::from(*byte)).sum();
        let encoded = format!("{checksum:06o}\0 ");
        header[148..156].copy_from_slice(encoded.as_bytes());
        let mut bytes = header.to_vec();
        bytes.extend_from_slice(contents);
        bytes.resize(bytes.len().div_ceil(512) * 512 + 1024, 0);
        bytes
    }

    /// A digest mismatch wins over malformed headers, proving admission precedes interpretation.
    #[test]
    fn digest_is_checked_before_archive_headers() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("source.tar");
        std::fs::write(&path, b"not a tar")?;
        let error = match VerifiedLoafArchive::read(&path, &digest_bytes(b"different")) {
            Ok(_) => return Err("malformed archive was admitted".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("digest mismatch"));
        Ok(())
    }

    /// Verified archives still refuse traversal, links and corrupt header checksums.
    #[test]
    fn unsafe_archive_members_are_refused() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("source.tar");
        for (name, kind) in [("../escape", b'0'), ("/absolute", b'0'), ("link", b'2')] {
            let bytes = fixture(name, b"", kind);
            std::fs::write(&path, &bytes)?;
            assert!(VerifiedLoafArchive::read(&path, &digest_bytes(&bytes)).is_err());
        }
        let mut bytes = fixture("loaf.toml", b"[project]", b'0');
        bytes[0] = b'x';
        std::fs::write(&path, &bytes)?;
        assert!(VerifiedLoafArchive::read(&path, &digest_bytes(&bytes)).is_err());
        Ok(())
    }

    /// Admission preserves the authored manifest and refuses materialization into an existing root.
    #[test]
    fn verified_manifest_materializes_only_into_a_fresh_root() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("source.tar");
        let bytes = fixture("loaf.toml", b"[project]", b'0');
        std::fs::write(&path, &bytes)?;
        let archive = VerifiedLoafArchive::read(&path, &digest_bytes(&bytes))?;
        assert_eq!(archive.manifest()?, b"[project]");
        let destination = root.path().join("source");
        archive.materialize(&destination)?;
        assert_eq!(std::fs::read(destination.join("loaf.toml"))?, b"[project]");
        assert!(archive.materialize(&destination).is_err());
        Ok(())
    }
}
