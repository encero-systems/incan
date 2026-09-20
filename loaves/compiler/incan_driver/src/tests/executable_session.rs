//! Executable resolution through a compilation session: the resolution test that needs the driver's session.

use std::error::Error;
use std::fs;

use crate::backend::replacement::ReplacementExecutionGraph;
use incan_frontend::body_ir::build_body_ir_module_v0;

/// Child frames share the complete checked graph, including a canonical backedge into its entry module.
#[test]
fn canonical_frames_reenter_the_entry_module_through_a_checked_cycle() -> Result<(), Box<dyn Error>> {
    use crate::modules::collect_modules_detailed_with_session;
    use crate::session::{CompilationSession, scoped_compilation_session_analysis_invocations};

    let temporary = tempfile::tempdir()?;
    let project = temporary.path().join("real");
    fs::create_dir_all(project.join("src"))?;
    fs::write(project.join("loaf.toml"), "[project]\nname = \"frame_cycle\"\n")?;
    fs::write(
        project.join("src/main.incn"),
        "from helper import bounce\n\npub def step(value: int) -> int:\n    if value == 0:\n        return 42\n    return bounce(value - 1)\n\ndef main() -> int:\n    return step(4)\n",
    )?;
    // The entry is deliberately spelled through a symlink rather than canonically. The import cycle reaches the
    // entry back under its canonical path, and collection keying files by spelling collected it twice, which the
    // graph then refused as a duplicate module identity (#1557). Windows cannot create the symlink without
    // privileges, so there the entry keeps its plain spelling.
    #[cfg(unix)]
    let entry_root = {
        let linked = temporary.path().join("linked");
        std::os::unix::fs::symlink(&project, &linked)?;
        linked
    };
    #[cfg(not(unix))]
    let entry_root = project.clone();
    let entry_path = entry_root.join("src/main.incn");
    fs::write(
        project.join("src/helper.incn"),
        "from main import step\n\npub def bounce(value: int) -> int:\n    return step(value)\n",
    )?;
    let count = scoped_compilation_session_analysis_invocations();
    let session = CompilationSession::discover_for_collection_with_feature_selection(&entry_path, &Default::default())?;
    let modules = collect_modules_detailed_with_session(entry_path.clone(), &session)
        .map_err(|failure| failure.render_human())?;
    let analysis = session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            None,
        )
        .map_err(|failure| failure.render_human())?;
    let lowered = modules
        .iter()
        .map(|module| {
            let info = analysis
                .type_info_for_path(&module.file_path)
                .ok_or("module analysis absent")?;
            Ok(build_body_ir_module_v0(&module.ast, &module.path_segments, info))
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let entry_index = modules
        .iter()
        .position(|module| module.file_path == entry_path)
        .ok_or("entry absent")?;
    let graph = ReplacementExecutionGraph::new(
        &lowered[entry_index],
        lowered
            .iter()
            .enumerate()
            .filter_map(|(index, module)| (index != entry_index).then_some(module)),
    )?;
    let execution = crate::backend::replacement::prepare_free_function_execution_in_graph(graph, "main", &[], None)?;
    assert_eq!(
        crate::backend::replacement::execute_prevalidated_free_function(execution)?
            .value
            .observable_text(),
        "42"
    );
    assert_eq!(count.invocation_count(), 1);
    Ok(())
}
