//! Darwin linker boundary that gives development objects durable, content-derived debug-map paths.

use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Private argument through which the adapter supplies its already validated absolute native store.
pub(crate) const STORE_ARGUMENT: &str = "--incan-link-input-store=";
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Replace every direct object or archive input with an immutable content-named copy, then preserve ld's status and
/// diagnostics. Rust rlibs are archives even when their published Bakery path has no extension.
pub(crate) fn run(arguments: &[String]) -> io::Result<i32> {
    let store = link_store(arguments)?;
    let mut rewritten = Vec::with_capacity(arguments.len());
    for argument in arguments {
        if argument.starts_with(STORE_ARGUMENT) {
            continue;
        }
        let path = Path::new(argument);
        match link_input_suffix(path)? {
            Some(suffix) => rewritten.push(stable_input(&store, path, suffix)?.to_string_lossy().into_owned()),
            None => rewritten.push(argument.clone()),
        }
    }
    // Development links skip ld's deduplication pass: a debug executable gains nothing from folded duplicates, and
    // the pass is a measurable share of an edit's link.
    let status = Command::new("/usr/bin/ld")
        .arg("-no_deduplicate")
        .args(rewritten)
        .status()?;
    Ok(status.code().unwrap_or(1))
}

/// Select exactly one absolute store path and reject an invocation that could escape the development store.
fn link_store(arguments: &[String]) -> io::Result<PathBuf> {
    let stores = arguments
        .iter()
        .filter_map(|argument| argument.strip_prefix(STORE_ARGUMENT))
        .collect::<Vec<_>>();
    let [store] = stores.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stable linker requires exactly one native store",
        ));
    };
    let store = PathBuf::from(store);
    if !store.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "stable linker native store must be absolute",
        ));
    }
    Ok(store)
}

/// Classify the link inputs whose paths Darwin records, recognizing extensionless Bakery archives by their magic.
fn link_input_suffix(path: &Path) -> io::Result<Option<&'static str>> {
    if !path.is_file() {
        return Ok(None);
    }
    if path.extension().and_then(|extension| extension.to_str()) == Some("o") {
        return Ok(Some("o"));
    }
    let mut file = File::open(path)?;
    let mut magic = [0_u8; 8];
    if file.read(&mut magic)? == magic.len() && magic == *b"!<arch>\n" {
        return Ok(Some("a"));
    }
    Ok(None)
}

/// Publish one link input once beneath its digest; an existing content name must still contain those exact bytes.
fn stable_input(store: &Path, source: &Path, suffix: &str) -> io::Result<PathBuf> {
    let digest = identified_digest(store, source)?;
    let directory = store.join("link-inputs/sha256").join(&digest[..2]);
    let target = directory.join(format!("{digest}.{suffix}"));
    fs::create_dir_all(&directory)?;
    if target.is_file() {
        if identified_digest(store, &target)? == digest {
            return Ok(target);
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "stable linker object does not match its content name: {}",
                target.display()
            ),
        ));
    }

    let number = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(".{digest}.{}.{number}.tmp", std::process::id()));
    let copied = copy_object(source, &temporary).and_then(|()| {
        if link_input_digest(&temporary)? != digest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "stable linker object changed while copying",
            ));
        }
        match fs::hard_link(&temporary, &target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && link_input_digest(&target)? == digest => {
                Ok(())
            }
            Err(error) => Err(error),
        }
    });
    let _ = fs::remove_file(&temporary);
    copied?;
    Ok(target)
}

/// Copy into a new sibling, sync it, and make the completed store input immutable before publication.
fn copy_object(source: &Path, target: &Path) -> io::Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(target)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    let mut permissions = output.metadata()?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(target, permissions)
}

/// A link input's digest, hashed once per file identity: device, inode, size, and modification and change times.
///
/// The store keeps one record per device and inode with the identity it was hashed at. A record whose identity still
/// matches answers without reading the file; any write changes the identity, so the file is hashed again. A file whose
/// identity changed while it was hashed is not recorded.
fn identified_digest(store: &Path, path: &Path) -> io::Result<String> {
    let metadata = fs::metadata(path)?;
    let before = file_identity(&metadata);
    let record = store
        .join("link-inputs/identities")
        .join(format!("{}-{}", metadata.dev(), metadata.ino()));
    if let Ok(text) = fs::read_to_string(&record) {
        if let Some((identity, digest)) = text.trim_end().rsplit_once(' ') {
            if identity == before && digest.len() == 64 {
                return Ok(digest.to_owned());
            }
        }
    }
    let digest = link_input_digest(path)?;
    if file_identity(&fs::metadata(path)?) == before {
        record_identity(&record, &format!("{before} {digest}\n"));
    }
    Ok(digest)
}

/// The identity a file's content is recorded under; a write changes its size, modification time or change time.
fn file_identity(metadata: &fs::Metadata) -> String {
    format!(
        "{} {} {} {}.{} {}.{}",
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec()
    )
}

/// Replace one identity record atomically; a record that cannot be written only costs the next link a hash.
fn record_identity(record: &Path, text: &str) {
    let Some(directory) = record.parent() else {
        return;
    };
    if fs::create_dir_all(directory).is_err() {
        return;
    }
    let number = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(".{}.{number}.tmp", std::process::id()));
    if fs::write(&temporary, text).is_err() || fs::rename(&temporary, record).is_err() {
        let _ = fs::remove_file(&temporary);
    }
}

/// Hash one link input, as an invocation span a test can count: the linker's every read of an input's bytes.
fn link_input_digest(path: &Path) -> io::Result<String> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::link_input_digest", input = %path.display()).entered();
    file_digest(path)
}

/// Hash one complete native input without retaining a program-sized allocation.
pub(crate) fn file_digest(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(hex::encode(hash.finalize()))
}
