//! Darwin linker boundary that gives development objects durable, content-derived debug-map paths.

use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Private argument through which the adapter supplies its already validated absolute native store.
pub(crate) const STORE_ARGUMENT: &str = "--incan-link-input-store=";
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);
/// The system's `ld` launcher, which finds the selected toolchain's linker on every start.
const LAUNCHER: &str = "/usr/bin/ld";

/// Replace every direct object or archive input with an immutable content-named copy, then preserve ld's status and
/// diagnostics. Rust rlibs are archives even when their published Bakery path has no extension. The two halves of a
/// link, preparing the inputs and running ld, are each an invocation span, so a trace splits the link.
pub(crate) fn run(arguments: &[String]) -> io::Result<i32> {
    let store = link_store(arguments)?;
    let rewritten = stable_arguments(&store, arguments)?;
    let linker = linker(arguments, std::env::var_os("TOOLCHAINS").is_some());
    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::run_linker", linker = %linker.display())
            .entered();
    // Development links skip ld's deduplication pass: a debug executable gains nothing from folded duplicates, and
    // the pass is a measurable share of an edit's link.
    let status = Command::new(linker).arg("-no_deduplicate").args(rewritten).status()?;
    Ok(status.code().unwrap_or(1))
}

/// The argument vector with every object and archive input named by its content, and the store argument removed.
fn stable_arguments(store: &Path, arguments: &[String]) -> io::Result<Vec<String>> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::link_inputs").entered();
    let mut rewritten = Vec::with_capacity(arguments.len());
    for argument in arguments {
        if argument.starts_with(STORE_ARGUMENT) {
            continue;
        }
        let path = Path::new(argument);
        match link_input_suffix(path)? {
            Some(suffix) => rewritten.push(stable_input(store, path, suffix)?.to_string_lossy().into_owned()),
            None => rewritten.push(argument.clone()),
        }
    }
    Ok(rewritten)
}

/// The `ld` a development link runs: the one that ships beside the SDK rustc selected (`-syslibroot`), run directly.
/// `/usr/bin/ld` is a launcher that looks the same program up on every start, a measurable share of an edit's link;
/// it stays the answer when the caller selected a toolchain by name (`TOOLCHAINS`), when the link names no SDK, and
/// when the SDK's developer directory holds no `ld`.
fn linker(arguments: &[String], toolchain_selected: bool) -> PathBuf {
    let sdk = arguments
        .windows(2)
        .find(|pair| pair[0] == "-syslibroot")
        .map(|pair| Path::new(&pair[1]));
    match sdk.filter(|_| !toolchain_selected).and_then(sdk_linker) {
        Some(linker) => linker,
        None => PathBuf::from(LAUNCHER),
    }
}

/// The `ld` of the developer directory that holds `sdk`: Xcode keeps an SDK at
/// `<developer>/Platforms/<name>.platform/Developer/SDKs/<sdk>` and its linker in the default toolchain; the Command
/// Line Tools keep an SDK at `<developer>/SDKs/<sdk>` and their linker at `<developer>/usr/bin/ld`.
fn sdk_linker(sdk: &Path) -> Option<PathBuf> {
    let sdks = sdk.parent()?;
    if sdks.file_name()? != "SDKs" {
        return None;
    }
    let above = sdks.parent()?;
    let platform = above.parent()?;
    let linker = if above.file_name()? == "Developer" && platform.extension().is_some_and(|kind| kind == "platform") {
        platform
            .parent()?
            .parent()?
            .join("Toolchains/XcodeDefault.xctoolchain/usr/bin/ld")
    } else {
        above.join("usr/bin/ld")
    };
    linker.is_file().then_some(linker)
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

/// Publish one link input once beneath its digest. An existing content name is reused only while it still holds
/// those exact bytes; one that does not, as a power loss before the unit's seal can leave it, is replaced whole by a
/// verified copy.
fn stable_input(store: &Path, source: &Path, suffix: &str) -> io::Result<PathBuf> {
    let digest = identified_digest(store, source, link_input_digest)?;
    let directory = store.join("link-inputs/sha256").join(&digest[..2]);
    let target = directory.join(format!("{digest}.{suffix}"));
    fs::create_dir_all(&directory)?;
    let damaged = target.is_file();
    if damaged && identified_digest(store, &target, link_input_digest)? == digest {
        return Ok(target);
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
        if damaged {
            return fs::rename(&temporary, &target);
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

/// Copy into a new sibling, hand its bytes to the device, and make the completed store input immutable before
/// publication.
///
/// The copy is flushed with `fsync`, not with the full device flush `File::sync_all` asks of macOS (`F_FULLFSYNC`),
/// which cost about 5 ms of every edit's link. The full flush is the seal's: the Bakery seals the linked unit with
/// one full device flush before any view of it exists, and that flush covers every earlier write to the same device,
/// this copy included. Until that seal nothing names this link's output, and a power loss in between can at worst
/// leave a content name over wrong bytes, which [`stable_input`] replaces at its next use.
fn copy_object(source: &Path, target: &Path) -> io::Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new().write(true).create_new(true).open(target)?;
    io::copy(&mut input, &mut output)?;
    // SAFETY: `output` is open for the whole call, so its descriptor is valid.
    if unsafe { libc::fsync(output.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut permissions = output.metadata()?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(target, permissions)
}

/// A native input's digest, hashed once per file identity: device, inode, size, and modification and change times.
///
/// The store keeps one record per device and inode with the identity it was hashed at. A record whose identity still
/// matches answers without reading the file; any write changes the identity, so the file is hashed again. A file whose
/// identity changed while it was hashed is not recorded. `hash` reads the bytes, as the caller traces it.
pub(crate) fn identified_digest(
    store: &Path,
    path: &Path,
    hash: fn(&Path) -> io::Result<String>,
) -> io::Result<String> {
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
    let digest = hash(path)?;
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

#[cfg(test)]
mod tests {
    use super::{LAUNCHER, identified_digest, link_input_digest, linker, stable_input};
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    /// A fresh directory for one test.
    fn scratch(name: &str) -> io::Result<PathBuf> {
        let directory = std::env::temp_dir().join(format!("incan-stable-link-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory)?;
        Ok(directory)
    }

    /// A link's arguments naming `sdk` as rustc names it.
    fn link_naming(sdk: &Path) -> Vec<String> {
        vec![
            "main.o".into(),
            "-syslibroot".into(),
            sdk.to_string_lossy().into_owned(),
            "-o".into(),
            "main".into(),
        ]
    }

    /// An empty file at `path`, with its directories.
    fn touch(path: &Path) -> io::Result<()> {
        fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("no parent"))?)?;
        fs::write(path, b"")
    }

    /// A link runs the linker beside its SDK directly, in Xcode's layout and in the Command Line Tools'.
    #[test]
    fn a_link_runs_the_linker_beside_its_sdk() -> io::Result<()> {
        let root = scratch("sdk-linker")?;
        let xcode = root.join("Xcode.app/Contents/Developer");
        let xcode_linker = xcode.join("Toolchains/XcodeDefault.xctoolchain/usr/bin/ld");
        touch(&xcode_linker)?;
        let xcode_sdk = xcode.join("Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk");
        assert_eq!(linker(&link_naming(&xcode_sdk), false), xcode_linker);
        let tools = root.join("CommandLineTools");
        let tools_linker = tools.join("usr/bin/ld");
        touch(&tools_linker)?;
        assert_eq!(
            linker(&link_naming(&tools.join("SDKs/MacOSX.sdk")), false),
            tools_linker
        );
        fs::remove_dir_all(root)
    }

    /// The launcher still chooses when a toolchain is selected by name, no SDK is named, or the SDK has no linker.
    #[test]
    fn the_launcher_links_where_no_sdk_linker_is_certain() -> io::Result<()> {
        let root = scratch("launcher")?;
        let tools = root.join("CommandLineTools");
        touch(&tools.join("usr/bin/ld"))?;
        let sdk = tools.join("SDKs/MacOSX.sdk");
        assert_eq!(linker(&link_naming(&sdk), true), Path::new(LAUNCHER));
        assert_eq!(linker(&["main.o".to_owned()], false), Path::new(LAUNCHER));
        let bare = root.join("Elsewhere/SDKs/MacOSX.sdk");
        assert_eq!(linker(&link_naming(&bare), false), Path::new(LAUNCHER));
        assert_eq!(
            linker(&link_naming(&root.join("MacOSX.sdk")), false),
            Path::new(LAUNCHER)
        );
        fs::remove_dir_all(root)
    }

    /// A content name left over wrong bytes is replaced by a verified copy, and an intact one is reused as it is.
    #[test]
    fn a_damaged_content_name_is_replaced_whole() -> io::Result<()> {
        let root = scratch("damaged")?;
        let store = root.join("store");
        let object = root.join("unit.o");
        fs::write(&object, b"object bytes")?;
        let published = stable_input(&store, &object, "o")?;
        assert_eq!(fs::read(&published)?, b"object bytes");
        let mut permissions = fs::metadata(&published)?.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&published, permissions)?;
        fs::write(&published, b"")?;
        let repaired = stable_input(&store, &object, "o")?;
        assert_eq!(repaired, published);
        assert_eq!(fs::read(&repaired)?, b"object bytes");
        let digest = identified_digest(&store, &repaired, link_input_digest)?;
        assert!(repaired.to_string_lossy().contains(&digest));
        assert_eq!(stable_input(&store, &object, "o")?, repaired);
        fs::remove_dir_all(root)
    }
}
