//! Module collection through a compilation session: the test that needs the driver's session.

use std::error::Error;
use std::fs;

use incan_frontend::body_ir::build_body_ir_module_v0;

/// An import cycle back into the entry collects each module once and analyzes them in one session invocation, and
/// Body IR represents every body of both. The program's run is the behavior fixture
/// `execution_calls_types_and_modules/an_import_cycle_back_into_the_entry`.
#[test]
fn an_import_cycle_back_into_the_entry_is_collected_once() -> Result<(), Box<dyn Error>> {
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
    assert_eq!(modules.len(), 2, "the cycle must collect each module once");
    assert!(
        modules.iter().any(|module| module.file_path == entry_path),
        "the entry must be collected under the spelling it was given"
    );
    for module in &lowered {
        for body in &module.bodies {
            assert_eq!(body.first_representation_gap(), None, "`{}`", body.name);
        }
    }
    assert_eq!(count.invocation_count(), 1);
    Ok(())
}
