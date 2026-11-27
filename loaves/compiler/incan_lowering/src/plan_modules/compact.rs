//! Request-local admission of complete positioned module notes for native compilation.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::Path;

use incan_mir_plan_native::{
    ADMITTED_MODULE_NOTE, AdmittedDefinition, MAX_PLAN_MODULE_SELECTION_BYTES, PARTIAL_MODULE_NOTE,
    POSITIONED_MODULE_PAGE, PlanSelectedModule, RequestModuleNote, WHOLE_ADMISSION_SELECTOR, WholeModuleAdmission,
    decode_request_module_note, read_positioned_plan_members,
};
use internal_models_bindings::{complete_view, mapped_bytes, single_row_reference_prefix};
use sha2::{Digest, Sha256};

use super::{FRAME_LIST, ListedFrame, PlanModuleSelection, framed_bytes, parse_frame_list, read_selection_file};

// A row header, maximum encoded payload length, first reference and seven-byte selector fit in this prefix.
const NOTE_PREFIX_BYTES: usize = 32;

/// The exact emitted whole-module note and the member fingerprints it authorized under the staged selection.
pub struct AuthenticatedWholeAdmission {
    pub note: WholeModuleAdmission,
    pub note_sha256: [u8; 32],
    pub definitions: Vec<AdmittedDefinition>,
}

#[cfg(test)]
thread_local! {
    static DECODED_NOTES: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static PREFIX_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static POSITIONED_BACKING_BYTES: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// Read all exact compact admissions for the current activation in canonical module-name order.
pub fn read_plan_module_admissions(
    products: &Path,
    selection: Option<&PlanModuleSelection>,
) -> Result<Vec<AuthenticatedWholeAdmission>, String> {
    let directory = products.join("admissions");
    let paths = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if selection.is_none() || error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read compact module admissions: {error}")),
    };
    let pinned = selection.map(|selection| (selection, hex::encode(selection.digest())));
    let selector = WHOLE_ADMISSION_SELECTOR.to_vec();
    let mut compact = BTreeMap::new();
    let mut scalar = BTreeSet::new();
    let frames = OnceCell::new();
    for entry in paths {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(suffix) = [ADMITTED_MODULE_NOTE, PARTIAL_MODULE_NOTE]
            .into_iter()
            .find(|suffix| name.ends_with(suffix))
        else {
            continue;
        };
        let Some((selection, expected)) = &pinned else {
            if note_prefix(&entry).is_some_and(|prefix| single_row_reference_prefix(&prefix, &selector)) {
                return Err("whole-module admission has no independently accepted selection".into());
            }
            continue;
        };
        let (note, encoded) = read_note(&entry)?;
        match note {
            RequestModuleNote::Scalar { module, complete } => {
                check_note_name(name, suffix, &module, complete)?;
                scalar.insert(module);
            }
            RequestModuleNote::Whole(note) => {
                check_note_name(name, suffix, &note.module, true)?;
                let selected = selected_positioned_module(selection, expected, &note)?;
                no_sparse_placement(&entry.path())?;
                let definitions = selected_module_definitions(products, selected, &frames)?;
                let module = note.module.clone();
                let replaced = compact.insert(
                    module,
                    AuthenticatedWholeAdmission {
                        note,
                        note_sha256: Sha256::digest(&encoded).into(),
                        definitions,
                    },
                );
                if replaced.is_some() {
                    return Err("module has more than one compact admission".into());
                }
            }
        }
    }
    if compact.keys().any(|module| scalar.contains(module)) {
        return Err("module has overlapping scalar and compact admissions".into());
    }
    Ok(compact.into_values().collect())
}

/// Read only the bytes needed to recognize a possible unsolicited compact note.
fn note_prefix(entry: &std::fs::DirEntry) -> Option<Vec<u8>> {
    if !entry.file_type().ok()?.is_file() {
        return None;
    }
    let mut prefix = Vec::with_capacity(NOTE_PREFIX_BYTES);
    std::fs::File::open(entry.path())
        .ok()?
        .take(NOTE_PREFIX_BYTES as u64)
        .read_to_end(&mut prefix)
        .ok()?;
    #[cfg(test)]
    PREFIX_BYTES.with(|read| read.set(read.get() + prefix.len()));
    Some(prefix)
}

/// Decode one bounded note and retain exactly the bytes its settled fingerprint names.
fn read_note(entry: &std::fs::DirEntry) -> Result<(RequestModuleNote, Vec<u8>), String> {
    if !entry.file_type().map_err(|error| error.to_string())?.is_file() {
        return Err("module admission is not an ordinary staged file".into());
    }
    let encoded = read_selection_file(&entry.path(), MAX_PLAN_MODULE_SELECTION_BYTES)?;
    #[cfg(test)]
    DECODED_NOTES.with(|read| {
        let (count, bytes) = read.get();
        read.set((count + 1, bytes + encoded.len()));
    });
    Ok((decode_request_module_note(&encoded)?, encoded))
}

/// Bind the actual suffix and basename to the admitted module and completeness status.
fn check_note_name(name: &str, suffix: &str, module: &str, complete: bool) -> Result<(), String> {
    if complete != (suffix == ADMITTED_MODULE_NOTE) || name != format!("{}{suffix}", hex::encode(module)) {
        return Err("module admission note disagrees with its filename".into());
    }
    Ok(())
}

/// Require the complete current selection and all its matching positioned page identity fields.
fn selected_positioned_module<'a>(
    selection: &'a PlanModuleSelection,
    expected: &str,
    note: &WholeModuleAdmission,
) -> Result<&'a PlanSelectedModule, String> {
    if note.selection != expected {
        return Err("whole-module admission names a different selected graph".into());
    }
    let selected = selection
        .module(&note.module)
        .ok_or("whole-module admission has no selected module")?;
    if !selected.positioned
        || selected.output != note.output
        || selected.digest != note.digest
        || selected.count != note.count
    {
        return Err("whole-module admission differs from its selected positioned page".into());
    }
    Ok(selected)
}

/// A compact note cannot carry or coexist with a sparse placement companion.
fn no_sparse_placement(note: &Path) -> Result<(), String> {
    let mut path = note.as_os_str().to_os_string();
    path.push(".placement");
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot check compact module placement: {error}")),
        Ok(_) => Err("whole-module admission has a stale placement companion".into()),
    }
}

/// Read the exact named positioned copy or immutable store frame as one bounded input.
fn selected_module_definitions(
    products: &Path,
    selected: &PlanSelectedModule,
    frames: &OnceCell<Result<HashMap<String, ListedFrame>, String>>,
) -> Result<Vec<AdmittedDefinition>, String> {
    let basename = format!("{}{POSITIONED_MODULE_PAGE}", hex::encode(&selected.module));
    let path = products.join(&basename);
    let bytes = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            let named = read_selection_file(&products.join(format!("{basename}.digest")), 64)?;
            if named != selected.digest.as_bytes() {
                return Err("positioned Plan copy does not name the selected page digest".into());
            }
            read_selection_file(&path, MAX_PLAN_MODULE_SELECTION_BYTES)?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let offered = frames
                .get_or_init(|| current_frame_list(products))
                .as_ref()
                .map_err(Clone::clone)?;
            let frame = offered
                .get(&basename)
                .ok_or("selected positioned module has no staged page")?;
            if hex::encode(frame.content) != selected.digest || frame.length as u64 > MAX_PLAN_MODULE_SELECTION_BYTES {
                return Err("positioned Plan frame does not name a bounded selected page".into());
            }
            let segment = std::fs::metadata(&frame.segment).map_err(|error| error.to_string())?;
            if !segment.is_file()
                || frame
                    .offset
                    .checked_add(frame.length as u64)
                    .is_none_or(|end| end > segment.len())
            {
                return Err("selected positioned Plan frame is outside its segment".into());
            }
            framed_bytes(frame).ok_or("cannot read the selected positioned Plan frame")?
        }
        Ok(_) => return Err("selected positioned Plan copy was not a regular file".into()),
        Err(error) => {
            return Err(format!("cannot inspect staged positioned Plan copy: {error}"));
        }
    };
    #[cfg(test)]
    let capture = (bytes.as_ptr().addr(), bytes.len());
    let owner = mapped_bytes(&bytes)?;
    drop(bytes);
    let view = complete_view(&owner).ok_or("cannot retain the selected positioned Plan view")?;
    #[cfg(test)]
    POSITIONED_BACKING_BYTES.with(|backing| {
        let (total, copied) = backing.get();
        let (address, bytes) = capture;
        let copied_bytes = if view.0.as_bytes().as_ptr().addr() == address {
            0
        } else {
            bytes
        };
        backing.set((total + bytes, copied + copied_bytes));
    });
    read_positioned_plan_members(&view, selected)
}

/// Admit this request's actual frame inventory once before reading any offered positioned ranges.
fn current_frame_list(products: &Path) -> Result<HashMap<String, ListedFrame>, String> {
    let bytes = read_selection_file(&products.join(FRAME_LIST), MAX_PLAN_MODULE_SELECTION_BYTES)?;
    let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
    let frames = parse_frame_list(text).ok_or("malformed selected Plan frame list")?;
    if frames.len() != text.lines().count() {
        return Err("selected Plan frame list repeats a page name".into());
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use incan_mir_plan_native::{
        ADMITTED_MODULE_NOTE, AdmittedFunction, AdmittedModule, AdmittedPlacement, EventPosition, Function, Local,
        LoweringEvent, PARTIAL_MODULE_NOTE, POSITIONED_MODULE_PAGE, PlanFunctionRecord, PlanModuleFunction,
        PlanModuleSelectionRecord, PlanModuleValue, PlanSelectedModule, PlanSelectionScope, PlanType, RvalueKind,
        TerminatorKind, WholeModuleAdmission, assign, block, encode_lowering_event, encode_module_note,
        encode_plan_function_record, encode_plan_module, encode_product, encode_whole_module_admission, integer, place,
        plan_function_digest, read_module_admission, source_span, write_module_placement,
    };
    use sha2::{Digest, Sha256};

    use super::{
        AdmittedDefinition, DECODED_NOTES, NOTE_PREFIX_BYTES, POSITIONED_BACKING_BYTES, PREFIX_BYTES,
        WHOLE_ADMISSION_SELECTOR, read_plan_module_admissions,
    };
    use crate::plan_modules::{FRAME_LIST, PlanModuleSelection, tests::directory};

    /// Build one native positioned page with an independently known definition and nonempty product.
    fn positioned_fixture() -> Result<(Vec<u8>, AdmittedDefinition), String> {
        let span = source_span("probe.incn", 7, 3);
        let body = block(
            vec![assign(place(0, &span), RvalueKind::Use(integer(10, &span)), &span)],
            TerminatorKind::Return,
            &span,
        );
        let name = "probe_run";
        let symbol = "pub::probe::run";
        let function = Function::new(
            name.into(),
            Vec::new(),
            PlanType::Int,
            vec![Local {
                ty: PlanType::Int,
                span: span.clone(),
            }],
            vec![body],
            span,
        );
        let binary = encode_product(&function, &[], &[], &[])?;
        let digest = plan_function_digest(&binary, &[])?;
        let product = encode_plan_function_record(&PlanFunctionRecord {
            digest: digest.clone(),
            inputs: vec!["positioned-probe".into()],
            function: binary,
            requests: Vec::new(),
        })?;
        let key = "c7".repeat(32);
        let event = encode_lowering_event(&LoweringEvent {
            function: name.into(),
            identity: symbol.into(),
            key: key.clone(),
            module: "probe".into(),
            product,
            positions: vec![EventPosition { line: 0, column: 3 }],
            reused: true,
            ..LoweringEvent::default()
        })?;
        let encoded = encode_plan_module(&PlanModuleValue {
            module: "probe".into(),
            functions: vec![PlanModuleFunction {
                identity: "function:probe::run|signature".into(),
                symbol: symbol.into(),
                function: name.into(),
                key: key.clone(),
                event,
            }],
        })?;
        Ok((
            encoded,
            AdmittedDefinition {
                function: name.into(),
                identity: symbol.into(),
                key,
                digest,
            },
        ))
    }

    /// A pinned compact selection of the one positioned `probe` module page with `page_digest` at `output`.
    fn positioned_selection(pin: [u8; 32], output: &str, page_digest: &str) -> PlanModuleSelection {
        PlanModuleSelection {
            digest: pin,
            record: PlanModuleSelectionRecord {
                scope: PlanSelectionScope {
                    store_root: String::new(),
                    placement: "memory:1".into(),
                    manifest: "8a".repeat(32),
                    generation: 3,
                    view: "home".into(),
                    index: "index".into(),
                    origin: "probe".into(),
                    profile: "incan.lowering-records/12".into(),
                },
                modules: vec![PlanSelectedModule {
                    module: "probe".into(),
                    output: output.to_owned(),
                    digest: page_digest.to_owned(),
                    count: 1,
                    positioned: true,
                    rank: 1,
                    certificate: "probe-order".into(),
                }],
            },
        }
    }

    /// An incomplete inspected scalar note for `legacy` whose one function keeps a placement.
    fn legacy_scalar_note() -> AdmittedModule {
        AdmittedModule {
            module: "legacy".into(),
            complete: false,
            inspected: true,
            functions: vec![AdmittedFunction {
                function: "legacy_run".into(),
                identity: "pub::legacy::run".into(),
                references: Vec::new(),
                placement: Some(AdmittedPlacement {
                    key: "2d".repeat(32),
                    positions: vec![EventPosition { line: 4, column: 3 }],
                }),
            }],
        }
    }

    /// A complete uninspected note for `ordinary` whose one function has no placement.
    fn ordinary_complete_note() -> AdmittedModule {
        AdmittedModule {
            module: "ordinary".into(),
            complete: true,
            inspected: false,
            functions: vec![AdmittedFunction {
                function: "ordinary_run".into(),
                identity: "pub::ordinary::run".into(),
                references: Vec::new(),
                placement: None,
            }],
        }
    }

    /// An unpinned request decodes no scalar note and reads only note prefixes, but still refuses a whole-module note,
    /// including one with a damaged body or description digest.
    fn assert_unpinned_requests_skip_scalar_notes(
        root: &Path,
        admissions: &Path,
        note: &Path,
        whole_bytes: Vec<u8>,
    ) -> Result<(), String> {
        let write = |path: &Path, value: &[u8]| fs::write(path, value).map_err(|error| error.to_string());
        let ordinary = admissions.join(format!("{}{ADMITTED_MODULE_NOTE}", hex::encode("ordinary")));
        let ordinary_note = ordinary_complete_note();
        write(&ordinary, &encode_module_note(&ordinary_note)?)?;
        let malformed_scalar = admissions.join(format!("{}{ADMITTED_MODULE_NOTE}", hex::encode("old")));
        let malformed = b"malformed legacy diagnostic";
        write(&malformed_scalar, malformed)?;
        DECODED_NOTES.with(|read| read.set((0, 0)));
        PREFIX_BYTES.with(|read| read.set(0));
        assert!(read_plan_module_admissions(root, None)?.is_empty());
        assert_eq!(
            DECODED_NOTES.with(std::cell::Cell::get),
            (0, 0),
            "an unpinned native request replayed legacy scalar diagnostics"
        );
        assert_eq!(
            PREFIX_BYTES.with(std::cell::Cell::get),
            2 * NOTE_PREFIX_BYTES + malformed.len()
        );
        write(note, &whole_bytes)?;
        assert!(read_plan_module_admissions(root, None).is_err());
        write(note, &whole_bytes[..whole_bytes.len() - 1])?;
        assert!(
            read_plan_module_admissions(root, None).is_err(),
            "an unpinned whole-module note with a damaged body was ignored"
        );
        let dictionary = whole_bytes
            .windows(WHOLE_ADMISSION_SELECTOR.len())
            .position(|window| window == WHOLE_ADMISSION_SELECTOR)
            .ok_or("whole module note omitted its reference selector")?;
        let mut damaged_digest = whole_bytes;
        damaged_digest[dictionary + WHOLE_ADMISSION_SELECTOR.len()] ^= 1;
        write(note, &damaged_digest)?;
        assert!(
            read_plan_module_admissions(root, None).is_err(),
            "an unpinned whole-module note with a damaged description digest was ignored"
        );
        assert_eq!(DECODED_NOTES.with(std::cell::Cell::get), (0, 0));
        Ok(())
    }

    /// Require the current staged positioned bytes and independent note, preserving an old scalar in the same scan.
    #[test]
    fn compact_notes_resolve_one_exact_positioned_page() -> Result<(), String> {
        let root = directory("compact-positioned");
        let admissions = root.join("admissions");
        fs::create_dir_all(&admissions).map_err(|error| error.to_string())?;
        let (encoded, definition) = positioned_fixture()?;
        let page_digest = hex::encode(Sha256::digest(&encoded));
        let pin = [0x6cu8; 32];
        let output = format!("plan-module\tincan.stable-declaration/1\tprobe\tprobe.incn\tchecked\t{page_digest}");
        let selected = positioned_selection(pin, &output, &page_digest);
        let compact = WholeModuleAdmission {
            module: "probe".into(),
            output,
            digest: page_digest.clone(),
            count: 1,
            selection: hex::encode(pin),
        };
        let whole_bytes = encode_whole_module_admission(&compact)?;
        let note = admissions.join(format!("{}{ADMITTED_MODULE_NOTE}", hex::encode("probe")));
        let page_name = format!("{}{POSITIONED_MODULE_PAGE}", hex::encode("probe"));
        let page = root.join(&page_name);
        let companion = root.join(format!("{page_name}.digest"));
        let write = |path: &Path, value: &[u8]| fs::write(path, value).map_err(|error| error.to_string());
        write(&note, &whole_bytes)?;
        write(&page, &encoded)?;
        write(&companion, page_digest.as_bytes())?;
        let scalar = legacy_scalar_note();
        let scalar_path = admissions.join(format!("{}{PARTIAL_MODULE_NOTE}", hex::encode("legacy")));
        write(&scalar_path, &encode_module_note(&scalar)?)?;
        write_module_placement(&format!("{}.placement", scalar_path.display()), &scalar)?;
        assert_eq!(read_module_admission(&scalar_path.to_string_lossy())?, scalar);
        let read = || read_plan_module_admissions(&root, Some(&selected));
        POSITIONED_BACKING_BYTES.with(|count| count.set((0, 0)));
        let kept = read()?;
        let copy_backing = POSITIONED_BACKING_BYTES.with(|count| count.replace((0, 0)));
        assert_eq!(kept.len(), 1, "the compact request accepted no positioned module");
        assert_eq!(kept[0].note, compact);
        assert_eq!(kept[0].note_sha256, <[u8; 32]>::from(Sha256::digest(&whole_bytes)));
        assert!(kept[0].definitions == [definition.clone()]);
        assert!(read_plan_module_admissions(&root, None).is_err());
        let foreign_output = WholeModuleAdmission {
            output: "foreign-output".into(),
            ..compact.clone()
        };
        write(&note, &encode_whole_module_admission(&foreign_output)?)?;
        assert!(
            read().is_err(),
            "another output key matched the accepted compact selection"
        );
        write(&note, &whole_bytes)?;

        let mut damaged = encoded.clone();
        damaged[0] ^= 1;
        write(&page, &damaged)?;
        assert!(
            read().is_err(),
            "the selected positioned digest companion masked damaged bytes"
        );
        fs::remove_file(&page).map_err(|error| error.to_string())?;
        fs::remove_file(&companion).map_err(|error| error.to_string())?;
        assert!(read().is_err(), "the selected positioned page disappeared");

        let segment = root.join("segment");
        write(&segment, &[b"segment".as_slice(), &encoded].concat())?;
        let frame = root.join(FRAME_LIST);
        let indexed = |offset| {
            format!(
                "{page_name}\t{}\t{offset}\t{}\t{page_digest}\n",
                segment.display(),
                encoded.len(),
            )
        };
        write(&frame, indexed(7).as_bytes())?;
        POSITIONED_BACKING_BYTES.with(|count| count.set((0, 0)));
        let framed = read()?;
        let frame_backing = POSITIONED_BACKING_BYTES.with(|count| count.replace((0, 0)));
        assert_eq!(framed.len(), 1);
        assert!(framed[0].definitions == [definition]);
        write(&frame, indexed(encoded.len() + 20).as_bytes())?;
        assert!(read().is_err(), "an out-of-range compact frame was admitted");
        write(&frame, indexed(7).as_bytes())?;
        let stale = admissions.join(format!("{}{ADMITTED_MODULE_NOTE}.placement", hex::encode("probe")));
        write(&stale, b"legacy placement")?;
        assert!(
            read().is_err(),
            "the complete positioned note accepted a stale placement"
        );
        fs::remove_file(&stale).map_err(|error| error.to_string())?;
        fs::remove_file(&note).map_err(|error| error.to_string())?;

        assert_unpinned_requests_skip_scalar_notes(&root, &admissions, &note, whole_bytes)?;
        fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
        assert_eq!(
            [copy_backing, frame_backing],
            [(encoded.len(), 0); 2],
            "direct and framed compact admission must retain the original capture without copying its bytes"
        );
        Ok(())
    }
}
