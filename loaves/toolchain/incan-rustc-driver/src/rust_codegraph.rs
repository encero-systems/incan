//! Observe a Rust unit after rustc analysis, using the exact invocation Oven selected, without running codegen.

use incan_codegraph::{
    CODEGRAPH_SCHEMA_VERSION, CodegraphHeaderRecord, CodegraphLanguage, CodegraphMode, CodegraphRecord,
};
use rustc_interface::interface;
use rustc_middle::ty::TyCtxt;
use rustc_middle::ty::print::{with_no_trimmed_paths, with_no_visible_paths, with_resolve_crate_name};

/// Analyze a Rust invocation and publish only a complete checked graph. Failed analysis never creates a cache entry.
pub fn export(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [output, root, rustc_arguments @ ..] = arguments else {
        return Err("usage: --rust-codegraph OUTPUT ROOT RUSTC_ARGS...".into());
    };
    let mut args = vec!["incan-rustc-driver".to_owned()];
    args.extend_from_slice(rustc_arguments);
    let mut callbacks = Observer {
        root: root.clone(),
        records: Vec::new(),
    };
    rustc_driver::catch_fatal_errors(|| rustc_driver::run_compiler(&args, &mut callbacks))
        .map_err(|_| "Rust analysis failed; no graph was published")?;
    if callbacks.records.is_empty() {
        return Err("Rust analysis produced no graph".into());
    }
    std::fs::write(output, incan_codegraph::to_jsonl(&callbacks.records)?)?;
    Ok(())
}

/// Retain records outside rustc's arena and stop before MIR code generation.
struct Observer {
    root: String,
    records: Vec<CodegraphRecord>,
}

impl rustc_driver::Callbacks for Observer {
    /// Extract only after name resolution and type checking succeeded; method targets come from typeck results.
    fn after_analysis(&mut self, _compiler: &interface::Compiler, tcx: TyCtxt<'_>) -> rustc_driver::Compilation {
        self.records.push(CodegraphRecord::Header(CodegraphHeaderRecord {
            schema_version: CODEGRAPH_SCHEMA_VERSION,
            compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
            mode: CodegraphMode::Strict,
            root_path: self.root.clone(),
            languages: vec![CodegraphLanguage::Rust],
            package: None,
            semantic_contexts: Vec::new(),
            degraded: false,
        }));
        // Identities must print identically in a defining crate and a consumer, regardless of reexports or aliases.
        with_resolve_crate_name!(with_no_trimmed_paths!(with_no_visible_paths!({
            crate::rust_graph_records::declarations(tcx, &mut self.records);
            crate::rust_graph_bodies::references(tcx, &mut self.records);
        })));
        rustc_driver::Compilation::Stop
    }
}
