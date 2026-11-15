//! A test unit's batch: test modules that share a plain session-scoped `conftest.incn` fixture compile into one test
//! unit and run in one process, so the fixture is set up once for all of them, as 0.5.1's single worker does.
//!
//! Each module keeps its own file group (`crate::test_groups`), whose entry is public. The conftest that declares a
//! shared plain session fixture holds its static and the public accessor that sets it up on first use, and each
//! module imports that accessor instead of writing its own. The unit's entry module registers one native entry that
//! runs every module's group in run order. A group's case markers carry the module's position, so the runner tells
//! cases of different modules apart in the one process's output.
//!
//! Test modules usually lie outside the source root, where a module's logical path is spelled from the directory an
//! import resolves it in. Every generated import therefore climbs to one directory, the batch's frame, before it
//! descends, and the entry module's logical path is spelled from the frame too, so every module of the batch has one
//! logical path whichever module imports it.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::test_conftest::Conftest;
use crate::test_entries::{PREFIX, blank, declaration_region};
use crate::test_groups::GROUP;
use incan_driver::session::CompilationSession;
use incan_driver::testing::types::{FixtureInfo, FixtureScope};
use incan_frontend::ast::Declaration;
use incan_lang::lang::conventions::NONE_TYPE_NAME;

/// The native entry a batch's entry module registers; it runs every module's group.
pub(crate) const BATCH: &str = "incan_case_batch";

/// A plain session fixture a conftest declares, which every test module below it can share: its conftest, its name
/// and its written type.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SessionFixture {
    pub(crate) conftest: PathBuf,
    pub(crate) name: String,
    pub(crate) ty: String,
}

impl SessionFixture {
    /// The key the runner groups test modules by: the conftest's path below the project root, as a stored unit's
    /// report is reused wherever the project lies, and the fixture's name.
    pub(crate) fn key(&self, project_root: &Path) -> String {
        format!(
            "{}::{}",
            project_path(&self.conftest, project_root).display(),
            self.name
        )
    }
}

/// A path below the project root, or the path itself outside it.
pub(crate) fn project_path<'a>(path: &'a Path, project_root: &Path) -> &'a Path {
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    match path.strip_prefix(&root) {
        Ok(below) => below,
        Err(_) => path,
    }
}

/// One module's place in its unit's batch: its position in run order, every module's path, and the batch's frame.
pub(crate) struct BatchPlace {
    pub(crate) position: usize,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) frame: PathBuf,
}

/// The batch's frame: the deepest directory that holds every module and every conftest on their chains.
pub(crate) fn frame<'a>(files: &'a [PathBuf], conftests: impl Iterator<Item = &'a Path>) -> PathBuf {
    let mut directories: Vec<&Path> = files.iter().filter_map(|file| file.parent()).collect();
    directories.extend(conftests.filter_map(Path::parent));
    let Some(first) = directories.first() else {
        return PathBuf::new();
    };
    let mut common: PathBuf = first.to_path_buf();
    for directory in &directories[1..] {
        while !directory.starts_with(&common) {
            if !common.pop() {
                break;
            }
        }
    }
    common
}

impl BatchPlace {
    /// The marker a case of this module carries: its entry, prefixed with the module's position.
    pub(crate) fn marker(&self, entry: &str) -> String {
        format!("f{}.{entry}", self.position)
    }
}

/// Whether `fixture`, as `chain` offers it to a test module, can be shared by every module of a batch: a plain,
/// synchronous session fixture without parameters that a conftest declares. Its conftest is the nearest that declares
/// it, which is the one the module receives it from.
pub(crate) fn shareable<'a>(fixture: &FixtureInfo, chain: &'a [Conftest]) -> Option<&'a Conftest> {
    if fixture.scope != FixtureScope::Session
        || fixture.has_teardown
        || fixture.is_async
        || !fixture.dependencies.is_empty()
    {
        return None;
    }
    chain.iter().find(|conftest| {
        conftest
            .discovery
            .fixtures
            .iter()
            .any(|declared| declared.name == fixture.name)
    })
}

/// The import path from the module at `from` to the module at `to` through `frame`: one dot more than the number of
/// directories from `from`'s directory up to the frame, then `to`'s dotted path below the frame.
pub(crate) fn import_path(from: &Path, to: &Path, frame: &Path) -> String {
    let climbs = from
        .parent()
        .and_then(|directory| directory.strip_prefix(frame).ok())
        .map_or(0, |below| below.components().count());
    let module = to.with_extension("");
    let below: Vec<String> = module
        .strip_prefix(frame)
        .unwrap_or(&module)
        .components()
        .map(|component: Component<'_>| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    let lead = if climbs == 0 {
        String::new()
    } else {
        ".".repeat(climbs + 1)
    };
    format!("{lead}{}", below.join("."))
}

/// The import of each shared fixture's accessor a module at `file` writes instead of its own static.
pub(crate) fn accessor_imports(file: &Path, hosted: &[SessionFixture], frame: &Path) -> String {
    let mut written = String::new();
    for fixture in hosted {
        written.push_str(&format!(
            "\n\nfrom {} import {PREFIX}_shared_{}\n",
            import_path(file, &fixture.conftest, frame),
            fixture.name
        ));
    }
    written
}

/// The entry module's batch entry: every module's group, the entry's own first, then the others in run order through
/// their public group entries.
pub(crate) fn batch_entry(files: &[PathBuf], frame: &Path) -> String {
    let mut written = String::new();
    let mut calls = vec![format!("    {GROUP}()")];
    for (position, file) in files.iter().enumerate().skip(1) {
        written.push_str(&format!(
            "\n\nfrom {} import {GROUP} as {BATCH}_{position}\n",
            import_path(&files[0], file, frame)
        ));
        calls.push(format!("    {BATCH}_{position}()"));
    }
    written.push_str(&format!(
        "\n\ndef {BATCH}() -> {NONE_TYPE_NAME}:\n{}\n",
        calls.join("\n")
    ));
    written
}

/// Each conftest that hosts a shared fixture, checked as a module of the unit: its text with its `yield` fixtures
/// blanked line for line, as a test module's appended copy blanks them, then each hosted fixture's static and public
/// accessor.
pub(crate) fn conftest_sources(
    hosted: &[SessionFixture],
    conftests: &[&Conftest],
    session: &CompilationSession,
) -> Result<Vec<(PathBuf, String)>, String> {
    let mut by_conftest: BTreeMap<&Path, Vec<&SessionFixture>> = BTreeMap::new();
    for fixture in hosted {
        by_conftest.entry(&fixture.conftest).or_default().push(fixture);
    }
    let mut sources = Vec::new();
    for (path, fixtures) in by_conftest {
        let declared = conftests
            .iter()
            .find(|conftest| conftest.path == path)
            .ok_or_else(|| format!("batch conftest {} is not on a module's chain", path.display()))?;
        let source =
            std::fs::read_to_string(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let program = session
            .parse_source(path, &source, false)
            .map_err(|errors| format!("cannot parse {}: {errors:?}", path.display()))?;
        let mut regions = Vec::new();
        for declaration in &program.declarations {
            let Declaration::Function(function) = &declaration.node else {
                continue;
            };
            let yields = declared
                .discovery
                .fixtures
                .iter()
                .any(|fixture| fixture.name == function.name && fixture.has_teardown);
            if yields {
                regions.push(declaration_region(&source, declaration, function));
            }
        }
        let mut text = blank(&source, &regions);
        for fixture in fixtures {
            let name = &fixture.name;
            let ty = &fixture.ty;
            text.push_str(&format!(
                "\n\nstatic {PREFIX}_scope_{name}: list[{ty}] = []\n\n\npub def {PREFIX}_shared_{name}() -> {ty}:\n    if len({PREFIX}_scope_{name}) == 0:\n        {PREFIX}_scope_{name}.append({name}())\n    return {PREFIX}_scope_{name}[0]\n"
            ));
        }
        sources.push((path.to_path_buf(), text));
    }
    Ok(sources)
}
