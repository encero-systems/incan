//! Rust facts from rustc's own crate metadata (H9): the checker's view of the Rust items a unit names.
//!
//! One rustc session over a stub crate that links the unit's externs resolves each requested path through public
//! module children, as [`crate::callees`] resolves a callee, and describes the item in the checker's metadata shape: a
//! type with its generics, public inherent methods, fields and variants; a function's signature; a constant's type; a
//! trait's functions; a module's children. The nominal types a described signature names are described too, to
//! `depth` steps, so the checker can call on a call's result.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use incan_lang::interop::{
    RustAssociatedTypeBinding, RustFieldInfo, RustFunctionSig, RustImplementedTrait, RustItemKind, RustItemMetadata,
    RustMacroForm, RustMacroInfo, RustMethodSig, RustModuleChild, RustModuleChildKind, RustModuleInfo, RustParam,
    RustPayloadCarrier, RustTraitAssoc, RustTraitBound, RustTraitInfo, RustTypeInfo, RustTypeMetadataCompleteness,
    RustTypeShape, RustVariantInfo, RustVisibility,
};
use rustc_hir::def::{DefKind, MacroKinds};
use rustc_hir::def_id::DefId;
use rustc_interface::interface;
use rustc_middle::ty::{self, Ty, TyCtxt, TypeFoldable, TypeFolder};
use rustc_session::config::Input;
use rustc_span::{FileName, sym};

mod crate_discovery;
mod item_descriptions;

pub(crate) use crate_discovery::*;
use item_descriptions::*;

/// What one extraction reads: what to describe, and the crates and directories that resolve it.
pub struct RustFactsRequest<'a> {
    /// The pinned toolchain's sysroot, which supplies `std`, `core` and `alloc`.
    pub sysroot: &'a Path,
    /// The crates the extraction may name, as `--extern CRATE=RLIB` pairs.
    pub externs: &'a [(String, PathBuf)],
    /// The directories that resolve those crates' own dependencies.
    pub search: &'a [PathBuf],
    /// What to describe.
    pub scope: RustFactsScope,
}

/// What one extraction describes.
pub enum RustFactsScope {
    /// The named paths, and the nominal types their signatures name `depth` steps out: the paths a unit imports that
    /// no ABI cache covers.
    Paths { paths: BTreeSet<String>, depth: usize },
    /// Every public item of the named crates: their ABI cache.
    Crates(BTreeSet<String>),
}

impl RustFactsScope {
    /// The crates the extraction reads, which the stub names (`std`, `core` and `alloc` are always loaded).
    fn roots(&self) -> BTreeSet<String> {
        let names: Box<dyn Iterator<Item = &str>> = match self {
            Self::Paths { paths, .. } => {
                Box::new(paths.iter().filter_map(|path| crate::native_crates::callee_root(path)))
            }
            Self::Crates(crates) => Box::new(crates.iter().map(String::as_str)),
        };
        names
            .filter(|root| !matches!(*root, "std" | "core" | "alloc"))
            .map(str::to_owned)
            .collect()
    }
}

/// The compiler's rustc as a session's Rust facts source: the unit's own crates, read for the paths no ABI cache holds.
#[derive(Debug)]
pub struct RustcFacts {
    /// Exact native generation that identifies a bounded fallback extraction cache.
    pub unit: incan_driver::unit::UnitDescription,
    /// The pinned toolchain's sysroot, which supplies `std`, `core` and `alloc`.
    pub sysroot: PathBuf,
    /// The unit's own crates, as `--extern CRATE=RLIB` pairs.
    pub externs: Vec<(String, PathBuf)>,
    /// The directories that resolve those crates' own dependencies.
    pub search: Vec<PathBuf>,
}

impl incan_driver::session::RustFactsSource for RustcFacts {
    /// Reuse the exact bounded extraction for this request and native generation, after shipped ABI lookup.
    fn facts(&self, paths: &BTreeSet<String>) -> Result<BTreeMap<String, RustItemMetadata>, String> {
        incan_driver::rust_abi::cached_path_facts(&self.unit, paths, || self.extract_paths(paths))
            .map_err(|error| error.message)
    }
}

impl RustcFacts {
    /// Read uncovered paths from exactly the unit's declared native artifacts, retaining the existing depth bound
    /// and missing-artifact refusals. Persistent storage must never broaden this extraction's scope.
    fn extract_paths(&self, paths: &BTreeSet<String>) -> Result<BTreeMap<String, RustItemMetadata>, String> {
        let started = std::time::Instant::now();

        // ---- Linked native paths ----
        let (linked, unavailable): (BTreeSet<_>, BTreeSet<_>) = paths.iter().cloned().partition(|path| {
            let root = path.split("::").next().unwrap_or(path);
            matches!(root, "std" | "core" | "alloc") || self.externs.iter().any(|(name, _)| name == root)
        });
        let mut facts = extract(RustFactsRequest {
            sysroot: &self.sysroot,
            externs: &self.externs,
            search: &self.search,
            scope: RustFactsScope::Paths {
                paths: linked,
                depth: 1,
            },
        })?;
        // ---- Missing artifact diagnostics ----
        // Keep missing-artifact refusals at the imported item, so they do not mask independent source diagnostics.
        for path in unavailable {
            let root = path.split("::").next().unwrap_or(&path);
            let description = format!("Rust facts require an exact `{root}` artifact in the unit's native closure");
            facts.insert(
                path.clone(),
                RustItemMetadata {
                    canonical_path: path,
                    definition_path: None,
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Unsupported { description },
                },
            );
        }
        // ---- Optional extraction trace ----
        if incan_frontend::trace_flags::trace_rust_facts() {
            eprintln!(
                "rust facts: {} uncovered paths, {} items in {:?}",
                paths.len(),
                facts.len(),
                started.elapsed()
            );
        }
        Ok(facts)
    }
}

/// Describe what `request.scope` names, keyed by path.
pub fn extract(request: RustFactsRequest<'_>) -> Result<BTreeMap<String, RustItemMetadata>, String> {
    let roots = request.scope.roots();
    for root in &roots {
        let mut artifacts = request.externs.iter().filter(|(name, _)| name == root);
        let Some((_, artifact)) = artifacts.next() else {
            return Err(format!(
                "Rust facts require an exact `{root}` artifact in the unit's native closure"
            ));
        };
        if artifacts.any(|(_, other)| other != artifact) {
            return Err(format!("Rust facts received conflicting --extern artifacts for {root}"));
        }
    }
    let stub = roots
        .iter()
        .map(|root| format!("extern crate {root};\n"))
        .collect::<String>();
    let mut args = vec![
        "incan-rustc-driver".to_owned(),
        "--sysroot".to_owned(),
        request.sysroot.to_string_lossy().into_owned(),
        "--crate-name".to_owned(),
        "incan_rust_facts".to_owned(),
        "--crate-type".to_owned(),
        "lib".to_owned(),
        "--edition".to_owned(),
        "2024".to_owned(),
        "--cap-lints".to_owned(),
        "allow".to_owned(),
        "incan_rust_facts.rs".to_owned(),
    ];
    for (name, path) in request.externs {
        args.push("--extern".to_owned());
        args.push(format!("{name}={}", path.display()));
    }
    // Direct roots come only from --extern; directories locate their hash-qualified transitive dependencies.
    for directory in request.search {
        args.push("-L".to_owned());
        args.push(format!("dependency={}", directory.display()));
    }
    let mut callbacks = FactsCallbacks {
        stub,
        scope: request.scope,
        facts: BTreeMap::new(),
    };
    rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut callbacks))
        .map_err(|_| "rustc could not load the crates the Rust facts name".to_owned())?;
    Ok(callbacks.facts)
}

/// The stub crate session: install the stub source, describe the scope once the crates are loaded, then stop.
struct FactsCallbacks {
    stub: String,
    scope: RustFactsScope,
    facts: BTreeMap<String, RustItemMetadata>,
}

impl rustc_driver::Callbacks for FactsCallbacks {
    fn config(&mut self, config: &mut interface::Config) {
        config.input = Input::Str {
            name: FileName::Custom("incan_rust_facts".into()),
            input: self.stub.clone(),
        };
    }

    fn after_expansion(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        self.facts = match &self.scope {
            RustFactsScope::Paths { paths, depth } => describe_paths(tcx, paths, *depth),
            RustFactsScope::Crates(crates) => describe_crates(tcx, crates),
        };
        rustc_driver::Compilation::Stop
    }
}
