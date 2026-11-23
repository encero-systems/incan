//! Request-local admission of complete positioned module notes for native compilation.

use std::path::Path;

use incan_mir_plan_native::{AdmittedDefinition, WholeModuleAdmission};

use super::PlanModuleSelection;

/// The exact emitted whole-module note and the member fingerprints it authorized under the staged selection.
pub struct AuthenticatedWholeAdmission {
    pub note: WholeModuleAdmission,
    pub note_sha256: [u8; 32],
    pub definitions: Vec<AdmittedDefinition>,
}

/// Read all exact compact admissions for the current activation in canonical module-name order.
pub fn read_plan_module_admissions(
    _products: &Path,
    _selection: Option<&PlanModuleSelection>,
) -> Result<Vec<AuthenticatedWholeAdmission>, String> {
    Ok(Vec::new())
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

    use super::{AdmittedDefinition, read_plan_module_admissions};
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
        let selected = PlanModuleSelection {
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
                    output: output.clone(),
                    digest: page_digest.clone(),
                    count: 1,
                    positioned: true,
                    rank: 1,
                    certificate: "probe-order".into(),
                }],
            },
        };
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
        let scalar = AdmittedModule {
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
        };
        let scalar_path = admissions.join(format!("{}{PARTIAL_MODULE_NOTE}", hex::encode("legacy")));
        write(&scalar_path, &encode_module_note(&scalar)?)?;
        write_module_placement(&format!("{}.placement", scalar_path.display()), &scalar)?;
        assert_eq!(read_module_admission(&scalar_path.to_string_lossy())?, scalar);
        let read = || read_plan_module_admissions(&root, Some(&selected));
        let kept = read()?;
        assert_eq!(kept.len(), 1, "the compact request accepted no positioned module");
        assert_eq!(kept[0].note, compact);
        assert_eq!(kept[0].note_sha256, <[u8; 32]>::from(Sha256::digest(&whole_bytes)));
        assert_eq!(kept[0].definitions, [definition]);
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
        assert_eq!(read()?.len(), 1);
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
        assert!(read_plan_module_admissions(&root, None)?.is_empty());
        fs::remove_dir_all(&root).map_err(|error| error.to_string())?;
        Ok(())
    }
}
