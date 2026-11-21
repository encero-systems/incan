//! A module's admitted plan record: the page a worker restores its functions from, and the note of what it restored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use incan_mir_plan_native::{
    ADMITTED_MODULE_NOTE, AdmittedFunction, AdmittedModule, PARTIAL_MODULE_NOTE, POSITIONED_MODULE_PAGE,
    PlanModuleFunction, REPOSITION_MODULE_PAGE, decode_plan_module, encode_module_note, read_module_admission,
    write_module_placement,
};

/// Where Bakery places the plan record of an admitted module: named by the module, never by a product key.
pub fn module_page(products: &str, module: &str) -> PathBuf {
    let basename = hex::encode(module.as_bytes());
    let placed = Path::new(products).join(format!("{basename}{POSITIONED_MODULE_PAGE}"));
    if placed.is_file() {
        return placed;
    }
    Path::new(products).join(format!("{basename}{REPOSITION_MODULE_PAGE}"))
}

/// An admitted module's final events by native function name, and whether its bodies must be placed at the current
/// source's node positions.
pub struct AdmittedPage {
    pub events: HashMap<String, PlanModuleFunction>,
    pub inspect_placement: bool,
}

impl AdmittedPage {
    /// The final event of the body lowered to this native function, when the page holds one. The name carries no
    /// source position, so a body that moved in its file, as under an added comment, still finds its event.
    pub fn event(&self, function: &str) -> Option<&PlanModuleFunction> {
        self.events.get(function)
    }
}

/// The final events of an admitted module, by the native function name of the body each belongs to.
///
/// Bakery writes the page only for a module whose every witness still validates. No page, or a page that does not
/// decode or names another module, admits nothing: every body then takes the keyed route.
pub fn admitted_page(products: Option<&str>, module: &str, generation: Option<&str>) -> AdmittedPage {
    let mut page = AdmittedPage {
        events: HashMap::new(),
        inspect_placement: true,
    };
    let (Some(directory), Some(_)) = (products, generation) else {
        return page;
    };
    let path = module_page(directory, module);
    let Ok(encoded) = std::fs::read(&path) else {
        return page;
    };
    let Ok(record) = decode_plan_module(&encoded) else {
        return page;
    };
    if record.module != module {
        return page;
    }
    page.inspect_placement = path.to_string_lossy().ends_with(REPOSITION_MODULE_PAGE);
    for function in record.functions {
        page.events.insert(function.function.clone(), function);
    }
    page
}

/// Distinguish fully admitted modules from modules with individual restored functions.
fn note_suffix(complete: bool) -> &'static str {
    if complete {
        ADMITTED_MODULE_NOTE
    } else {
        PARTIAL_MODULE_NOTE
    }
}

/// Report each restored function; only a request that restored all ordinary bodies records a whole module.
pub fn report_admitted(
    report: Option<&str>,
    module: &str,
    functions: &[AdmittedFunction],
    ordinary: usize,
    inspected: bool,
    products: Option<&str>,
) -> Result<(), String> {
    let (Some(root), Some(directory)) = (report, products) else {
        return Ok(());
    };
    let basename = hex::encode(module.as_bytes());
    let selected = note_suffix(functions.len() == ordinary);
    let private = Path::new(directory).join("admissions");
    for suffix in [ADMITTED_MODULE_NOTE, PARTIAL_MODULE_NOTE] {
        let path = private.join(format!("{basename}{suffix}"));
        let diagnostic = Path::new(root).join(format!("{basename}{suffix}"));
        if suffix == selected && !functions.is_empty() {
            let note = AdmittedModule {
                module: module.to_owned(),
                complete: selected == ADMITTED_MODULE_NOTE,
                functions: functions.to_vec(),
                inspected,
            };
            write_admitted_note(&path, &diagnostic, &note)?;
            continue;
        }
        for stale in [path, diagnostic] {
            if stale.is_file() {
                std::fs::remove_file(&stale).map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

/// Publish the restored function positions used to assemble the current native plan.
fn write_admitted_note(path: &Path, diagnostic: &Path, note: &AdmittedModule) -> Result<(), String> {
    let encoded = encode_module_note(note)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    write_module_placement(&PathBuf::from(format!("{}.placement", path.display())), note)?;
    std::fs::write(path, &encoded).map_err(|error| error.to_string())?;
    // The usual descriptor-scoped note is retained for external observers. Publication reads the private pair above.
    std::fs::write(diagnostic, &encoded).map_err(|error| error.to_string())
}

/// Read every module note of a plan report, in name order.
pub fn read_admitted(directory: &Path) -> Result<Vec<AdmittedModule>, String> {
    let mut notes = Vec::new();
    if !directory.is_dir() {
        return Ok(notes);
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .map(|entry| entry.map(|entry| entry.path()).map_err(|error| error.to_string()))
        .collect::<Result<_, _>>()?;
    paths.sort_by_key(|path| path.to_string_lossy().into_owned());
    for path in paths {
        let text = path.to_string_lossy();
        if !text.ends_with(ADMITTED_MODULE_NOTE) && !text.ends_with(PARTIAL_MODULE_NOTE) {
            continue;
        }
        let note = read_module_admission(&path)?;
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned());
        let expected = format!("{}{}", hex::encode(note.module.as_bytes()), note_suffix(note.complete));
        if name.as_deref() != Some(expected.as_str()) {
            return Err("module admission note disagrees with its filename".into());
        }
        notes.push(note);
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use incan_mir_plan_native::{
        AdmittedFunction, AdmittedPlacement, EventPosition, EventReference, PlanModuleFunction, PlanModuleValue,
        encode_plan_module,
    };

    use super::{admitted_page, read_admitted, report_admitted};

    /// A fresh directory for one test.
    fn directory(test: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("incan-plan-modules-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("directory");
        path
    }

    fn restored(function: &str, placed: bool) -> AdmittedFunction {
        AdmittedFunction {
            function: function.into(),
            identity: format!("pub::lone_alpha::{function}"),
            references: vec![EventReference {
                kind: "model".into(),
                index: 1,
                name: "Point".into(),
            }],
            placement: placed.then(|| AdmittedPlacement {
                key: "ab".repeat(32),
                positions: vec![EventPosition { line: 1, column: 5 }],
            }),
        }
    }

    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn a_whole_module_note_replaces_a_partial_one_and_reads_back() {
        let root = directory("notes");
        let (report, products) = (root.join("report"), root.join("products"));
        std::fs::create_dir_all(&report).expect("report");
        let module = "lone_alpha";
        let basename = hex::encode(module);
        let (report_text, products_text) = (report.to_string_lossy(), products.to_string_lossy());
        let partial = [restored("a", false)];
        report_admitted(Some(&report_text), module, &partial, 2, false, Some(&products_text)).expect("partial");
        assert_eq!(names(&report), [format!("{basename}.partially-admitted")]);
        let whole = [restored("a", false), restored("b", true)];
        report_admitted(Some(&report_text), module, &whole, 2, true, Some(&products_text)).expect("whole");
        assert_eq!(names(&report), [format!("{basename}.admitted")]);
        assert_eq!(
            names(&products.join("admissions")),
            [format!("{basename}.admitted"), format!("{basename}.admitted.placement")]
        );
        let notes = read_admitted(&products.join("admissions")).expect("notes");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].complete && notes[0].inspected);
        assert_eq!(notes[0].functions, whole);
        report_admitted(Some(&report_text), module, &[], 2, true, Some(&products_text)).expect("none");
        assert!(names(&report).is_empty());
        assert!(
            read_admitted(&products.join("admissions"))
                .expect("no notes")
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_note_under_another_module_name_refuses() {
        let root = directory("renamed");
        let (report, products) = (root.join("report"), root.join("products"));
        std::fs::create_dir_all(&report).expect("report");
        let (report_text, products_text) = (report.to_string_lossy(), products.to_string_lossy());
        report_admitted(
            Some(&report_text),
            "lone_alpha",
            &[restored("a", false)],
            1,
            false,
            Some(&products_text),
        )
        .expect("note");
        let admissions = products.join("admissions");
        let from = admissions.join(format!("{}.admitted", hex::encode("lone_alpha")));
        let to = admissions.join(format!("{}.admitted", hex::encode("lone_beta")));
        std::fs::rename(&from, &to).expect("rename");
        // A note without changed placements and without inspection has no placement companion.
        assert!(!PathBuf::from(format!("{}.placement", from.display())).exists());
        assert!(read_admitted(&admissions).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_a_profiled_page_of_the_same_module_admits() {
        let root = directory("pages");
        let products = root.to_string_lossy().into_owned();
        let page = PlanModuleValue {
            module: "lone_alpha".into(),
            functions: vec![PlanModuleFunction {
                identity: "pub::lone_alpha::a".into(),
                symbol: "pub::lone_alpha::a".into(),
                function: "a".into(),
                key: "ef".repeat(32),
                event: vec![1],
            }],
        };
        let encoded = encode_plan_module(&page).expect("page");
        let basename = hex::encode("lone_alpha");
        std::fs::write(root.join(format!("{basename}.incnmodule.reposition")), &encoded).expect("write");
        assert!(admitted_page(Some(&products), "lone_alpha", None).events.is_empty());
        let admitted = admitted_page(Some(&products), "lone_alpha", Some("profile"));
        assert!(admitted.inspect_placement);
        assert_eq!(
            admitted.event("a").map(|event| event.key.clone()),
            Some("ef".repeat(32))
        );
        std::fs::write(root.join(format!("{basename}.incnmodule.positioned")), &encoded).expect("write");
        assert!(!admitted_page(Some(&products), "lone_alpha", Some("profile")).inspect_placement);
        let other = hex::encode("lone_beta");
        std::fs::write(root.join(format!("{other}.incnmodule.positioned")), &encoded).expect("write");
        assert!(
            admitted_page(Some(&products), "lone_beta", Some("profile"))
                .events
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
