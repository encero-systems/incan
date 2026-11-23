use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_mir_plan_native::{
    AdmittedFunction, AdmittedPlacement, EventPosition, EventReference, Function, Local, LoweringEvent,
    MAX_PLAN_MODULE_SELECTION_BYTES, PLAN_MODULE_SELECTION_PAGE, PlanModuleFunction, PlanModuleValue, PlanType,
    TerminatorKind, block, encode_plan_module, encode_product, source_span,
};

use super::{
    AdmittedProduct, FRAME_LIST, PlanAdmissions, PlanModuleSelection, admitted_page, admitted_plan_page, read_admitted,
    report_admitted, staged_page, staged_page_reads,
};
use crate::record_positions::{indexed_product, placed_record};
use crate::resident_products::RestoreInputs;
use crate::scalar_store::AdmittedPayload;
use sha2::{Digest, Sha256};

/// Offer the page at `path` by a frame range of `segment` in its directory's frame list, named by `digest`,
/// replacing any line the list had for it.
fn list_frame(path: &Path, segment: &Path, offset: usize, length: usize, digest: &[u8]) -> Result<(), String> {
    let list = path.parent().ok_or("page has no directory")?.join(FRAME_LIST);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("page has no name")?;
    let kept = std::fs::read_to_string(&list).unwrap_or_default();
    let mut lines: Vec<String> = kept
        .lines()
        .filter(|line| line.split('\t').next() != Some(name))
        .map(str::to_owned)
        .collect();
    lines.push(format!(
        "{name}\t{}\t{offset}\t{length}\t{}",
        segment.display(),
        hex::encode(digest)
    ));
    std::fs::write(&list, lines.join("\n") + "\n").map_err(|error| error.to_string())
}

/// A fresh directory for one test.
pub(super) fn directory(test: &str) -> PathBuf {
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

/// Give a test restore the product evidence it would establish when admitting its exact page.
fn evidence(helpers: &[&str]) -> AdmittedProduct {
    AdmittedProduct {
        key: "ab".repeat(32),
        digest: format!("sha256:{}", "cd".repeat(32)),
        helpers: helpers.iter().map(|helper| (*helper).to_owned()).collect(),
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
    let mut request = PlanAdmissions::default();
    let report_text = report.to_string_lossy();
    let products_text = products.to_string_lossy();
    let partial = vec![restored("a", false)];
    report_admitted(
        &mut request,
        Some(&report_text),
        module,
        partial,
        vec![evidence(&[])],
        2,
        false,
        Some(&products_text),
        None,
    )?;
    assert_eq!(names(&report), [format!("{basename}.partially-admitted")]);
    let whole = vec![restored("a", false), restored("b", true)];
    let carried = vec![evidence(&[]), evidence(&["helper"])];
    report_admitted(
        &mut request,
        Some(&report_text),
        module,
        whole.clone(),
        carried.clone(),
        2,
        true,
        Some(&products_text),
        None,
    )?;
    assert_eq!(names(&report), [format!("{basename}.admitted")]);
    let admissions = products.join("admissions");
    assert_eq!(
        names(&admissions),
        [format!("{basename}.admitted"), format!("{basename}.admitted.placement")]
    );
    let notes = read_admitted(&admissions)?;
    assert_eq!(notes.len(), 1);
    assert!(notes[0].complete && notes[0].inspected);
    assert_eq!(notes[0].functions, whole);
    {
        let stored = request
            .modules()
            .get(module)
            .ok_or("request did not retain its admission")?;
        assert_eq!(stored.note.functions, whole);
        assert_eq!(stored.products, carried);
    }
    assert!(
        report_admitted(
            &mut request,
            Some(&report_text),
            module,
            whole,
            Vec::new(),
            2,
            true,
            Some(&products_text),
            None,
        )
        .is_err()
    );
    report_admitted(
        &mut request,
        Some(&report_text),
        module,
        Vec::new(),
        Vec::new(),
        2,
        true,
        Some(&products_text),
        None,
    )?;
    assert!(request.modules().is_empty());
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
    let mut request = PlanAdmissions::default();
    let report_text = report.to_string_lossy();
    let products_text = products.to_string_lossy();
    report_admitted(
        &mut request,
        Some(&report_text),
        "lone_alpha",
        vec![restored("a", false)],
        vec![evidence(&[])],
        1,
        false,
        Some(&products_text),
        None,
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
    Ok(AdmittedPayload {
        function: placed,
        requests: Vec::new(),
        event,
        coordinates_match: true,
        digest: evidence(&[]).digest,
    })
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
    assert!(
        Arc::ptr_eq(first.resident(), again.resident()),
        "an unchanged page was read again"
    );
    assert!(first.inspect_placement && first.event("a").is_some());
    // The kept page holds no event bytes; each request reads them from its own copy of the page.
    assert!(first.event("a").is_some_and(|event| event.event.is_empty()));
    assert_eq!(again.event_bytes("a"), Some([2_u8].as_slice()));
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
    assert!(!Arc::ptr_eq(first.resident(), placed.resident()) && !placed.inspect_placement);
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

/// A page Oven names by the digest of its bytes is found by that name: the worker does not read it again while the
/// name is unchanged, and refuses bytes that do not match the name when it does read them.
#[test]
fn a_page_named_by_its_identity_is_not_read_again() -> Result<(), String> {
    let root = directory("named");
    let products = root.to_string_lossy().into_owned();
    let page = PlanModuleValue {
        module: "named_epsilon".into(),
        functions: vec![PlanModuleFunction {
            identity: "pub::named_epsilon::a".into(),
            symbol: "pub::named_epsilon::a".into(),
            function: "a".into(),
            key: "ab".repeat(32),
            event: vec![3],
        }],
    };
    let encoded = encode_plan_module(&page)?;
    let path = root.join(format!("{}.incnmodule.reposition", hex::encode("named_epsilon")));
    let named = PathBuf::from(format!("{}.digest", path.display()));
    let write = |path: &Path, bytes: &[u8]| std::fs::write(path, bytes).map_err(|error| error.to_string());
    write(&path, &encoded)?;
    write(&named, hex::encode(Sha256::digest(&encoded)).as_bytes())?;
    let first = admitted_page(Some(&products), "named_epsilon", Some("profile"));
    assert!(first.event("a").is_some());
    // Bytes that no longer match the name are not read while the name is unchanged, and refused when they are.
    write(&path, b"not a page")?;
    let again = admitted_page(Some(&products), "named_epsilon", Some("profile"));
    assert!(
        Arc::ptr_eq(first.resident(), again.resident()),
        "a page named by its identity was read again"
    );
    assert_eq!(again.event_bytes("a"), None);
    // A name that does not match the bytes on a first read admits nothing.
    let other = root.join(format!("{}.incnmodule.reposition", hex::encode("named_zeta")));
    write(
        &other,
        &encode_plan_module(&PlanModuleValue {
            module: "named_zeta".into(),
            ..page.clone()
        })?,
    )?;
    write(
        &PathBuf::from(format!("{}.digest", other.display())),
        "cd".repeat(32).as_bytes(),
    )?;
    assert!(
        admitted_page(Some(&products), "named_zeta", Some("profile"))
            .events
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// A worker reads, checks and decodes a staged page once per content Oven names it by, and keeps it with its
/// events: a request over the same pages reads none of them again, and one changed page is the one page read and
/// hashed.
#[test]
fn a_worker_reads_and_hashes_one_page_for_one_changed_page() -> Result<(), String> {
    let root = directory("resident-full-pages");
    let write = |path: &Path, bytes: &[u8]| std::fs::write(path, bytes).map_err(|error| error.to_string());
    let page = |module: &str, event: u8| PlanModuleValue {
        module: module.into(),
        functions: vec![PlanModuleFunction {
            identity: format!("pub::{module}::a"),
            symbol: format!("pub::{module}::a"),
            function: "a".into(),
            key: "cd".repeat(32),
            event: vec![event],
        }],
    };
    let offer = |name: &str, encoded: &[u8]| -> Result<PathBuf, String> {
        let segment = root.join(format!("{name}-{}.seg", hex::encode(&Sha256::digest(encoded)[..4])));
        let mut stored = b"key prefix".to_vec();
        let offset = stored.len();
        stored.extend_from_slice(encoded);
        write(&segment, &stored)?;
        let path = root.join(format!("{}.incnmodule.positioned", hex::encode(name)));
        list_frame(&path, &segment, offset, encoded.len(), &Sha256::digest(encoded))?;
        Ok(path)
    };
    let first = offer("resident_kappa", &encode_plan_module(&page("resident_kappa", 1))?)?;
    let second = offer("resident_lambda", &encode_plan_module(&page("resident_lambda", 2))?)?;
    let reads = || staged_page_reads(&first) + staged_page_reads(&second);
    for path in [&first, &second] {
        admitted_plan_page(path)?;
    }
    assert_eq!(reads(), 2);
    for path in [&first, &second] {
        assert_eq!(admitted_plan_page(path)?.functions.len(), 1);
    }
    assert_eq!(reads(), 2, "a page the worker holds was read again");
    // One page changes: the next request reads and hashes that page alone.
    let changed = offer("resident_lambda", &encode_plan_module(&page("resident_lambda", 3))?)?;
    assert_eq!(changed, second);
    assert_eq!(admitted_plan_page(&first)?.functions[0].event, vec![1]);
    assert_eq!(admitted_plan_page(&second)?.functions[0].event, vec![3]);
    assert_eq!(reads(), 3, "one changed page was not the one page read");
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// An oversized selection copy or named digest refuses at its own file boundary before reading its bytes.
#[test]
fn staged_module_selection_bounds_both_its_files() -> Result<(), String> {
    let root = directory("selection-file-bounds");
    let path = root.join(PLAN_MODULE_SELECTION_PAGE);
    let companion = root.join(format!("{PLAN_MODULE_SELECTION_PAGE}.digest"));
    let expected = [0x55; 32];
    let write = |path: &Path, value: &[u8]| std::fs::write(path, value).map_err(|error| error.to_string());
    write(&companion, hex::encode(expected).as_bytes())?;
    let descriptor = std::fs::File::create(&path).map_err(|error| error.to_string())?;
    descriptor
        .set_len(MAX_PLAN_MODULE_SELECTION_BYTES + 1)
        .map_err(|error| error.to_string())?;
    assert!(matches!(
        PlanModuleSelection::read(&root, expected),
        Err(reason) if reason.contains(&format!("limit of {MAX_PLAN_MODULE_SELECTION_BYTES} bytes"))
    ));
    write(&path, b"incomplete descriptor")?;
    write(&companion, &[b'5'; 65])?;
    assert!(matches!(
        PlanModuleSelection::read(&root, expected),
        Err(reason) if reason.contains("limit of 64 bytes")
    ));
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// Every reader of a staged page, such as the one that appends the events of an admitted module's restored
/// functions, reads a page Oven offers by its store frame from the segment range, checked against the digest the
/// page is named by; a copied page is read as before, and a frame without its digest, or with another, is refused.
#[test]
fn every_reader_of_a_staged_page_reads_a_frame_offered_page_checked_by_its_digest() -> Result<(), String> {
    let root = directory("frame-reader");
    let write = |path: &Path, bytes: &[u8]| std::fs::write(path, bytes).map_err(|error| error.to_string());
    let segment = root.join("one.seg");
    write(&segment, b"prefix page bytes suffix")?;
    let framed = root.join("framed.incnmodule.positioned");
    list_frame(&framed, &segment, 7, 10, &Sha256::digest(b"page bytes"))?;
    assert_eq!(staged_page(&framed)?, b"page bytes".to_vec());
    let copied = root.join("copied.incnmodule.positioned");
    write(&copied, b"copied page")?;
    assert_eq!(staged_page(&copied)?, b"copied page".to_vec());
    let unnamed = root.join("unnamed.incnmodule.positioned");
    assert!(
        staged_page(&unnamed).is_err(),
        "a page with neither a copy nor a frame was read"
    );
    list_frame(&unnamed, &segment, 7, 10, &Sha256::digest(b"other bytes"))?;
    assert!(
        staged_page(&unnamed).is_err(),
        "a frame whose bytes are not its digest was read"
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// A page Oven offers by its store frame is read from the immutable segment range the frame names, never from a
/// staged copy, and only when its bytes match the content digest Oven names it by.
#[test]
fn a_page_offered_by_its_store_frame_is_read_from_the_segment() -> Result<(), String> {
    let root = directory("framed");
    let products = root.to_string_lossy().into_owned();
    let write = |path: &Path, bytes: &[u8]| std::fs::write(path, bytes).map_err(|error| error.to_string());
    let offer = |module: &str, encoded: &[u8], named: &[u8]| -> Result<PathBuf, String> {
        // The page sits inside a store segment between other frames, as a value does.
        let segment = root.join(format!("{module}.seg"));
        let mut stored = b"key prefix of the frame".to_vec();
        let offset = stored.len();
        stored.extend_from_slice(encoded);
        stored.extend_from_slice(b"the next frame");
        write(&segment, &stored)?;
        let path = root.join(format!("{}.incnmodule.reposition", hex::encode(module)));
        list_frame(&path, &segment, offset, encoded.len(), &Sha256::digest(named))?;
        Ok(path)
    };
    let page = |module: &str| PlanModuleValue {
        module: module.into(),
        functions: vec![PlanModuleFunction {
            identity: format!("pub::{module}::a"),
            symbol: format!("pub::{module}::a"),
            function: "a".into(),
            key: "cd".repeat(32),
            event: vec![5],
        }],
    };
    let encoded = encode_plan_module(&page("framed_eta"))?;
    let path = offer("framed_eta", &encoded, &encoded)?;
    assert!(!path.exists(), "the page was staged as a copy");
    let admitted = admitted_page(Some(&products), "framed_eta", Some("profile"));
    assert!(admitted.event("a").is_some());
    assert_eq!(admitted.event_bytes("a"), Some(&[5u8][..]));
    // A frame whose bytes are not the content the page is named by admits nothing.
    let other = encode_plan_module(&page("framed_theta"))?;
    offer("framed_theta", &other, b"another page")?;
    assert!(
        admitted_page(Some(&products), "framed_theta", Some("profile"))
            .events
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
