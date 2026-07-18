use std::sync::OnceLock;

/// Derive codegen fixture ownership from the Incan entrypoint used to build the stdlib artifact.
pub(crate) fn artifact_module_paths() -> Vec<Vec<String>> {
    static MODULE_PATHS: OnceLock<Vec<Vec<String>>> = OnceLock::new();
    MODULE_PATHS
        .get_or_init(|| {
            let mut paths = include_str!("../../crates/incan_stdlib/stdlib/src/lib.incn")
                .lines()
                .filter_map(|line| {
                    let imported_module = line.strip_prefix("import ")?;
                    let mut segments = imported_module.split('.').map(str::to_owned).collect::<Vec<_>>();
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
