//! Native source-item products selected by Oven's source-independent semantic context.
//!
//! A module product contains independent checked bodies, not a replacement checking route. Its frontend owner
//! admits only scalar expressions with complete local-call witnesses and validates exact current locators. Imported
//! calls and other bodies fall back to ordinary source checking; a missing, corrupt or concurrently replaced product
//! is an ordinary miss.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use incan_driver::session::{CallableBodySource, CompilationSession};
use incan_driver::unit::{UnitDescription, UnitKind};
use incan_frontend::declaration_interface::{CallableBodyInputs, CallableBodyProducts, ValidatedCallableBodies};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::TypeChecker;
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{CompilerNodeId, module_identity_for_path, package_module_identity};
use sha2::{Digest, Sha256};

/// One invocation's authenticated store placement and accepted declaration inputs, without retaining checkers.
pub(super) struct CallableProducts {
    directory: Option<PathBuf>,
    package: String,
    project_root: PathBuf,
    inputs: BTreeMap<PathBuf, CallableBodyInputs>,
}

#[derive(Debug)]
struct StoredCallableBodies {
    directory: PathBuf,
    package: String,
    library: bool,
    project_root: PathBuf,
}

impl CallableBodySource for StoredCallableBodies {
    fn select(&self, module: &ParsedModule) -> Option<ValidatedCallableBodies> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::select_callable_products",
            file = %module.file_path.display())
        .entered();
        let path = self.directory.join(module_key(module));
        let mut file = std::fs::File::open(&path).ok()?;
        let limit = 64 * 1024 * 1024;
        let length = file.metadata().ok()?.len();
        if length > limit {
            return None;
        }
        let mut magic = [0; 4];
        file.read_exact(&mut magic).ok()?;
        if &magic != b"CRMB" {
            return None;
        }
        let mut bytes = Vec::with_capacity(length as usize);
        bytes.extend_from_slice(&magic);
        file.take(limit + 1 - 4).read_to_end(&mut bytes).ok()?;
        let source = project_source(&self.project_root, module)?;
        let owner = CompilerNodeId::module(if self.library {
            package_module_identity(&self.package, &module.path_segments)
        } else {
            module_identity_for_path(&module.path_segments)
        });
        let reused = decoded().get(&path, &bytes, &owner, &source);
        let products = match reused {
            Some(products) => {
                tracing::debug!(target: "incan::invocation", file = %module.file_path.display(),
                    "reused decoded callable body products");
                products
            }
            None => {
                let products = super::callable_crumb::decode(&bytes, &owner, &source).ok()?;
                decoded().insert(path, bytes, owner, source, products.clone());
                products
            }
        };
        let selection = products
            .select(&module.ast, &module.source, &self.package, &module.path_segments)
            .ok()?;
        if selection.is_empty() {
            return None;
        }
        tracing::debug!(target: "incan::invocation", file = %module.file_path.display(),
            retained_bodies = selection.len(), "selected callable body products");
        Some(selection)
    }
}

impl CallableProducts {
    /// Install the private scalar source when Oven supplied its authenticated compilation context.
    pub(super) fn prepare(unit: &UnitDescription, session: &mut CompilationSession) -> Self {
        let directory = product_directory(unit);
        let project_root = directory
            .as_ref()
            .and_then(|_| unit.project_root.canonicalize().ok())
            .unwrap_or_else(|| unit.project_root.clone());
        if let Some(directory) = &directory {
            session.set_callable_body_source(std::sync::Arc::new(StoredCallableBodies {
                directory: directory.clone(),
                package: unit.package.clone(),
                library: unit.kind == UnitKind::Library,
                project_root: project_root.clone(),
            }));
        }
        Self {
            directory,
            package: unit.package.clone(),
            project_root,
            inputs: BTreeMap::new(),
        }
    }

    /// Retain accepted inputs from the same checking pass used for publication. Unsupported source shapes simply
    /// decline capture, preserving their ordinary diagnostics and full compilation behavior.
    pub(super) fn observe(&mut self, module: &ParsedModule, checker: &TypeChecker) {
        if self.directory.is_none() {
            return;
        }
        match CallableBodyInputs::from_checked_for_unit(&module.ast, &module.source, checker, &self.package) {
            Ok(inputs) => {
                self.inputs.insert(module.file_path.clone(), inputs);
            }
            Err(reason) => tracing::debug!(target: "incan::invocation", file = %module.file_path.display(),
                %reason, "callable product capture refused"),
        }
    }

    /// Publish independently validated products from the actual completed Body IR pass. A cache write failure is
    /// observational only and cannot change source acceptance or substitute another native implementation.
    pub(super) fn publish(
        mut self,
        modules: &[ParsedModule],
        bodies: &[BodyIrModule],
    ) -> Vec<(incan_semantics_core::CompilerNodeId, String)> {
        let Some(directory) = self.directory else {
            return Vec::new();
        };
        let mut admitted = Vec::new();
        for (module, body) in modules.iter().zip(bodies) {
            let Some(inputs) = self.inputs.remove(&module.file_path) else {
                continue;
            };
            let result = inputs
                .finish(body)
                .map_err(|error| error.to_string())
                .and_then(|products| {
                    admitted.extend(products.native_inputs().map_err(|error| error.to_string())?);
                    let Some(source) = project_source(&self.project_root, module) else {
                        return Ok(());
                    };
                    let path = directory.join(module_key(module));
                    publish(&path, products, &body.module_id, &source)
                        .map_err(|error| format!("cannot publish callable products {}: {error}", path.display()))
                });
            if let Err(error) = result {
                tracing::debug!(target: "incan::invocation", %error, "callable products were not retained");
            }
        }
        admitted
    }
}

/// Use only Oven's explicit item directory and well-formed context digest. Library, program and test units share the
/// same source-independent semantic key; current module inventory and exact declaration source/provenance are still
/// independently checked by the frontend before any body can skip work.
fn product_directory(unit: &UnitDescription) -> Option<PathBuf> {
    let context = unit.checked_context.as_deref()?;
    if context.len() != 64 || !context.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    unit.checked_products
        .as_ref()
        .map(|directory| directory.join(incan_frontend::checker_records::GRAPH_BODY_RECORD_PROFILE))
}

/// Scope native function products to Oven's complete semantic generation and current native profile. Lowering
/// additionally keys the exact checked body, overflow policy and current diagnostic/frame locations.
pub(super) fn native_directory(unit: &UnitDescription) -> Option<String> {
    product_directory(unit).map(|directory| directory.join(&unit.profile).to_string_lossy().into_owned())
}

/// Bind the kernel owner frame to its project-relative source identity across relocations.
fn project_source(project: &Path, module: &ParsedModule) -> Option<String> {
    if let Ok(relative) = module.file_path.strip_prefix(project) {
        return relative.to_str().map(str::to_owned);
    }
    // Explicit source roots may be siblings of the project. Preserve their identity relative to this unit as well.
    let mut root = project.components().peekable();
    let mut file = module.file_path.components().peekable();
    if root.peek() != file.peek() {
        return None;
    }
    while root.peek().is_some() && root.peek() == file.peek() {
        root.next();
        file.next();
    }
    let mut path = PathBuf::new();
    for _ in root {
        path.push("..");
    }
    path.extend(file);
    path.to_str().map(str::to_owned)
}

/// Distinguish source modules through their complete producer path, without parsing any generated carrier name.
fn module_key(module: &ParsedModule) -> String {
    let mut digest = Sha256::new();
    for segment in &module.path_segments {
        digest.update(segment.len().to_le_bytes());
        digest.update(segment.as_bytes());
    }
    format!("{}.incnitems", hex::encode(digest.finalize()))
}

/// Verify the complete private transport before atomically replacing one module's independent item inventory.
/// A concurrent writer publishes another complete generation; every reader still validates its own source input.
fn publish(
    path: &std::path::Path,
    products: CallableBodyProducts,
    module: &incan_semantics_core::CompilerNodeId,
    source: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = super::callable_crumb::encode(&products, module, source)?;
    let restored = super::callable_crumb::decode(&bytes, module, source)?;
    if super::callable_crumb::encode(&restored, module, source)? != bytes {
        return Err("callable products lost facts in their private round trip".into());
    }
    std::fs::create_dir_all(path.parent().ok_or("callable products have no owning directory")?)?;
    let temporary = path.with_extension(format!("{}.pending", std::process::id()));
    let mut output = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    output.write_all(&bytes)?;
    output.flush()?;
    drop(output);
    std::fs::rename(temporary, path)?;
    decoded().insert(path.to_path_buf(), bytes, module.clone(), source.to_owned(), restored);
    Ok(())
}

/// Products this driver process decoded or published, by file. A resident driver's next edit reads the file its
/// previous request published, and the same bytes, module and source decode to the same products, so that decode is
/// skipped. Any other bytes, module or source decode again.
static DECODED: Mutex<DecodedProducts<CallableBodyProducts>> = Mutex::new(DecodedProducts::new());

fn decoded() -> MutexGuard<'static, DecodedProducts<CallableBodyProducts>> {
    DECODED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Decoded values by file, each with the exact inputs of its decode. Each record is bounded by `RECORD_BYTES` and the
/// number of files by `FILES`; the least recently stored file is dropped first.
struct DecodedProducts<T> {
    entries: Vec<(PathBuf, Decoded<T>)>,
}

struct Decoded<T> {
    bytes: Vec<u8>,
    module: CompilerNodeId,
    source: String,
    value: T,
}

impl<T: Clone> DecodedProducts<T> {
    const FILES: usize = 256;
    const RECORD_BYTES: usize = 4 * 1024 * 1024;

    const fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// The value decoded from exactly these bytes for this module and source, if this file's last decode was that.
    fn get(&self, path: &Path, bytes: &[u8], module: &CompilerNodeId, source: &str) -> Option<T> {
        self.entries
            .iter()
            .find(|(file, decoded)| {
                file == path && decoded.bytes == bytes && &decoded.module == module && decoded.source == source
            })
            .map(|(_, decoded)| decoded.value.clone())
    }

    /// Replace this file's record with the value decoded from `bytes`.
    fn insert(&mut self, path: PathBuf, bytes: Vec<u8>, module: CompilerNodeId, source: String, value: T) {
        self.entries.retain(|(file, _)| file != &path);
        if bytes.len() > Self::RECORD_BYTES {
            return;
        }
        if self.entries.len() >= Self::FILES {
            self.entries.remove(0);
        }
        self.entries.push((
            path,
            Decoded {
                bytes,
                module,
                source,
                value,
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decode_is_reused_only_for_the_same_file_bytes_module_and_source() {
        let mut decoded = DecodedProducts::new();
        let module = CompilerNodeId::module("hello");
        let path = PathBuf::from("/products/hello.incnitems");
        decoded.insert(
            path.clone(),
            b"CRMB one".to_vec(),
            module.clone(),
            "hello.incn".into(),
            "one".to_owned(),
        );
        assert_eq!(
            decoded.get(&path, b"CRMB one", &module, "hello.incn"),
            Some("one".to_owned())
        );
        assert_eq!(decoded.get(&path, b"CRMB two", &module, "hello.incn"), None);
        assert_eq!(
            decoded.get(&path, b"CRMB one", &CompilerNodeId::module("other"), "hello.incn"),
            None
        );
        assert_eq!(decoded.get(&path, b"CRMB one", &module, "other.incn"), None);
        assert_eq!(
            decoded.get(
                Path::new("/products/other.incnitems"),
                b"CRMB one",
                &module,
                "hello.incn"
            ),
            None
        );
        decoded.insert(
            path.clone(),
            b"CRMB two".to_vec(),
            module.clone(),
            "hello.incn".into(),
            "two".to_owned(),
        );
        assert_eq!(decoded.get(&path, b"CRMB one", &module, "hello.incn"), None);
        assert_eq!(
            decoded.get(&path, b"CRMB two", &module, "hello.incn"),
            Some("two".to_owned())
        );
        let large = vec![0; DecodedProducts::<String>::RECORD_BYTES + 1];
        decoded.insert(
            path.clone(),
            large.clone(),
            module.clone(),
            "hello.incn".into(),
            "large".to_owned(),
        );
        assert_eq!(decoded.get(&path, &large, &module, "hello.incn"), None);
        assert_eq!(decoded.get(&path, b"CRMB two", &module, "hello.incn"), None);
        for index in 0..=DecodedProducts::<String>::FILES {
            let file = PathBuf::from(format!("/products/{index}.incnitems"));
            decoded.insert(
                file,
                b"CRMB".to_vec(),
                module.clone(),
                "hello.incn".into(),
                index.to_string(),
            );
        }
        assert_eq!(decoded.entries.len(), DecodedProducts::<String>::FILES);
        assert_eq!(
            decoded.get(Path::new("/products/0.incnitems"), b"CRMB", &module, "hello.incn"),
            None
        );
        let last = PathBuf::from(format!("/products/{}.incnitems", DecodedProducts::<String>::FILES));
        let expected = Some(DecodedProducts::<String>::FILES.to_string());
        assert_eq!(decoded.get(&last, b"CRMB", &module, "hello.incn"), expected);
    }
}
