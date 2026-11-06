//! Command-owned pinned Git blob reads for native preparation and current metadata (#1337, #1698).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use super::{
    Error, environment, index_file_with_counter, validate_index_commit, validate_index_pin, validate_index_relative,
};

/// Actual work performed by one pinned index reader; callers identify its preparation or validation boundary.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct IndexBatchWork {
    /// Git processes started for capability selection, either transport and optional adoption listing.
    pub processes: usize,
    /// Requests written to the persistent child, including the initial commit-type check.
    pub requests: usize,
    /// File demands, including repeated demands served by command-local bytes.
    pub file_requests: usize,
    /// Distinct blob payloads read through the selected transport.
    pub blob_reads: usize,
    /// Repeated file demands served from the same pinned command snapshot.
    pub cache_hits: usize,
    /// Actual blob payload bytes read through the selected transport, excluding headers and commit metadata.
    pub blob_bytes: usize,
}

/// One canonical pin with a NUL-framed child when supported, otherwise the conservative canonical transport.
pub(crate) struct PinnedIndexBatch {
    commit: String,
    index: std::path::PathBuf,
    child: Option<Child>,
    input: Option<ChildStdin>,
    output: Option<BufReader<ChildStdout>>,
    batch: bool,
    finished: bool,
    failed: bool,
    stderr: std::fs::File,
    files: BTreeMap<String, Vec<u8>>,
    adoptions: Option<Vec<String>>,
    work: IndexBatchWork,
}

impl PinnedIndexBatch {
    /// Select the supported transport explicitly; older Git retains canonical blob reads without a new version gate.
    pub(crate) fn open(index: &Path, commit: &str) -> Result<Self, Error> {
        validate_index_commit(commit)?;
        let index = index.canonicalize()?;
        let child = Command::new("git")
            .arg("-C")
            .arg(&index)
            .args(["cat-file", "--batch", "-Z"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let capability = child.wait_with_output()?;
        let supported = batch_support(&capability)?;
        Self::with_support(
            &index,
            commit,
            supported,
            IndexBatchWork {
                processes: 1,
                ..Default::default()
            },
        )
    }

    /// Construct either selected transport with the same full commit admission; tests can exercise older-Git fallback.
    fn with_support(index: &Path, commit: &str, batch: bool, mut work: IndexBatchWork) -> Result<Self, Error> {
        validate_index_commit(commit)?;
        let index = index.canonicalize()?;
        let stderr = tempfile::tempfile()?;
        let mut reader = Self {
            commit: commit.to_string(),
            index,
            child: None,
            input: None,
            output: None,
            batch,
            finished: false,
            failed: false,
            stderr,
            files: BTreeMap::new(),
            adoptions: None,
            work: IndexBatchWork::default(),
        };
        if batch {
            let mut child = Command::new("git")
                .arg("-C")
                .arg(&reader.index)
                .args(["cat-file", "--batch", "-Z"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(reader.stderr.try_clone()?)
                .spawn()?;
            work.processes += 1;
            reader.input = child.stdin.take();
            reader.output = child.stdout.take().map(BufReader::new);
            reader.child = Some(child);
            reader.work = work;
            let object = reader.request(commit)?;
            if object.kind != "commit" || !object.identity.eq_ignore_ascii_case(commit) {
                return Err("index pin must identify an existing Git commit object".into());
            }
        } else {
            validate_index_pin(&reader.index, commit, &mut work.processes)?;
            reader.work = work;
        }
        Ok(reader)
    }

    /// Read one raw blob at the admitted pin, preserving binary bytes and newline-containing path names.
    pub(crate) fn read(&mut self, relative: &str) -> Result<Vec<u8>, Error> {
        self.ensure_open()?;
        validate_index_relative(relative)?;
        self.work.file_requests += 1;
        if let Some(bytes) = self.files.get(relative) {
            self.work.cache_hits += 1;
            return Ok(bytes.clone());
        }
        let bytes = if self.batch {
            let object = self.request(&format!("{}:{relative}", self.commit))?;
            if object.kind != "blob" {
                self.failed = true;
                self.stop();
                return Err(format!("pinned index file is not a blob: {relative}").into());
            }
            object.bytes
        } else {
            match index_file_with_counter(&self.index, &self.commit, relative, &mut self.work.processes) {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.failed = true;
                    return Err(error);
                }
            }
        };
        self.work.blob_reads += 1;
        self.work.blob_bytes += bytes.len();
        self.files.insert(relative.to_string(), bytes.clone());
        Ok(bytes)
    }

    /// List adoption paths once through the existing projection; file reads continue through the held pin.
    pub(crate) fn adoption_paths(&mut self) -> Result<Vec<String>, Error> {
        self.ensure_open()?;
        if self.adoptions.is_none() {
            match environment::adoption_paths_with_counter(&self.index, &self.commit, &mut self.work.processes) {
                Ok(paths) => self.adoptions = Some(paths),
                Err(error) => {
                    self.failed = true;
                    self.stop();
                    return Err(error);
                }
            }
        }
        Ok(self
            .adoptions
            .as_ref()
            .ok_or("pinned adoption listing missing")?
            .clone())
    }

    /// Borrow actual process, protocol and blob work performed by this reader.
    pub(crate) fn work(&self) -> &IndexBatchWork {
        &self.work
    }

    /// Close input, reap the child and require success before admitting a completed command's metadata result.
    pub(crate) fn finish(&mut self) -> Result<(), Error> {
        if self.failed {
            return Err("pinned index reader previously refused a demand".into());
        }
        if self.finished {
            return Ok(());
        }
        drop(self.input.take());
        if let Some(mut child) = self.child.take() {
            let status = match child.wait() {
                Ok(status) => status,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    self.failed = true;
                    return Err(error.into());
                }
            };
            if !status.success() {
                self.failed = true;
                return Err(format!("pinned index batch failed ({status}): {}", self.stderr_text()?).into());
            }
        }
        self.finished = true;
        Ok(())
    }

    /// A completed or failed command cannot continue serving cached data as a live admitted context.
    fn ensure_open(&self) -> Result<(), Error> {
        if self.finished || self.failed {
            return Err("pinned index reader is closed or refused".into());
        }
        Ok(())
    }

    /// Exchange one complete frame; any protocol/I/O failure terminates and reaps the child before returning.
    fn request(&mut self, request: &str) -> Result<GitObject, Error> {
        let result = (|| -> Result<GitObject, Error> {
            let input = self.input.as_mut().ok_or("pinned index batch is closed")?;
            input.write_all(request.as_bytes())?;
            input.write_all(&[0])?;
            input.flush()?;
            self.work.requests += 1;
            read_object(self.output.as_mut().ok_or("pinned index batch has no output")?, request)
        })();
        match result {
            Ok(object) => Ok(object),
            Err(source) => {
                self.failed = true;
                self.stop();
                let stderr = self
                    .stderr_text()
                    .unwrap_or_else(|error| format!("cannot read Git diagnostics: {error}"));
                Err(Box::new(IndexBatchFailure { source, stderr }))
            }
        }
    }

    /// Reap an interrupted/refused reader without letting a blocked child outlive the command.
    fn stop(&mut self) {
        drop(self.input.take());
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Read complete child diagnostics from a file, avoiding an unread stderr pipe deadlock.
    fn stderr_text(&mut self) -> Result<String, Error> {
        self.stderr.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        self.stderr.read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

impl Drop for PinnedIndexBatch {
    /// Error/early-return paths retain no running or unreaped Git child.
    fn drop(&mut self) {
        self.stop();
    }
}

/// Only an unsupported-option usage response selects fallback; repository and transport failures remain refusals.
fn batch_support(output: &std::process::Output) -> Result<bool, Error> {
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(129) {
        return Ok(false);
    }
    Err(format!(
        "cannot select pinned index Git transport ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    )
    .into())
}

/// Preserve the original protocol/I/O refusal and Git's diagnostics without flattening its source.
#[derive(Debug, thiserror::Error)]
#[error("pinned index batch refused: {source}; Git diagnostics: {stderr}")]
struct IndexBatchFailure {
    #[source]
    source: Error,
    stderr: String,
}

/// One size-delimited object; neither arbitrary text splitting nor a product-specific size ceiling is used.
struct GitObject {
    identity: String,
    kind: String,
    bytes: Vec<u8>,
}

/// Decode one NUL header, exact byte count and trailing NUL; reject missing and malformed responses explicitly.
fn read_object(output: &mut impl BufRead, request: &str) -> Result<GitObject, Error> {
    let mut header = Vec::new();
    if output.read_until(0, &mut header)? == 0 || header.pop() != Some(0) {
        return Err("pinned index batch ended without a complete header".into());
    }
    if header == format!("{request} missing").as_bytes() {
        return Err(format!("pinned index file unavailable: {request}").into());
    }
    let fields = std::str::from_utf8(&header)?.split(' ').collect::<Vec<_>>();
    let [identity, kind, size] = fields.as_slice() else {
        return Err("pinned index batch returned an invalid object header".into());
    };
    if identity.len() != 40
        || !identity.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !matches!(*kind, "blob" | "tree" | "commit" | "tag")
    {
        return Err("pinned index batch returned an invalid identity or type".into());
    }
    let size: usize = size.parse()?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size)?;
    bytes.resize(size, 0);
    output.read_exact(&mut bytes)?;
    let mut delimiter = [0];
    output.read_exact(&mut delimiter)?;
    if delimiter != [0] {
        return Err("pinned index batch object has an invalid delimiter".into());
    }
    Ok(GitObject {
        identity: identity.to_string(),
        kind: kind.to_string(),
        bytes,
    })
}

#[cfg(test)]
mod tests;
