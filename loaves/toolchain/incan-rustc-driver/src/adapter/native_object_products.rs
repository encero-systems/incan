//! Preserve one ordinary link's regular products for a later, independently selected request.

use super::PrivateOutput;
use super::stable_link::{COMPLETED_ARGUMENT, LinkedObjects, PreviousObject, content_object_digest, file_identity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub(crate) const PREDECESSOR_LOCATOR: &str = "native-link-predecessor.json";
const PRODUCT_LIMIT: u64 = 4 * 1024 * 1024;
const FORMAT: u32 = 1;

/// Private handoff owned by the compiler until the ordinary link and output publish return.
pub(super) struct LinkCapture<'a> {
    receipt: tempfile::NamedTempFile,
    native_store: &'a Path,
    incremental: &'a Path,
    previous: BTreeMap<String, PreviousObject>,
}

impl<'a> LinkCapture<'a> {
    /// Bind the current linker option to the issuer's fresh private file and exact output holder.
    pub(super) fn issue(
        environment: Option<(&'a PrivateOutput, &'a Path)>,
        arguments: &mut Vec<String>,
        locator: Option<&Path>,
    ) -> Option<Self> {
        let (held, native_store) = environment?;
        let incremental = held.session.as_deref()?;
        let receipt = tempfile::Builder::new()
            .prefix(".incan-completed-objects-")
            .tempfile_in(held.path.parent()?)
            .ok()?;
        let metadata = receipt.as_file().metadata().ok()?;
        arguments.push(format!(
            "-Clink-arg={COMPLETED_ARGUMENT}{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            receipt.path().display(),
        ));
        let previous = previous(locator, native_store, incremental);
        Some(Self {
            receipt,
            native_store,
            incremental,
            previous,
        })
    }

    /// Move earlier authority into the backend while the compiler keeps the exact issued file.
    pub(super) fn backend_input(&mut self) -> Option<(File, BTreeMap<String, PreviousObject>)> {
        Some((
            self.receipt.as_file().try_clone().ok()?,
            std::mem::take(&mut self.previous),
        ))
    }

    /// Stage a candidate only after the native link and loader record have succeeded.
    pub(super) fn store_result(self, output: &Path) {
        store_result(self.receipt.as_file(), output, self.native_store, self.incremental);
    }
}

#[derive(Deserialize)]
struct SelectedReceipt {
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize, Serialize)]
struct StoredProduct {
    group: String,
    product: PreviousObject,
    saved_identity: String,
    restored: bool,
}

#[derive(Deserialize, Serialize)]
struct StoredLink {
    format: u32,
    finalized: PathBuf,
    products: Vec<StoredProduct>,
}

/// Read only the earlier product independently admitted by Oven, while the current native holder owns the session.
fn previous(locator: Option<&Path>, native_store: &Path, incremental: &Path) -> BTreeMap<String, PreviousObject> {
    let Some((selected, receipt)) = locator.and_then(|path| read_previous(path, native_store, incremental)) else {
        return BTreeMap::new();
    };
    let mut prior = BTreeMap::new();
    for record in selected.products {
        let product = record.product;
        if !regular_name(&product.saved_name) || content_object_digest(native_store, &product.selected).is_none() {
            continue;
        }
        if !fs::symlink_metadata(receipt.join(&product.saved_name)).is_ok_and(|metadata| {
            metadata.is_file()
                && metadata.dev() == product.device
                && metadata.ino() == product.inode
                && file_identity(&metadata) == record.saved_identity
        }) {
            continue;
        }
        if prior.insert(record.group, product).is_some() {
            return BTreeMap::new();
        }
    }
    prior
}

/// Resolve only the named CAS member, and admit it by the independent selected publication's digest.
fn read_previous(locator: &Path, native_store: &Path, incremental: &Path) -> Option<(StoredLink, PathBuf)> {
    let products = locator.parent()?;
    let request = products.parent()?;
    let products_metadata = fs::symlink_metadata(products).ok()?;
    let request_metadata = fs::symlink_metadata(request).ok()?;
    if products.file_name()? != "products"
        || !products_metadata.is_dir()
        || !request_metadata.is_dir()
        || request_metadata.mode() & 0o777 != 0o700
        || products_metadata.uid() != request_metadata.uid()
        || !request.file_name()?.to_str()?.starts_with("plan-admission-")
    {
        return None;
    }
    let selected: SelectedReceipt = serde_json::from_slice(&read_regular(locator, 16 * 1024)?).ok()?;
    let digest = selected.sha256.as_str();
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let expected = native_store.parent()?.join("payloads").join(&digest[..2]).join(digest);
    if selected.path != expected {
        return None;
    }
    let bytes = read_regular(&selected.path, PRODUCT_LIMIT)?;
    if hex::encode(Sha256::digest(&bytes)) != digest {
        return None;
    }
    let stored: StoredLink = serde_json::from_slice(&bytes).ok()?;
    if stored.format != FORMAT || !finalized_in(&stored.finalized, incremental) {
        return None;
    }
    let finalized = stored.finalized.clone();
    Some((stored, finalized))
}

/// Rustc uses a crate-specific child of the driver-owned incremental root.
fn finalized_in(finalized: &Path, incremental: &Path) -> bool {
    let Ok(root) = fs::canonicalize(incremental) else {
        return false;
    };
    finalized.parent().and_then(Path::parent) == Some(root.as_path())
        && fs::symlink_metadata(finalized).is_ok_and(|metadata| metadata.is_dir())
        && finalized
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("s-") && !name.ends_with("-working"))
}

/// Only a complete regular file from the same observed physical generation can supply optional evidence.
fn read_regular(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let before = file.metadata().ok()?;
    if !before.is_file() || before.len() > limit {
        return None;
    }
    let original = file_identity(&before);
    let mut bytes = Vec::new();
    (&mut file).take(limit + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 == before.len()
        && file_identity(&file.metadata().ok()?) == original
        && file_identity(&fs::symlink_metadata(path).ok()?) == original)
        .then_some(bytes)
}

/// A saved regular slot is exactly one filename and cannot rebase the finalized path.
fn regular_name(name: &str) -> bool {
    let path = Path::new(name);
    path.file_name().is_some_and(|file| file == name)
        && path.extension().is_some_and(|extension| extension == "o")
        && !name.ends_with(".asm.o")
}

/// After successful native output and its loader receipt, stage a candidate member in the same ordinary bake.
/// Only the later successful Oven selection can admit it for a future request.
fn store_result(receipt: &File, output: &Path, native_store: &Path, incremental: &Path) {
    let Some(stored) = linked_products(receipt, native_store, incremental) else {
        return;
    };
    let Some(parent) = output.parent() else { return };
    let Some(name) = output.file_name() else { return };
    let Ok(mut file) = tempfile::NamedTempFile::new_in(parent) else {
        return;
    };
    let mut writer = BufWriter::new(file.as_file_mut());
    if serde_json::to_writer(&mut writer, &stored).is_err() || writer.flush().is_err() {
        return;
    }
    drop(writer);
    let mut member = name.to_owned();
    member.push(".native-objects.json");
    let _ = file.persist(parent.join(member));
}

/// Recover only the actual successful link and bind its selected regular products to the cleaned finalized session.
fn linked_products(receipt: &File, native_store: &Path, incremental: &Path) -> Option<StoredLink> {
    let mut file = receipt.try_clone().ok()?;
    file.rewind().ok()?;
    if file.metadata().ok()?.len() > PRODUCT_LIMIT {
        return None;
    }
    let linked: LinkedObjects = serde_json::from_reader(file.take(PRODUCT_LIMIT)).ok()?;
    if !finalized_in(&linked.finalized, incremental) {
        return None;
    }
    let mut products = Vec::with_capacity(linked.entries.len());
    for entry in linked.entries {
        let complete = entry.completed;
        if !regular_name(&complete.saved_name) || content_object_digest(native_store, &entry.selected).is_none() {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(linked.finalized.join(&complete.saved_name)) else {
            continue;
        };
        if !metadata.is_file() || metadata.dev() != complete.device || metadata.ino() != complete.inode {
            continue;
        }
        products.push(StoredProduct {
            group: complete.group,
            product: PreviousObject {
                saved_name: complete.saved_name,
                selected: entry.selected,
                device: metadata.dev(),
                inode: metadata.ino(),
                length: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
            },
            saved_identity: file_identity(&metadata),
            restored: complete.cached_source.is_some(),
        });
    }
    if products.is_empty() {
        return None;
    }
    Some(StoredLink {
        format: FORMAT,
        finalized: linked.finalized,
        products,
    })
}
