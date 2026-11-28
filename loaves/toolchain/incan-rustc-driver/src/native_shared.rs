//! Derive immutable shared native libraries from exact rlibs for local development executables.
//!
//! The carrier reexports the original crate; it does not redeclare a type or change a checked callee identity. Rustc
//! selects dynamic dependency formats from the carrier's metadata. Every cached carrier retains witnesses for all
//! native metadata it loaded, including earlier shared libraries and the pinned standard library.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

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
/// Content identities already proved for an exact filesystem generation in this resident compiler worker.
static CONTENT_IDENTITIES: LazyLock<Mutex<HashMap<(PathBuf, String), String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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

/// What the caller's installation supplies to development carriers. Oven selects it from the home and the toolchain
/// manifest (`--installed-native`, `--installed-home`, `--installed-extern`, `--link-tool`); this driver only applies
/// it, because a carrier's recipe and index name the inputs rustc reports while the carrier compiles in this process.
#[derive(Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct InstalledNative {
    /// The home's carrier store. Without it, only the pinned test harness gets a carrier, in the development store.
    pub(crate) store: Option<PathBuf>,
    /// The directory an index names files relative to, so a moved home reads its carriers.
    pub(crate) home: Option<PathBuf>,
    /// The crates the toolchain manifest installs.
    pub(crate) externs: BTreeSet<String>,
    /// Programs rustc runs by search path for a carrier's link, as that invocation selects them.
    pub(crate) tools: Vec<PathBuf>,
}

/// Prepare reached installed native roots in dependency-compatible order, building a reached Loaf only when an
/// earlier carrier does not already contain it. Only the crates the installation provides and the pinned test harness
/// get a carrier, and the runtime and the harness come first, so every executable of one installation shares their
/// carriers whatever else it reaches and whatever its sources say. An authored library changes with its sources and
/// links into the executable, which is linked again anyway, instead of making one more new shared library each edit.
/// On Darwin a link without a selected SDK leaves rustc to discover one, which no carrier key can name, so such an
/// executable links its natives statically. Rustc validates all crate identities again when compiling the
/// executable; no source acceptance or decoded type context is cached here.
pub(crate) fn libraries(
    roots: BTreeSet<&str>,
    externs: &[(String, PathBuf)],
    directories: &[PathBuf],
    identity: &DriverIdentity,
    store: &Path,
    installed: &InstalledNative,
) -> Result<Vec<(String, PathBuf)>, DriverError> {
    // ---- Carrier store, selected SDK and dependency-compatible root order ----
    let carriers = installed.store.as_deref().unwrap_or(store);
    if !store.is_absolute() || !carriers.is_absolute() {
        return Err(refusal("the shared native store must be absolute"));
    }
    if identity.target.ends_with("-apple-darwin") && std::env::var_os("SDKROOT").is_none_or(|sdk| sdk.is_empty()) {
        return Ok(Vec::new());
    }
    fs::create_dir_all(carriers).map_err(io_error)?;
    let mut roots = roots.into_iter().collect::<Vec<_>>();
    roots.sort_by_key(|root| (shared_rank(root), *root));
    let mut shared = Vec::new();
    let mut covered = BTreeSet::new();

    // ---- Reached installed native implementations ----
    for root in roots {
        let Some(artifact) = native_library(root, externs, identity)? else {
            continue;
        };
        let artifact = artifact.as_ref();
        if artifact.extension().is_none_or(|extension| extension != "rlib")
            || covered.contains(artifact)
            || (root != "test" && !installed.externs.contains(root))
        {
            continue;
        }
        let library = prepared(
            root,
            artifact,
            externs,
            directories,
            identity,
            carriers,
            installed,
            &shared,
        )?;
        covered.extend(library.inputs.iter().map(|(path, _)| path.clone()));
        shared.push((format!("incan_debug_shared_{}", shared.len()), library.path));
    }
    Ok(shared)
}

/// Carrier order: the runtime, then the pinned test harness, then installed Loaves by name.
fn shared_rank(root: &str) -> u8 {
    match root {
        "incan_native_runtime" => 0,
        "test" => 1,
        _ => 2,
    }
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
    installed: &InstalledNative,
    shared: &[(String, PathBuf)],
) -> Result<Library, DriverError> {
    // ---- Exact recipe and publication lock ----
    let home = installed.home.as_deref();
    let recipe = recipe(root, artifact, identity, store, installed, shared)?;
    let index = store.join(format!("{}.json", recipe.key));
    let lock = File::options()
        .create(true)
        .read(true)
        .write(true)
        .open(store.join(format!("{}.lock", recipe.key)))
        .map_err(io_error)?;
    lock.lock().map_err(io_error)?;

    // ---- Completed native evidence ----
    if let Some(library) = cached(&index, &recipe, home) {
        return Ok(library);
    }

    // ---- New shared library ----
    let number = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
    let staging = store.join(format!(".staged-{}-{number}", std::process::id()));
    fs::create_dir(&staging).map_err(io_error)?;
    let built = build(root, recipe, externs, directories, identity, store, &staging, shared);
    let result = built.and_then(|library| publish(store, &index, library, home));
    // This directory and every file in it belong to this invocation; no completed store entry is removed here.
    let _ = fs::remove_dir_all(&staging);
    result
}

/// Bind a carrier recipe to its exact input generation, pinned compiler, target, SDK and preceding carriers.
fn recipe(
    root: &str,
    artifact: &Path,
    identity: &DriverIdentity,
    store: &Path,
    installed: &InstalledNative,
    shared: &[(String, PathBuf)],
) -> Result<Recipe, DriverError> {
    // ---- Pinned compiler and exact native inputs ----
    let executable = std::env::current_exe().map_err(io_error)?;
    let artifact_input = observed(artifact)?;
    let artifact_content = content_identity(store, &artifact_input)?;
    let executable_input = observed(&executable)?;
    let executable_content = content_identity(store, &executable_input)?;
    let mut inputs = vec![artifact_input, executable_input];
    let mut content = vec![artifact_content, executable_content];
    let mut system = Vec::new();
    let mut system_content = Vec::new();
    for (_, path) in shared {
        let input = observed(path)?;
        content.push(content_identity(store, &input)?);
        inputs.push(input);
    }

    // ---- Selected SDK, named by the directory it resolves to ----
    let sdk = std::env::var("SDKROOT").unwrap_or_default();
    let resolved = if sdk.is_empty() {
        PathBuf::new()
    } else {
        fs::canonicalize(&sdk).map_err(io_error)?
    };
    if !sdk.is_empty() {
        let sdk_input = observed(&resolved)?;
        inputs.push(sdk_input.clone());
        system.push(sdk_input);
        if let Some(parent) = resolved.parent() {
            let parent_input = observed(parent)?;
            inputs.push(parent_input.clone());
            system.push(parent_input);
        }
        let settings = resolved.join("SDKSettings.json");
        if settings.is_file() {
            let input = observed(&settings)?;
            system_content.push(content_identity(store, &input)?);
            inputs.push(input);
        }
    }

    // ---- Selected link tools, target and inherited native link configuration ----
    let mut tools = Vec::new();
    for tool in link_tools(&sdk, &identity.target, installed)? {
        let input = observed(&tool)?;
        tools.push(content_identity(store, &input)?);
        inputs.push(input);
    }
    let environment = [
        "MACOSX_DEPLOYMENT_TARGET",
        "DEVELOPER_DIR",
        "LIBRARY_PATH",
        "TOOLCHAINS",
    ]
    .map(|name| (name, std::env::var_os(name)));
    let description = format!(
        "incan.native-shared/3\n{root}\n{}\n{}\n{}\n{}\n{environment:?}\n{content:?}\n{tools:?}\n{system:?}\n{system_content:?}",
        identity.sysroot.display(),
        identity.toolchain,
        identity.target,
        resolved.display()
    );
    Ok(Recipe {
        key: hex::encode(Sha256::digest(description.as_bytes())),
        inputs,
    })
}

/// The programs a carrier's link runs, as that link selects them, so the caller's raw search path is not itself an
/// input. On Darwin they are the development linker beside this driver, which rustc runs by path, and the `ld` the
/// stable linker selects for the SDK. Elsewhere rustc runs the search path's `cc`, which Oven names as an invocation
/// selects it.
fn link_tools(sdk: &str, target: &str, installed: &InstalledNative) -> Result<Vec<PathBuf>, DriverError> {
    if !target.ends_with("-apple-darwin") {
        return Ok(installed.tools.clone());
    }
    let selection = vec!["-syslibroot".to_owned(), sdk.to_owned()];
    Ok(vec![
        crate::adapter::development_linker().map_err(io_error)?,
        crate::adapter::stable_link::linker(&selection, std::env::var_os("TOOLCHAINS").is_some()),
    ])
}

/// Reuse only a completed carrier and transitive native inputs whose complete filesystem generations still match.
fn cached(index: &Path, recipe: &Recipe, home: Option<&Path>) -> Option<Library> {
    let (path, stamp, inputs): Record = serde_json::from_slice(&fs::read(index).ok()?).ok()?;
    let path = located(&path, home);
    let inputs = inputs
        .iter()
        .map(|(input, generation)| (located(input, home), generation.clone()))
        .collect::<Inputs>();
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
    store: &Path,
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
    if identity.target.ends_with("-apple-darwin") {
        let linker = crate::adapter::development_linker().map_err(io_error)?;
        args.push(format!("-Clinker={}", linker.display()));
        args.push("-Clinker-flavor=ld".into());
        args.push(format!(
            "-Clink-arg={}{}",
            crate::adapter::stable_link::STORE_ARGUMENT,
            store.display()
        ));
    }

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
fn publish(store: &Path, index: &Path, library: Library, home: Option<&Path>) -> Result<Library, DriverError> {
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
    let inputs = library
        .inputs
        .iter()
        .map(|(input, generation)| (recorded(input, home), generation.clone()))
        .collect::<Inputs>();
    let record = (recorded(&path, home), stamp, inputs);
    let bytes = serde_json::to_vec(&record).map_err(|error| refusal(&error.to_string()))?;
    let temporary = index.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, bytes).map_err(io_error)?;
    fs::rename(temporary, index).map_err(io_error)?;
    Ok(Library {
        path,
        inputs: library.inputs,
    })
}

/// The prefix an index gives a file of the home, so the home's carriers are read again, not derived again, after the
/// home moves: a move keeps every file's generation and changes only the home's path.
const HOME_RELATIVE: &str = "$INCAN_HOME";

/// Name a file of the home relative to it in an index; any other file keeps its absolute path.
fn recorded(path: &Path, home: Option<&Path>) -> PathBuf {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => Path::new(HOME_RELATIVE).join(rest),
        None => path.to_path_buf(),
    }
}

/// Resolve an index's file against the current home.
fn located(path: &Path, home: Option<&Path>) -> PathBuf {
    match (home, path.strip_prefix(HOME_RELATIVE)) {
        (Some(home), Ok(rest)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// Observe each followed native file; unsupported filesystem observations never authorize reuse.
fn observed(path: &Path) -> Result<(PathBuf, String), DriverError> {
    let stamp = crate::native_search::file_identity(path)
        .ok_or_else(|| refusal(&format!("cannot observe native input {}", path.display())))?;
    Ok((path.to_owned(), stamp))
}

/// Name a file input only by bytes for a reproducible carrier recipe, reading an exact filesystem generation once
/// per file identity: the native store records each digest with the file's identity, so a fresh worker reads nothing
/// the store already named, and a resident worker answers from memory. The generation witness remains the cache key
/// and is checked again after a read.
fn content_identity(store: &Path, input: &(PathBuf, String)) -> Result<String, DriverError> {
    let mut identities = CONTENT_IDENTITIES.lock().map_err(|_| DriverError::State)?;
    if let Some(identity) = identities.get(input) {
        return Ok(identity.to_owned());
    }
    let identity =
        crate::adapter::stable_link::identified_digest(store, &input.0, native_input_digest).map_err(io_error)?;
    if observed(&input.0)? != *input {
        return Err(refusal("native input changed while computing its content identity"));
    }
    identities.insert(input.to_owned(), identity.to_owned());
    Ok(identity)
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

/// Hash one carrier input, as an invocation span a test can count: a worker's every read of a native input's bytes.
fn native_input_digest(path: &Path) -> std::io::Result<String> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_rustc_driver::native_input_digest", input = %path.display()).entered();
    crate::adapter::stable_link::file_digest(path)
}

#[cfg(test)]
mod relocation_tests {
    use super::*;

    /// A carrier published in one home is read, not derived again, after that home moves to another path.
    #[test]
    fn a_moved_home_reads_its_carriers() -> Result<(), Box<dyn std::error::Error>> {
        let scratch = tempfile::tempdir()?;
        let installed = scratch.path().join("install-home");
        let input = installed.join("toolchains/0.6.0/native/libincan_native_runtime.rlib");
        let staged = installed.join("cache/native-shared/.staged");
        fs::create_dir_all(input.parent().ok_or("input has no directory")?)?;
        fs::create_dir_all(&staged)?;
        fs::write(&input, b"runtime")?;
        let key = "0".repeat(64);
        let name = format!("libincan_debug_shared_{}.dylib", &key[..16]);
        fs::write(staged.join(&name), b"carrier")?;
        let store = installed.join("cache/native-shared");
        let index = store.join(format!("{key}.json"));
        let library = Library {
            path: staged.join(&name),
            inputs: vec![observed(&input)?],
        };
        publish(&store, &index, library, Some(&installed))?;
        assert!(!fs::read_to_string(&index)?.contains(installed.to_str().ok_or("home is not text")?));

        let moved = scratch.path().join("home");
        fs::rename(&installed, &moved)?;
        let recipe = Recipe {
            key,
            inputs: vec![observed(
                &moved.join("toolchains/0.6.0/native/libincan_native_runtime.rlib"),
            )?],
        };
        let store = moved.join("cache/native-shared");
        let read = cached(&store.join(format!("{}.json", recipe.key)), &recipe, Some(&moved))
            .ok_or("carrier derived again")?;
        assert!(read.path.starts_with(&store) && read.path.ends_with(&name));
        Ok(())
    }
}
