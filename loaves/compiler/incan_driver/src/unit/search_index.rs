//! Verify the exact content-named search-index bytes before adding any directories to a compiled unit.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::SEARCH_INDEX_SCHEMA;
use crate::error::{CliError, CliResult};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyIndex {
    schema: String,
    search: Vec<PathBuf>,
}

pub(super) fn read_search_index(path: &Path) -> CliResult<Vec<PathBuf>> {
    let current = path.extension().and_then(|e| e.to_str()) == Some("crumb");
    let bytes = if current {
        let file = fs::File::open(path).map_err(|e| failed(path, e))?;
        let limit = 64 * 1024 * 1024;
        if file.metadata().map_err(|e| failed(path, e))?.len() > limit {
            return Err(failed(path, "Crumb index exceeds 64 MiB"));
        }
        let mut bytes = Vec::new();
        file.take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| failed(path, e))?;
        if bytes.len() as u64 > limit {
            return Err(failed(path, "Crumb index exceeds 64 MiB"));
        }
        bytes
    } else {
        fs::read(path).map_err(|e| failed(path, e))?
    };
    let named = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
    if hex::encode(Sha256::digest(&bytes)) != named {
        return Err(failed(path, "does not match the digest it is named by"));
    }
    if current {
        return internal_models::decode_search_paths(&bytes)
            .map(|paths| paths.into_iter().map(PathBuf::from).collect())
            .map_err(|error| failed(path, error));
    }
    let index: LegacyIndex = serde_json::from_slice(&bytes).map_err(|error| failed(path, error))?;
    if index.schema != SEARCH_INDEX_SCHEMA {
        return Err(failed(path, "unsupported legacy search index schema"));
    }
    Ok(index.search)
}

fn failed(path: &Path, reason: impl std::fmt::Display) -> CliError {
    CliError::failure(format!("cannot read the search index {}: {reason}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_roots_preserve_generation_and_digest() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = tempfile::tempdir()?;
        let roots = vec!["/a path".to_owned(), "/β".to_owned()];
        let expected: Vec<_> = roots.iter().map(PathBuf::from).collect();
        let bytes = internal_models::encode_search_paths(&roots)?;
        let path = scratch.path().join(format!("{:x}.crumb", Sha256::digest(&bytes)));
        fs::write(&path, &bytes)?;
        assert_eq!(read_search_index(&path)?, expected);

        let old = serde_json::to_vec(&serde_json::json!({"schema":SEARCH_INDEX_SCHEMA, "search":roots}))?;
        let legacy = scratch.path().join(format!("{:x}.json", Sha256::digest(&old)));
        fs::write(&legacy, &old)?;
        assert_eq!(read_search_index(&legacy)?, expected);
        let crossed = legacy.with_extension("crumb");
        fs::write(&crossed, &old)?;
        assert!(read_search_index(&crossed).is_err());
        fs::write(&path, &old)?;
        assert!(read_search_index(&path).is_err());
        assert!(internal_models::encode_search_paths(&vec!["z".to_owned(), "a".to_owned()]).is_err());
        Ok(())
    }
}
