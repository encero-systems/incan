use std::fs;
use std::sync::OnceLock;

use incan::frontend::ast::{Declaration, ImportKind};
use incan::frontend::{lexer, parser};

/// Derive codegen fixture ownership from the Incan entrypoint used to build the stdlib artifact.
pub(crate) fn artifact_module_paths() -> Vec<Vec<String>> {
    static MODULE_PATHS: OnceLock<Vec<Vec<String>>> = OnceLock::new();
    MODULE_PATHS
        .get_or_init(|| {
            let entrypoint =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib/src/lib.incn");
            let source = match fs::read_to_string(&entrypoint) {
                Ok(source) => source,
                Err(error) => panic!("failed to read {}: {error}", entrypoint.display()),
            };
            let tokens = match lexer::lex(&source) {
                Ok(tokens) => tokens,
                Err(errors) => panic!("stdlib entrypoint lexer failed: {errors:?}"),
            };
            let program = match parser::parse(&tokens) {
                Ok(program) => program,
                Err(errors) => panic!("stdlib entrypoint parser failed: {errors:?}"),
            };
            let mut paths = program
                .declarations
                .iter()
                .filter_map(|declaration| {
                    let Declaration::Import(import) = &declaration.node else {
                        return None;
                    };
                    let ImportKind::Module(path) = &import.kind else {
                        return None;
                    };
                    let mut segments = path.segments.clone();
                    if segments.len() > 1 && segments.last().map(String::as_str) == Some("prelude") {
                        segments.pop();
                    }
                    Some(segments)
                })
                .collect::<Vec<_>>();
            paths.sort();
            paths.dedup();
            paths
        })
        .clone()
}
