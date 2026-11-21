//! Re-record a corpus's expected outputs from one replay, when a product schema change leaves every plan unchanged.
//!
//! A product carries its schema's description closure, so a codec that gains a variant or a shape writes different
//! product bytes for unchanged plans, and no product of the new schema can equal a reference written by the old one. A
//! recording is a copy of the corpus whose cases keep every input and every reference answer, and whose expected
//! product files, report files and publications are the replay's own. It is refused when any call's answer differs
//! from its reference: it re-records outputs whose meaning is unchanged, never a plan.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::corpus::Case;

/// A corpus being recorded into `root`: `blobs/`, `cases/` and `publications/`, as [`crate::corpus::Corpus`] reads
/// them.
pub struct Recording {
    root: PathBuf,
}

impl Recording {
    /// Start a recording in `root`, which must not exist yet, with the source corpus's blobs and publications.
    pub fn create(source: &Path, root: &Path) -> Result<Self, String> {
        if root.exists() {
            return Err(format!("recording destination {} already exists", root.display()));
        }
        for part in ["blobs", "cases", "publications"] {
            std::fs::create_dir_all(root.join(part)).map_err(|error| error.to_string())?;
        }
        // Inputs and every publication the replay does not reach stay as the source has them.
        for part in ["blobs", "publications"] {
            let Ok(entries) = std::fs::read_dir(source.join(part)) else {
                continue;
            };
            for entry in entries {
                let path = entry.map_err(|error| error.to_string())?.path();
                if let Some(name) = path.file_name() {
                    std::fs::copy(&path, root.join(part).join(name)).map_err(|error| error.to_string())?;
                }
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// Store one file's bytes once, by their SHA-256, returning the digest a case names it by.
    pub fn store(&self, bytes: &[u8]) -> Result<String, String> {
        let digest = hex::encode(Sha256::digest(bytes));
        let path = self.root.join("blobs").join(&digest);
        if !path.exists() {
            std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        }
        Ok(digest)
    }

    /// Every file directly in `directory`, by name in name order, each stored as a blob.
    pub fn files(&self, directory: &Path) -> Result<Vec<(String, String)>, String> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_file() {
                let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                files.push((name, self.store(&bytes)?));
            }
        }
        files.sort();
        Ok(files)
    }

    /// Write one recorded case under its corpus file name.
    pub fn write_case(&self, file_name: &str, case: &Case) -> Result<(), String> {
        let bytes = postcard::to_allocvec(case).map_err(|error| error.to_string())?;
        std::fs::write(self.root.join("cases").join(file_name), bytes).map_err(|error| error.to_string())
    }

    /// Write the report a plan publishes, under the digest of the reference plan it is compared for.
    pub fn write_publication(&self, plan_digest: &str, files: &[(String, Vec<u8>)]) -> Result<(), String> {
        let bytes = postcard::to_allocvec(&files.to_vec()).map_err(|error| error.to_string())?;
        std::fs::write(
            self.root
                .join("publications")
                .join(format!("{plan_digest}.publication")),
            bytes,
        )
        .map_err(|error| error.to_string())
    }
}

/// Refuse a recording when any call's answer differed from its reference, naming how many and the first.
pub fn recordable(differences: &[String]) -> Result<(), String> {
    match differences.first() {
        None => Ok(()),
        Some(first) => Err(format!(
            "refusing to record: {} call answers differ from the reference; first: {first}",
            differences.len()
        )),
    }
}
