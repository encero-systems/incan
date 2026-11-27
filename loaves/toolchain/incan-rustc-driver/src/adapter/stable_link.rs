//! Darwin linker boundary that gives development objects durable, content-derived debug-map paths.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Private argument through which the adapter supplies its already validated absolute native store.
pub(crate) const STORE_ARGUMENT: &str = "--incan-link-input-store=";
/// The current compiler's optional completed regular-object handoff, owned by its private output lease.
pub(crate) const COMPLETED_ARGUMENT: &str = "--incan-link-completed-objects=";
const COMPLETED_LIMIT: u64 = 4 * 1024 * 1024;

/// A regular object assigned by the actual completed backend, with its independent cache-source outcome.
#[derive(Deserialize, Serialize)]
pub(crate) struct CompletedObject {
    pub group: String,
    pub source: PathBuf,
    pub source_identity: String,
    pub cached_source: Option<PathBuf>,
    pub same_cache_file: bool,
    pub saved_name: String,
    pub device: u64,
    pub inode: u64,
    pub length: u64,
    pub modified: (i64, i64),
}

/// The completed objects and finalized rustc session of this one ordinary link.
#[derive(Deserialize, Serialize)]
pub(crate) struct CompletedObjects {
    pub finalized: PathBuf,
    pub entries: Vec<CompletedObject>,
    pub previous: BTreeMap<String, PreviousObject>,
}

/// A selected earlier product admitted under the holder before rustc opens that generation.
#[derive(Deserialize, Serialize)]
pub(crate) struct PreviousObject {
    pub saved_name: String,
    pub selected: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub length: u64,
    pub modified: (i64, i64),
}

/// The regular object's identity and selected content path returned by the one real successful link.
#[derive(Deserialize, Serialize)]
pub(crate) struct LinkedObject {
    pub completed: CompletedObject,
    pub selected: PathBuf,
}

/// The regular inputs resolved by this invocation, returned only after the ordinary link succeeds.
#[derive(Deserialize, Serialize)]
pub(crate) struct LinkedObjects {
    pub finalized: PathBuf,
    pub entries: Vec<LinkedObject>,
}

struct CompletedLink {
    note: PathBuf,
    receipt: File,
    received_identity: String,
    finalized: PathBuf,
    sources: BTreeMap<PathBuf, CompletedObject>,
    previous: BTreeMap<String, PreviousObject>,
    linked: Vec<LinkedObject>,
}

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);
/// The system's `ld` launcher, which finds the selected toolchain's linker on every start.
const LAUNCHER: &str = "/usr/bin/ld";

/// Replace every direct object or archive input with an immutable content-named copy, then preserve ld's status and
/// diagnostics. Rust rlibs are archives even when their published Bakery path has no extension. The two halves of a
/// link, preparing the inputs and running ld, are each an invocation span, so a trace splits the link.
pub(crate) fn run(arguments: &[String]) -> io::Result<i32> {
    let store = link_store(arguments)?;
    let mut completed = CompletedLink::open(arguments);
    let rewritten = stable_arguments(&store, arguments, completed.as_mut())?;
    let linker = linker(arguments, std::env::var_os("TOOLCHAINS").is_some());
    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::run_linker", linker = %linker.display())
            .entered();
    // Development links skip ld's deduplication pass: a debug executable gains nothing from folded duplicates, and
    // the pass is a measurable share of an edit's link.
    let status = Command::new(linker).arg("-no_deduplicate").args(rewritten).status()?;
    drop(_invocation);
    if status.success()
        && let Some(completed) = completed
    {
        completed.finish();
    }
    Ok(status.code().unwrap_or(1))
}

/// The argument vector with every object and archive input named by its content, and the store argument removed.
fn stable_arguments(
    store: &Path,
    arguments: &[String],
    mut completed: Option<&mut CompletedLink>,
) -> io::Result<Vec<String>> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::link_inputs").entered();
    let mut rewritten = Vec::with_capacity(arguments.len());
    for argument in arguments {
        if argument.starts_with(STORE_ARGUMENT) || argument.starts_with(COMPLETED_ARGUMENT) {
            continue;
        }
        let path = Path::new(argument);
        match link_input_suffix(path)? {
            Some(suffix) => {
                let selected = match completed.as_deref_mut().filter(|_| suffix == "o") {
                    Some(completed) => completed.select_object(store, path)?,
                    None => stable_input(store, path, suffix)?,
                };
                rewritten.push(selected.to_string_lossy().into_owned());
            }
            None => rewritten.push(argument.clone()),
        }
    }
    Ok(rewritten)
}

impl CompletedLink {
    /// Accept one compiler-owned private note beside the exact output this invocation will link.
    fn open(arguments: &[String]) -> Option<Self> {
        let mut notes = arguments
            .iter()
            .filter_map(|value| value.strip_prefix(COMPLETED_ARGUMENT));
        let mut issued = notes.next()?.splitn(3, ':');
        let device: u64 = issued.next()?.parse().ok()?;
        let inode: u64 = issued.next()?.parse().ok()?;
        let note = Path::new(issued.next()?);
        let mut outputs = arguments.windows(2).filter(|pair| pair[0] == "-o");
        let output = Path::new(&outputs.next()?[1]);
        let directory = output.parent()?;
        if notes.next().is_some()
            || outputs.next().is_some()
            || !note.is_absolute()
            || note.parent()? != directory
            || !note.file_name()?.to_str()?.starts_with(".incan-completed-objects-")
            || !fs::symlink_metadata(note).ok()?.is_file()
        {
            return None;
        }

        let mut receipt = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(note)
            .ok()?;
        let metadata = receipt.metadata().ok()?;
        if !metadata.is_file()
            || metadata.dev() != device
            || metadata.ino() != inode
            || metadata.mode() & 0o777 != 0o600
            || metadata.nlink() != 1
            || metadata.len() > COMPLETED_LIMIT
        {
            return None;
        }
        let received_identity = file_identity(&metadata);
        let completed: CompletedObjects = serde_json::from_reader((&mut receipt).take(COMPLETED_LIMIT)).ok()?;
        if file_identity(&receipt.metadata().ok()?) != received_identity {
            return None;
        }
        let session_name = completed.finalized.file_name()?.to_str()?;
        if !completed.finalized.is_absolute() || !session_name.starts_with("s-") || session_name.ends_with("-working") {
            return None;
        }
        let mut sources = BTreeMap::new();
        for object in completed.entries {
            if object.source.parent()? != directory
                || object.source.extension()?.to_str()? != "o"
                || Path::new(&object.saved_name).file_name()?.to_str()? != object.saved_name
                || sources.insert(object.source.clone(), object).is_some()
            {
                return None;
            }
        }
        Some(Self {
            note: note.to_owned(),
            receipt,
            received_identity,
            finalized: completed.finalized,
            sources,
            previous: completed.previous,
            linked: Vec::new(),
        })
    }

    /// Choose a prior sealed input only for the same successfully transferred regular group and exact current operand.
    fn select_object(&mut self, store: &Path, source: &Path) -> io::Result<PathBuf> {
        let Some(completed) = self
            .sources
            .remove(source)
            .filter(|completed| current_object(completed))
        else {
            return stable_input(store, source, "o");
        };
        let working = self
            .finalized
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.rsplit_once('-'))
            .map(|(base, _)| self.finalized.with_file_name(format!("{base}-working")));
        let previous = self.previous.remove(&completed.group).filter(|prior| {
            completed.same_cache_file
                && completed.cached_source.as_deref().is_some_and(|cached| {
                    cached.parent() == working.as_deref()
                        && cached
                            .file_name()
                            .is_some_and(|name| name == completed.saved_name.as_str())
                })
                && completed.saved_name == prior.saved_name
                && completed.device == prior.device
                && completed.inode == prior.inode
                && completed.length == prior.length
                && completed.modified == prior.modified
                && verified_content_object(store, &prior.selected)
        });
        let selected = match previous.filter(|_| current_object(&completed)) {
            Some(prior) => prior.selected,
            None => stable_input(store, source, "o")?,
        };
        if current_object(&completed) {
            self.linked.push(LinkedObject {
                completed,
                selected: selected.clone(),
            });
        }
        Ok(selected)
    }

    /// Return the current link's observed selection through the same private file while its compiler owns it.
    fn finish(mut self) {
        let written = (|| -> io::Result<()> {
            let current = fs::symlink_metadata(&self.note)?;
            if !current.is_file()
                || file_identity(&current) != self.received_identity
                || file_identity(&self.receipt.metadata()?) != self.received_identity
            {
                return Ok(());
            }
            self.receipt.rewind()?;
            self.receipt.set_len(0)?;
            let mut writer = BufWriter::new(self.receipt);
            let linked = LinkedObjects {
                finalized: self.finalized,
                entries: self.linked,
            };
            serde_json::to_writer(&mut writer, &linked)?;
            writer.flush()
        })();
        let _ = written;
    }
}

/// The current issued argument is still the exact regular file returned by the backend.
fn current_object(object: &CompletedObject) -> bool {
    fs::symlink_metadata(&object.source)
        .is_ok_and(|found| found.is_file() && file_identity(&found) == object.source_identity)
}

/// A selected stable object must still authenticate under its exact ordinary content name.
pub(crate) fn content_object_digest<'a>(store: &Path, object: &'a Path) -> Option<&'a str> {
    let digest = object.file_stem()?.to_str()?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let expected = store
        .join("link-inputs/sha256")
        .join(&digest[..2])
        .join(format!("{digest}.o"));
    (object == expected).then_some(digest)
}

/// Verify the predecessor's content name against its current sealed object.
fn verified_content_object(store: &Path, object: &Path) -> bool {
    content_object_digest(store, object).is_some_and(|digest| {
        let Ok(before) = fs::symlink_metadata(object) else {
            return false;
        };
        before.is_file()
            && before.mode() & 0o222 == 0
            && identified_digest(store, object, link_input_digest).is_ok_and(|observed| observed == digest)
            && fs::symlink_metadata(object).is_ok_and(|after| file_identity(&before) == file_identity(&after))
    })
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
    let _copied = io::copy(&mut input, &mut output)?;
    #[cfg(test)]
    tests::record_link_copy(_copied);
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
pub(crate) fn file_identity(metadata: &fs::Metadata) -> String {
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
        #[cfg(test)]
        tests::record_link_hash(path, length as u64);
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::{LAUNCHER, linker, stable_input, verified_content_object};
    use std::cell::Cell;
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::sync::Barrier;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct LinkWork {
        read: u64,
        source_hash: u64,
        temporary_hash: u64,
        final_hash: u64,
        staged: u64,
    }

    impl LinkWork {
        const ZERO: Self = Self {
            read: 0,
            source_hash: 0,
            temporary_hash: 0,
            final_hash: 0,
            staged: 0,
        };

        /// Total bytes hashed regardless of where verification happens in the link.
        fn hashed(self) -> u64 {
            self.source_hash + self.temporary_hash + self.final_hash
        }
    }

    thread_local! {
        static LINK_WORK: Cell<LinkWork> = const { Cell::new(LinkWork::ZERO) };
    }

    /// Observe bytes read from the source and written to private stage by the production copy.
    pub(super) fn record_link_copy(bytes: u64) {
        LINK_WORK.with(|work| {
            let mut current = work.get();
            current.read += bytes;
            current.staged += bytes;
            work.set(current);
        });
    }

    /// Attribute production digest reads to the original source, private temporary, or settled target.
    pub(super) fn record_link_hash(path: &Path, bytes: u64) {
        LINK_WORK.with(|work| {
            let mut current = work.get();
            current.read += bytes;
            if path.extension() == Some(OsStr::new("tmp")) {
                current.temporary_hash += bytes;
            } else if path.parent().and_then(Path::parent).and_then(Path::file_name) == Some(OsStr::new("sha256")) {
                current.final_hash += bytes;
            } else {
                current.source_hash += bytes;
            }
            work.set(current);
        });
    }

    /// Return the bytes processed since the previous observation.
    fn take_link_work() -> LinkWork {
        LINK_WORK.with(|work| work.replace(LinkWork::ZERO))
    }

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

    /// Reuse a published target without repeated payload work and replace it whole when its settled bytes drift.
    #[test]
    fn a_damaged_content_name_is_replaced_whole() -> io::Result<()> {
        let root = scratch("damaged")?;
        let store = root.join("store");
        let object = root.join("unit.o");
        let bytes = b"object bytes";
        let length = bytes.len() as u64;
        fs::write(&object, bytes)?;
        let _ = take_link_work();
        let published = stable_input(&store, &object, "o")?;
        let publication = take_link_work();
        assert!(verified_content_object(&store, &published));
        assert_eq!(stable_input(&store, &object, "o")?, published);
        let reuse = take_link_work();
        assert_eq!(fs::read(&published)?, bytes);

        // The old witness stays on disk while the same inode's contents change under its original size and mtime.
        let original = fs::metadata(&published)?;
        let mut permissions = original.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&published, permissions)?;
        fs::write(&published, b"Object bytes")?;
        fs::OpenOptions::new()
            .write(true)
            .open(&published)?
            .set_times(fs::FileTimes::new().set_modified(original.modified()?))?;
        fs::set_permissions(&published, original.permissions())?;
        let stale = fs::metadata(&published)?;
        assert_eq!(stale.ino(), original.ino());
        assert_eq!(stale.len(), original.len());
        assert_eq!(stale.modified()?, original.modified()?);
        assert!(!verified_content_object(&store, &published));
        let refusal = take_link_work();
        assert_eq!(refusal.final_hash, length);

        // Repair publishes a different inode, and concurrent initial claimants agree on one verified target.
        let repaired = stable_input(&store, &object, "o")?;
        let _ = take_link_work();
        assert_eq!(repaired, published);
        assert_ne!(fs::metadata(&repaired)?.ino(), stale.ino());
        assert_eq!(fs::read(&repaired)?, bytes);
        assert!(verified_content_object(&store, &repaired));
        let repaired_reuse = take_link_work();
        verify_concurrent_publication(&root, &store)?;
        fs::remove_dir_all(root)?;

        assert!(
            publication.hashed() + reuse.hashed() <= length * 2
                && publication.read + reuse.read <= length * 3
                && publication.staged + reuse.staged == length
                && reuse == LinkWork::ZERO
                && repaired_reuse == LinkWork::ZERO,
            "link work persisted across publication and reuse: {publication:?}, {reuse:?}, repair reuse {repaired_reuse:?}"
        );
        Ok(())
    }

    /// Race two ordinary claimants to the same content while each owns its own staging work.
    fn verify_concurrent_publication(root: &Path, store: &Path) -> io::Result<()> {
        let source = root.join("parallel.o");
        let bytes = b"parallel object";
        fs::write(&source, bytes)?;
        let barrier = Barrier::new(2);
        let (first, second) = std::thread::scope(|scope| -> io::Result<_> {
            let contender = scope.spawn(|| {
                barrier.wait();
                stable_input(store, &source, "o")
            });
            barrier.wait();
            let first = stable_input(store, &source, "o");
            let second = contender
                .join()
                .map_err(|_| io::Error::other("concurrent claimant panicked"))?;
            Ok((first?, second?))
        })?;
        assert_eq!(first, second);
        assert_eq!(fs::read(&first)?, bytes);
        assert!(verified_content_object(store, &first));
        Ok(())
    }
}
