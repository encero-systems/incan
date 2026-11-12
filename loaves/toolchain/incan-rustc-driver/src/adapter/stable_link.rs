//! Darwin linker boundary that gives development objects durable, content-derived debug-map paths.

use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Private argument through which the adapter supplies its already validated absolute native store.
pub(crate) const STORE_ARGUMENT: &str = "--incan-link-input-store=";
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Replace every direct object input with an immutable content-named copy, then preserve ld's status and diagnostics.
pub(crate) fn run(arguments: &[String]) -> io::Result<i32> {
    let store = link_store(arguments)?;
    let mut rewritten = Vec::with_capacity(arguments.len());
    for argument in arguments {
        if argument.starts_with(STORE_ARGUMENT) {
            continue;
        }
        let path = Path::new(argument);
        if path.extension().and_then(|extension| extension.to_str()) == Some("o") && path.is_file() {
            rewritten.push(stable_object(&store, path)?.to_string_lossy().into_owned());
        } else {
            rewritten.push(argument.clone());
        }
    }
    let status = Command::new("/usr/bin/ld").args(rewritten).status()?;
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

/// Publish one object once beneath its digest; an existing content name must still contain those exact bytes.
fn stable_object(store: &Path, source: &Path) -> io::Result<PathBuf> {
    let digest = file_digest(source)?;
    let directory = store.join("link-inputs/sha256").join(&digest[..2]);
    let target = directory.join(format!("{digest}.o"));
    fs::create_dir_all(&directory)?;
    if target.is_file() {
        if file_digest(&target)? == digest {
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
        if file_digest(&temporary)? != digest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "stable linker object changed while copying",
            ));
        }
        match fs::hard_link(&temporary, &target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && file_digest(&target)? == digest => Ok(()),
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

/// Hash one complete object without retaining a program-sized allocation.
fn file_digest(path: &Path) -> io::Result<String> {
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
