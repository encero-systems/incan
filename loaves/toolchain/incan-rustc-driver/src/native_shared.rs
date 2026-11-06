//! Derive immutable shared native libraries from exact rlibs for local development executables.
//!
//! The carrier reexports the original crate; it does not redeclare a type or change a checked callee identity. Rustc
//! selects dynamic dependency formats from the carrier's metadata. Every cached carrier retains witnesses for all
//! native metadata it loaded, including earlier shared libraries and the pinned standard library.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rustc_ast as ast;
use rustc_interface::interface;
use rustc_middle::ty::TyCtxt;
use rustc_session::config::Input;
use rustc_span::{FileName, Ident, sym};
use sha2::{Digest, Sha256};

use crate::adapter::DriverError;
use crate::identity::DriverIdentity;

type Inputs = Vec<(PathBuf, String)>;
type Record = (PathBuf, String, Inputs);
static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

/// One shared carrier and the exact native inputs whose implementations it already carries.
struct Library {
    path: PathBuf,
    inputs: Inputs,
}

/// The exact native recipe and the input generations that must survive a carrier's compilation.
struct Recipe {
    key: String,
    inputs: Inputs,
}

/// Prepare reached native roots in dependency-compatible order, retaining the runtime across source edits and
/// building a reached Loaf only when an earlier carrier does not already contain it. Rustc validates all crate
/// identities again when compiling the executable; no source acceptance or decoded type context is cached here.
pub(crate) fn libraries(
    roots: BTreeSet<&str>,
    externs: &[(String, PathBuf)],
    directories: &[PathBuf],
    identity: &DriverIdentity,
    store: &Path,
) -> Result<Vec<(String, PathBuf)>, DriverError> {
    // ---- Store and dependency-compatible root order ----
    if !store.is_absolute() {
        return Err(refusal("the shared native store must be absolute"));
    }
    fs::create_dir_all(store).map_err(io_error)?;
    let mut roots = roots.into_iter().collect::<Vec<_>>();
    roots.sort_by_key(|root| (*root != "incan_native_runtime", *root));
    let mut shared = Vec::new();
    let mut covered = BTreeSet::new();

    // ---- Reached native implementations ----
    for root in roots {
        let Some(artifact) = native_library(root, externs, identity)? else {
            continue;
        };
        let artifact = artifact.as_ref();
        if artifact.extension().is_none_or(|extension| extension != "rlib") || covered.contains(artifact) {
            continue;
        }
        let library = prepared(root, artifact, externs, directories, identity, store, &shared)?;
        covered.extend(library.inputs.iter().map(|(path, _)| path.clone()));
        shared.push((format!("incan_debug_shared_{}", shared.len()), library.path));
    }
    Ok(shared)
}

/// Select an explicit native input, or the pinned test-harness archive that rustc ordinarily injects implicitly.
fn native_library<'a>(
    root: &str,
    externs: &'a [(String, PathBuf)],
    identity: &DriverIdentity,
) -> Result<Option<Cow<'a, Path>>, DriverError> {
    if let Some((_, path)) = externs.iter().find(|(name, _)| name == root) {
        return Ok(Some(Cow::Borrowed(path.as_path())));
    }
    if root != "test" {
        return Ok(None);
    }
    let directory = identity.sysroot.join("lib/rustlib").join(&identity.target).join("lib");
    let mut candidates = Vec::new();
    for entry in fs::read_dir(directory).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("libtest-") && name.ends_with(".rlib"))
        {
            candidates.push(path);
        }
    }
    if candidates.len() != 1 {
        return Err(refusal("the pinned sysroot must name exactly one test-harness archive"));
    }
    Ok(candidates.pop().map(Cow::Owned))
}

/// Retain the exact generations of dynamic native inputs while rustc still owns its current crate graph.
pub(crate) fn loaded(tcx: TyCtxt<'_>) -> Result<Vec<(PathBuf, String)>, DriverError> {
    let mut paths = BTreeSet::new();
    for number in tcx.crates(()) {
        if let Some(path) = &tcx.used_crate_source(*number).dylib {
            paths.insert(path.clone());
        }
    }
    paths.iter().map(|path| observed(path)).collect()
}

/// Publish an executable's loader witnesses only after its dynamic inputs survived the entire native compilation.
/// Oven validates these generations on unchanged runs as well as edits, before it can execute the stored program.
pub(crate) fn write_witness(output: &Path, inputs: &[(PathBuf, String)]) -> Result<(), DriverError> {
    if inputs.is_empty()
        || !inputs
            .iter()
            .all(|(path, stamp)| crate::native_search::file_identity(path).as_ref() == Some(stamp))
    {
        return Err(refusal(
            "dynamic native inputs are missing or changed during compilation",
        ));
    }
    let libraries = inputs
        .iter()
        .map(|(path, stamp)| serde_json::json!({"path": path, "identity": stamp}))
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&serde_json::json!({"schema": "incan.native-shared/1", "libraries": libraries}))
        .map_err(|error| refusal(&error.to_string()))?;
    fs::write(format!("{}.native-shared.json", output.display()), bytes).map_err(io_error)
}

/// Serialize preparation for one exact root and earlier shared dependencies. A stale or corrupt witness causes a
/// fresh build; a failed build publishes no usable index and cannot authorize another request.
fn prepared(
    root: &str,
    artifact: &Path,
    externs: &[(String, PathBuf)],
    directories: &[PathBuf],
    identity: &DriverIdentity,
    store: &Path,
    shared: &[(String, PathBuf)],
) -> Result<Library, DriverError> {
    // ---- Exact recipe and publication lock ----
    let recipe = recipe(root, artifact, identity, shared)?;
    let index = store.join(format!("{}.json", recipe.key));
    let lock = File::options()
        .create(true)
        .read(true)
        .write(true)
        .open(store.join(format!("{}.lock", recipe.key)))
        .map_err(io_error)?;
    lock.lock().map_err(io_error)?;

    // ---- Completed native evidence ----
    if let Some(library) = cached(&index, &recipe) {
        return Ok(library);
    }

    // ---- New shared library ----
    let number = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
    let staging = store.join(format!(".staged-{}-{number}", std::process::id()));
    fs::create_dir(&staging).map_err(io_error)?;
    let built = build(root, recipe, externs, directories, identity, &staging, shared);
    let result = built.and_then(|library| publish(store, &index, library));
    // This directory and every file in it belong to this invocation; no completed store entry is removed here.
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Bind a carrier recipe to its exact input generation, pinned compiler, target, SDK and preceding carriers.
fn recipe(
    root: &str,
    artifact: &Path,
    identity: &DriverIdentity,
    shared: &[(String, PathBuf)],
) -> Result<Recipe, DriverError> {
    // ---- Pinned compiler and exact native inputs ----
    let executable = std::env::current_exe().map_err(io_error)?;
    let mut inputs = vec![observed(artifact)?, observed(&executable)?];
    for (_, path) in shared {
        inputs.push(observed(path)?);
    }

    // ---- Selected SDK ----
    let sdk = std::env::var("SDKROOT").unwrap_or_default();
    if !sdk.is_empty() {
        inputs.push(observed(Path::new(&sdk))?);
        if let Some(parent) = Path::new(&sdk).parent() {
            inputs.push(observed(parent)?);
        }
        let settings = Path::new(&sdk).join("SDKSettings.json");
        if settings.is_file() {
            inputs.push(observed(&settings)?);
        }
    }

    // ---- Target and inherited native link configuration ----
    let environment = ["MACOSX_DEPLOYMENT_TARGET", "DEVELOPER_DIR", "LIBRARY_PATH", "PATH"]
        .map(|name| (name, std::env::var_os(name)));
    let description = format!(
        "incan.native-shared/1\n{root}\n{}\n{}\n{}\n{sdk}\n{environment:?}\n{inputs:?}",
        identity.sysroot.display(),
        identity.toolchain,
        identity.target
    );
    Ok(Recipe {
        key: hex::encode(Sha256::digest(description.as_bytes())),
        inputs,
    })
}

/// Reuse only a completed carrier and transitive native inputs whose complete filesystem generations still match.
fn cached(index: &Path, recipe: &Recipe) -> Option<Library> {
    let (path, stamp, inputs): Record = serde_json::from_slice(&fs::read(index).ok()?).ok()?;
    let expected = format!("libincan_debug_shared_{}", &recipe.key[..16]);
    if path.file_stem()?.to_str()? != expected || path.parent()?.parent()? != index.parent()? {
        return None;
    }
    if !recipe.inputs.iter().all(|input| inputs.contains(input)) {
        return None;
    }
    if observed(&path).ok()?.1 != stamp || !unchanged(&inputs) {
        return None;
    }
    Some(Library { path, inputs })
}

/// A shared carrier has one public native root; earlier carriers enter as ordinary extern crates for rustc's
/// dependency-format selection. Its declarations are AST nodes, never generated Rust source.
struct Carrier {
    root: String,
    shared: Vec<String>,
    search: Vec<rustc_session::search_paths::SearchPath>,
    inputs: Inputs,
}

impl rustc_driver::Callbacks for Carrier {
    /// Install a source-free crate and current native search indexes. Only the implicit test carrier receives the
    /// pinned `test` feature; authored Incan and Rust compilation retain their ordinary feature permissions.
    fn config(&mut self, config: &mut interface::Config) {
        config.input = Input::Str {
            name: FileName::Custom("incan-shared-native".into()),
            input: String::new(),
        };
        config.opts.search_paths = std::mem::take(&mut self.search);
        if self.root == "test" {
            // Only this source-free compiler-owned carrier imports libtest. No authored AST or source receives it.
            config.opts.unstable_features = rustc_feature::UnstableFeatures::Allow;
        }
    }

    /// Reexport the original root and load earlier shared carriers without copying any native declarations.
    fn after_crate_root_parsing(
        &mut self,
        compiler: &interface::Compiler,
        krate: &mut ast::Crate,
    ) -> rustc_driver::Compilation {
        let span = krate.spans.inner_span;
        if self.root == "test" {
            krate.attrs.push(ast::attr::mk_attr_nested_word(
                &compiler.sess.psess.attr_id_generator,
                ast::AttrStyle::Inner,
                ast::Safety::Default,
                sym::feature,
                sym::test,
                span,
            ));
        }
        let mut item = crate::declarations::external_crate(&self.root, span);
        item.kind = ast::ItemKind::ExternCrate(None, Ident::from_str_and_span(&self.root, span));
        item.vis.kind = ast::VisibilityKind::Public;
        krate.items.push(item);
        for name in &self.shared {
            krate.items.push(crate::declarations::external_crate(name, span));
        }
        rustc_driver::Compilation::Continue
    }

    /// Record actual metadata sources chosen by rustc, including transitive archives and shared dependencies.
    fn after_expansion(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        let mut paths = BTreeSet::new();
        for number in tcx.crates(()) {
            let source = tcx.used_crate_source(*number);
            paths.extend(source.paths().cloned());
        }
        for path in paths {
            match observed(&path) {
                Ok(input) => self.inputs.push(input),
                Err(error) => tcx.dcx().fatal(error.to_string()),
            }
        }
        rustc_driver::Compilation::Continue
    }
}

/// Compile one native carrier with ordinary rustc dynamic linking, against exact existing rlibs and prior carriers.
fn build(
    root: &str,
    recipe: Recipe,
    externs: &[(String, PathBuf)],
    directories: &[PathBuf],
    identity: &DriverIdentity,
    staging: &Path,
    shared: &[(String, PathBuf)],
) -> Result<Library, DriverError> {
    // ---- Source-free shared-library invocation ----
    let name = format!("incan_debug_shared_{}", &recipe.key[..16]);
    let extension = if identity.target.contains("apple-darwin") {
        "dylib"
    } else {
        "so"
    };
    let output = staging.join(format!("lib{name}.{extension}"));
    let mut args = vec![
        "incan-rustc-driver".into(),
        "--sysroot".into(),
        identity.sysroot.display().to_string(),
        "--crate-name".into(),
        name,
        "--crate-type=dylib".into(),
        "--edition=2024".into(),
        "-Cprefer-dynamic".into(),
        "-Crpath".into(),
        "-Cdebuginfo=0".into(),
        "--cap-lints=allow".into(),
        "-o".into(),
        output.display().to_string(),
        "incan-shared-native".into(),
    ];

    // ---- Existing implementations and preceding shared carriers ----
    let all = externs.iter().chain(shared).cloned().collect::<Vec<_>>();
    for (name, path) in &all {
        args.push(format!("--extern={name}={}", path.display()));
    }
    let mut callbacks = Carrier {
        root: root.into(),
        shared: shared.iter().map(|(name, _)| name.clone()).collect(),
        search: crate::native_search::dependencies(&all, directories),
        inputs: recipe.inputs,
    };

    // ---- Native compilation and input stability ----
    rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut callbacks))
        .map_err(|_| DriverError::Compilation)?;
    if !unchanged(&callbacks.inputs) {
        return Err(refusal("native inputs changed while building a shared library"));
    }
    Ok(Library {
        path: output,
        inputs: callbacks.inputs,
    })
}

/// Publish by the completed library's content digest, then atomically install its generation-guarded index.
fn publish(store: &Path, index: &Path, library: Library) -> Result<Library, DriverError> {
    // ---- Content-addressed native output ----
    let bytes = fs::read(&library.path).map_err(io_error)?;
    let digest = hex::encode(Sha256::digest(&bytes));
    let directory = store.join(&digest);
    fs::create_dir_all(&directory).map_err(io_error)?;
    let name = library
        .path
        .file_name()
        .ok_or_else(|| refusal("shared library has no filename"))?;
    let path = directory.join(name);
    if path.exists() {
        if fs::read(&path).map_err(io_error)? != bytes {
            // A content-addressed cache entry is regenerable: atomically repair exactly these verified bytes.
            fs::rename(&library.path, &path).map_err(io_error)?;
        }
    } else {
        fs::rename(&library.path, &path).map_err(io_error)?;
    }

    // ---- Completed recipe witness ----
    let (_, stamp) = observed(&path)?;
    let record = (path.clone(), stamp, &library.inputs);
    let bytes = serde_json::to_vec(&record).map_err(|error| refusal(&error.to_string()))?;
    let temporary = index.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, bytes).map_err(io_error)?;
    fs::rename(temporary, index).map_err(io_error)?;
    Ok(Library {
        path,
        inputs: library.inputs,
    })
}

/// Observe each followed native file; unsupported filesystem observations never authorize reuse.
fn observed(path: &Path) -> Result<(PathBuf, String), DriverError> {
    let stamp = crate::native_search::file_identity(path)
        .ok_or_else(|| refusal(&format!("cannot observe native input {}", path.display())))?;
    Ok((path.to_owned(), stamp))
}

/// Reject changed or missing native generations before or after a carrier build.
fn unchanged(inputs: &Inputs) -> bool {
    inputs
        .iter()
        .all(|(path, stamp)| crate::native_search::file_identity(path).as_ref() == Some(stamp))
}

/// Preserve a native-store failure at the adapter's typed invocation boundary.
fn io_error(error: std::io::Error) -> DriverError {
    refusal(&format!("shared native store: {error}"))
}

/// Name invalid store evidence without disguising it as a source-language diagnostic.
fn refusal(message: &str) -> DriverError {
    DriverError::Invocation(message.into())
}
