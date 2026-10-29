//! Observe a Rust unit after rustc analysis using Oven's exact invocation. Inspection stops before codegen;
//! compilation continues and publishes the same facts beside the native Loaf.

use incan_codegraph::{
    CODEGRAPH_SCHEMA_VERSION, CodegraphHeaderRecord, CodegraphLanguage, CodegraphMode, CodegraphPackage,
    CodegraphRecord,
};
use rustc_interface::interface;
use rustc_middle::ty::TyCtxt;
use rustc_middle::ty::print::{with_no_trimmed_paths, with_no_visible_paths, with_resolve_crate_name};

/// Analyze a Rust invocation and publish only a complete checked graph. Failed analysis never creates a cache entry.
pub fn export(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    run(arguments, false)
}

/// Capture the same checked Rust facts while continuing through the unit's ordinary native compilation.
pub fn compile(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    run(arguments, true)
}

/// One observer serves inspection and compilation; the latter publishes a graph only after code generation succeeds.
fn run(arguments: &[String], codegen: bool) -> Result<(), Box<dyn std::error::Error>> {
    let [output, root, rustc_arguments @ ..] = arguments else {
        return Err("usage: --rust-codegraph OUTPUT ROOT RUSTC_ARGS...".into());
    };
    let mut args = vec!["incan-rustc-driver".to_owned()];
    args.extend_from_slice(rustc_arguments);
    let mut callbacks = Observer {
        root: root.clone(),
        records: Vec::new(),
        codegen,
    };
    rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut callbacks))
        .map_err(|_| "Rust analysis failed; no graph was published")?;
    if callbacks.records.is_empty() {
        return Err("Rust analysis produced no graph".into());
    }
    std::fs::write(
        output,
        incan_codegraph::artifact::encode_graph(&callbacks.records, std::path::Path::new(root))?,
    )?;
    Ok(())
}

/// Retain records outside rustc's arena; the caller decides whether this invocation continues through codegen.
struct Observer {
    root: String,
    records: Vec<CodegraphRecord>,
    /// Compilation continues only for an ordinary unit build; inspection remains analysis-only.
    codegen: bool,
}

impl rustc_driver::Callbacks for Observer {
    /// Give hand-written Rust invocations the same opt-in phase diagnostics as Incan's native boundary.
    fn config(&mut self, config: &mut interface::Config) {
        crate::native_timing::Timing::requested().configure(config);
    }

    /// Extract only after name resolution and type checking succeeded; method targets come from typeck results.
    fn after_analysis(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        self.records.push(CodegraphRecord::Header(CodegraphHeaderRecord {
            schema_version: CODEGRAPH_SCHEMA_VERSION,
            compiler_version: incan_lang::version::INCAN_VERSION.to_owned(),
            mode: CodegraphMode::Strict,
            root_path: self.root.clone(),
            languages: vec![CodegraphLanguage::Rust],
            package: Some(CodegraphPackage {
                name: std::env::var("CARGO_PKG_NAME")
                    .ok()
                    .or_else(|| Some(tcx.crate_name(rustc_hir::def_id::LOCAL_CRATE).to_string())),
                version: std::env::var("CARGO_PKG_VERSION").ok(),
                root_path: Some(self.root.clone()),
            }),
            semantic_contexts: Vec::new(),
            degraded: false,
        }));
        // Identities must print identically in a defining crate and a consumer, regardless of reexports or aliases.
        with_resolve_crate_name!(with_no_trimmed_paths!(with_no_visible_paths!({
            crate::rust_graph_records::declarations(tcx, &mut self.records);
            crate::rust_graph_bodies::references(tcx, &mut self.records);
        })));
        if self.codegen {
            rustc_driver::Compilation::Continue
        } else {
            rustc_driver::Compilation::Stop
        }
    }
}
