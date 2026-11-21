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
///
/// `frames` names, aligned with `functions`, the helper frames each restored product carries. They go only to the
/// request-private note directory, where plan publication reads them instead of decoding every admitted product again.
pub fn report_admitted(
    report: Option<&str>,
    module: &str,
    functions: &[AdmittedFunction],
    frames: &[Vec<String>],
    ordinary: usize,
    inspected: bool,
    products: Option<&str>,
) -> Result<(), String> {
    let (Some(root), Some(directory)) = (report, products) else {
        return Ok(());
    };
    if frames.len() != functions.len() {
        return Err("admitted helper frames are not aligned with the restored functions".into());
    }
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
            write_admitted_note(&path, &diagnostic, &note, frames)?;
            continue;
        }
        for stale in [frames_path(&path), path, diagnostic] {
            if stale.is_file() {
                std::fs::remove_file(&stale).map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

/// Publish the restored function positions used to assemble the current native plan, with the private list of the
/// helper frames each restored product carries.
fn write_admitted_note(
    path: &Path,
    diagnostic: &Path,
    note: &AdmittedModule,
    frames: &[Vec<String>],
) -> Result<(), String> {
    let encoded = encode_module_note(note)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    write_module_placement(&format!("{}.placement", path.display()), note)?;
    let mut listed = String::new();
    for (function, carried) in note.functions.iter().zip(frames) {
        listed.push_str(&function.function);
        for frame in carried {
            listed.push('\t');
            listed.push_str(frame);
        }
        listed.push('\n');
    }
    std::fs::write(frames_path(path), listed).map_err(|error| error.to_string())?;
    std::fs::write(path, &encoded).map_err(|error| error.to_string())?;
    // The usual descriptor-scoped note is retained for external observers. Publication reads the private files above.
    std::fs::write(diagnostic, &encoded).map_err(|error| error.to_string())
}

/// The private list of helper frames beside a module note: one line per restored function, its native name then
/// the frames its product carries, separated by tabs.
fn frames_path(note: &Path) -> PathBuf {
    PathBuf::from(format!("{}.frames", note.display()))
}

/// Read the helper frames each restored function of a module note carries, by native function name.
pub fn admitted_frames(directory: &Path, note: &AdmittedModule) -> Result<HashMap<String, Vec<String>>, String> {
    let path = frames_path(&directory.join(format!(
        "{}{}",
        hex::encode(note.module.as_bytes()),
        note_suffix(note.complete)
    )));
    let listed =
        std::fs::read_to_string(&path).map_err(|error| format!("cannot read admitted helper frames {error}"))?;
    let mut frames = HashMap::new();
    for line in listed.lines() {
        let mut names = line.split('\t');
        let Some(function) = names.next() else {
            continue;
        };
        frames.insert(function.to_owned(), names.map(str::to_owned).collect());
    }
    if frames.len() != note.functions.len()
        || note
            .functions
            .iter()
            .any(|function| !frames.contains_key(&function.function))
    {
        return Err(format!(
            "admitted helper frames of {} disagree with its note",
            note.module
        ));
    }
    Ok(frames)
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
        let note = read_module_admission(&path.to_string_lossy())?;
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
    use std::sync::Arc;

    use incan_mir_plan_native::{
        AdmittedFunction, AdmittedPlacement, EventPosition, EventReference, Function, Local, LoweringEvent,
        PlanModuleFunction, PlanModuleValue, PlanType, TerminatorKind, block, encode_plan_module, encode_product,
        source_span,
    };

    use super::{admitted_frames, admitted_page, read_admitted, report_admitted};
    use crate::record_positions::{indexed_product, placed_record};
    use crate::resident_products::RestoreInputs;
    use crate::scalar_store::AdmittedPayload;

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
    fn a_whole_module_note_replaces_a_partial_one_and_reads_back() -> Result<(), String> {
        let root = directory("notes");
        let (report, products) = (root.join("report"), root.join("products"));
        std::fs::create_dir_all(&report).map_err(|error| error.to_string())?;
        let module = "lone_alpha";
        let basename = hex::encode(module);
        let (report_text, products_text) = (report.to_string_lossy(), products.to_string_lossy());
        let partial = [restored("a", false)];
        report_admitted(
            Some(&report_text),
            module,
            &partial,
            &[Vec::new()],
            2,
            false,
            Some(&products_text),
        )?;
        assert_eq!(names(&report), [format!("{basename}.partially-admitted")]);
        let whole = [restored("a", false), restored("b", true)];
        let carried = [Vec::new(), vec!["helper".to_owned()]];
        report_admitted(
            Some(&report_text),
            module,
            &whole,
            &carried,
            2,
            true,
            Some(&products_text),
        )?;
        assert_eq!(names(&report), [format!("{basename}.admitted")]);
        let admissions = products.join("admissions");
        assert_eq!(
            names(&admissions),
            [
                format!("{basename}.admitted"),
                format!("{basename}.admitted.frames"),
                format!("{basename}.admitted.placement")
            ]
        );
        let notes = read_admitted(&admissions)?;
        assert_eq!(notes.len(), 1);
        assert!(notes[0].complete && notes[0].inspected);
        assert_eq!(notes[0].functions, whole);
        let frames = admitted_frames(&admissions, &notes[0])?;
        assert_eq!(frames.get("a"), Some(&Vec::new()));
        assert_eq!(frames.get("b"), Some(&vec!["helper".to_owned()]));
        assert!(report_admitted(Some(&report_text), module, &whole, &[], 2, true, Some(&products_text)).is_err());
        report_admitted(Some(&report_text), module, &[], &[], 2, true, Some(&products_text))?;
        assert!(names(&report).is_empty());
        assert!(read_admitted(&admissions)?.is_empty());
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn a_note_under_another_module_name_refuses() -> Result<(), String> {
        let root = directory("renamed");
        let (report, products) = (root.join("report"), root.join("products"));
        std::fs::create_dir_all(&report).map_err(|error| error.to_string())?;
        let (report_text, products_text) = (report.to_string_lossy(), products.to_string_lossy());
        report_admitted(
            Some(&report_text),
            "lone_alpha",
            &[restored("a", false)],
            &[Vec::new()],
            1,
            false,
            Some(&products_text),
        )?;
        let admissions = products.join("admissions");
        let from = admissions.join(format!("{}.admitted", hex::encode("lone_alpha")));
        let to = admissions.join(format!("{}.admitted", hex::encode("lone_beta")));
        std::fs::rename(&from, &to).map_err(|error| error.to_string())?;
        // A note without changed placements and without inspection has no placement companion.
        assert!(!PathBuf::from(format!("{}.placement", from.display())).exists());
        assert!(read_admitted(&admissions).is_err());
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
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

    /// A placed product for a restored page entry, its requests and the event it came from.
    fn restored_payload() -> Result<AdmittedPayload, String> {
        let owner = source_span("resident.incn", 10, 1);
        let exit = block(Vec::new(), TerminatorKind::Return, &source_span("resident.incn", 11, 5));
        let locals = vec![Local {
            ty: PlanType::Unit,
            span: owner.clone(),
        }];
        let function = Function::new("a".into(), Vec::new(), PlanType::Unit, locals, vec![exit], owner);
        let wire = encode_product(&function, &[], &[], &[])?;
        let stored = indexed_product(&wire, &[(0, 1), (1, 5)], "resident.incn", 10)?;
        let placed = placed_record(&stored, &[(0, 1), (1, 5)], "resident.incn", 30)?;
        let event = LoweringEvent {
            identity: "pub::resident_gamma::a".into(),
            key: "cd".repeat(32),
            function: "a".into(),
            product: Vec::new(),
            reused: false,
            admitted: true,
            module: "resident_gamma".into(),
            role: "ordinary".into(),
            demand: String::new(),
            runtime_requirements: 0,
            references: Vec::new(),
            positions: Vec::new(),
        };
        Ok((placed, Vec::new(), event))
    }

    /// A resident worker reads an unchanged page once and restores each of its products once per restore inputs.
    #[test]
    fn a_resident_worker_reads_an_unchanged_page_once() -> Result<(), String> {
        let root = directory("resident");
        let products = root.to_string_lossy().into_owned();
        let page = PlanModuleValue {
            module: "resident_gamma".into(),
            functions: vec![PlanModuleFunction {
                identity: "pub::resident_gamma::a".into(),
                symbol: "pub::resident_gamma::a".into(),
                function: "a".into(),
                key: "cd".repeat(32),
                event: vec![2],
            }],
        };
        let encoded = encode_plan_module(&page)?;
        let basename = hex::encode("resident_gamma");
        let reposition = root.join(format!("{basename}.incnmodule.reposition"));
        std::fs::write(&reposition, &encoded).map_err(|error| error.to_string())?;
        let first = admitted_page(Some(&products), "resident_gamma", Some("profile"));
        let again = admitted_page(Some(&products), "resident_gamma", Some("profile"));
        assert!(Arc::ptr_eq(&first, &again), "an unchanged page was read again");
        assert!(first.inspect_placement && first.event("a").is_some());
        // The same bytes under another module name stay refused, and placed bytes are another page.
        let other = root.join(format!("{}.incnmodule.positioned", hex::encode("resident_delta")));
        std::fs::write(&other, &encoded).map_err(|error| error.to_string())?;
        assert!(
            admitted_page(Some(&products), "resident_delta", Some("profile"))
                .events
                .is_empty()
        );
        let positioned = root.join(format!("{basename}.incnmodule.positioned"));
        std::fs::write(&positioned, &encoded).map_err(|error| error.to_string())?;
        let placed = admitted_page(Some(&products), "resident_gamma", Some("profile"));
        assert!(!Arc::ptr_eq(&first, &placed) && !placed.inspect_placement);
        let inputs = RestoreInputs {
            frame: "a".into(),
            file: "resident.incn".into(),
            line: 30,
            overflow: true,
            reads: "reads".into(),
            generation: "profile".into(),
            positions: Some(vec![(0, 1), (1, 5)]),
        };
        let mut restores = 0;
        let made = first.restored(&inputs, || {
            restores += 1;
            restored_payload()
        })?;
        let kept = first.restored(&inputs, || {
            restores += 1;
            restored_payload()
        })?;
        assert!(
            Arc::ptr_eq(&made, &kept) && restores == 1,
            "a kept product was restored again"
        );
        let moved = RestoreInputs {
            line: 31,
            ..inputs.clone()
        };
        first.restored(&moved, || {
            restores += 1;
            restored_payload()
        })?;
        assert_eq!(
            restores, 2,
            "a product restored at another line was read from the worker"
        );
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
