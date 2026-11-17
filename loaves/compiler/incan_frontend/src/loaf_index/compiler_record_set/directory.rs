//! Logical records selected by prefix and optional exact name from a toolchain-pinned immutable directory (#2946).
use super::{OnceLock, RecordEntry, RecordSet, valid_digest};

/// Installed entry point provided by the native driver, which links the Incan store implementation.
pub type DirectoryReader = fn(&str, &str, &str, &str, usize) -> Result<(Vec<(String, String)>, usize), String>;
static READER: OnceLock<DirectoryReader> = OnceLock::new();

/// Connect the compiler support library to the driver's generation-matched Incan worker host.
pub fn install_directory_reader(reader: DirectoryReader) {
    let _ = READER.set(reader);
}

impl RecordSet {
    /// `limit == 0` leaves the exact selected range unrestricted (e.g. all overloads). Existence probes ask for one.
    pub(super) fn members(
        &self,
        prefix: &str,
        member_hash: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RecordEntry>, String> {
        let suffix = member_hash.map(|hash| format!("/{hash}"));
        let accepts = |key: &str| key.starts_with(prefix) && suffix.as_ref().is_none_or(|suffix| key.ends_with(suffix));
        if let Some((seal, top)) = &self.directory {
            let reader = READER
                .get()
                .ok_or("installed compiler selector has no generation-matched reader")?;
            let path = self.root.join("selection");
            let selected = member_hash.map_or_else(|| prefix.to_owned(), |hash| format!("{prefix}{hash}/"));
            let (rows, bytes) = reader(
                path.to_str().ok_or("non-UTF-8 compiler selector directory")?,
                top,
                seal,
                &selected,
                limit,
            )?;
            tracing::trace!(target: "incan::invocation", phase = "record_directory_pages", digest = %seal,
                root = %top, selector = %selected, bytes);
            let mut result = Vec::with_capacity(rows.len());
            for (key, revision) in rows {
                if !accepts(&key) {
                    return Err("installed compiler reader returned a different selector".into());
                }
                valid_digest(&revision)?;
                result.push(RecordEntry {
                    key,
                    value: revision.clone(),
                    revision,
                });
            }
            result.sort_unstable_by(|a, b| a.key.cmp(&b.key));
            if result.windows(2).any(|pair| pair[0].key == pair[1].key) {
                return Err("duplicate installed compiler directory key".into());
            }
            return Ok(result);
        }
        let count = if limit == 0 { usize::MAX } else { limit };
        let first = self.entries.partition_point(|row| row.key.as_str() < prefix);
        Ok(self.entries[first..]
            .iter()
            .take_while(|row| row.key.starts_with(prefix))
            .filter(|row| accepts(&row.key))
            .take(count)
            .cloned()
            .collect())
    }
}
