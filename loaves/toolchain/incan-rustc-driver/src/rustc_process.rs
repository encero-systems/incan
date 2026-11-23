//! Native compilation of a checked and lowered unit, in the process that lowered it or in a fresh one.
//!
//! Checking and lowering leave the driver's heap fragmented: the allocator keeps the pages of freed checking and
//! lowering values dirty, the process footprint counts them, and rustc's analysis and LLVM allocate on top of them. A
//! resident worker whose footprint after lowering is above its ceiling therefore does not run rustc: it writes
//! everything native compilation needs into one handoff file beside the unit's output, answers its caller with that
//! file and retires, so the caller runs `--rustc-handoff PATH` in a fresh driver process once the worker's heap is
//! gone. Within the ceiling, and in a short-lived `--unit` process, the same [`RustcHandoff`] compiles in process.
//! Either way native compilation, the test report and Loaf publication happen in one place. Source checking and
//! lowering certify their own reproducible refusals before this native boundary; failures here remain retryable.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use incan_driver::loaf_publication::PreparedLoafPublication;
use incan_driver::unit::{ToolchainAccess, UnitKind};
use incan_lowering::PlanModuleSelection;
use incan_mir_plan_native::CompactAdmissionPin;
use serde::{Deserialize, Serialize};

use crate::identity::DriverIdentity;
use crate::library_exports::LibraryFacade;
use crate::native_timing::Timing;
use crate::plan::Plan;

/// The selected products' independent authority, retained across an optional native handoff.
#[derive(Serialize, Deserialize)]
pub(crate) struct PlanRecordProducts {
    pub(crate) root: PathBuf,
    pub(crate) selection: Option<[u8; 32]>,
    /// The settled note count and digest, including zero, when this compilation accepted a selection.
    pub(crate) compact_notes: Option<CompactAdmissionPin>,
}

impl PlanRecordProducts {
    /// Open the staged selection under the digest the caller passed independently from those files.
    pub(crate) fn read_selection(&self) -> Result<Option<PlanModuleSelection>, String> {
        self.selection
            .map(|digest| PlanModuleSelection::read(&self.root, digest))
            .transpose()
    }
}

/// Everything native compilation reads and what it publishes after succeeding, owned so a fresh driver process can
/// decode it. The driver identity is not carried: a fresh process verifies it again from the sysroot.
#[derive(Serialize, Deserialize)]
pub(crate) struct RustcHandoff {
    pub(crate) plan: Plan,
    pub(crate) crate_name: String,
    pub(crate) output: PathBuf,
    pub(crate) sysroot: PathBuf,
    pub(crate) externs: Vec<(String, PathBuf)>,
    pub(crate) dependency_search_paths: Vec<PathBuf>,
    pub(crate) rustc_args: Vec<String>,
    pub(crate) access: ToolchainAccess,
    pub(crate) shared_store: Option<PathBuf>,
    pub(crate) kind: UnitKind,
    pub(crate) overflow_checks: bool,
    pub(crate) library_facades: Vec<LibraryFacade>,
    pub(crate) plan_record_report: Option<PathBuf>,
    pub(crate) plan_record_products: Option<PlanRecordProducts>,
    /// A test unit's collected tests, written beside the executable once it links.
    pub(crate) tests: Option<Vec<u8>>,
    /// A library unit's Loaf and the directory it is published in once the rlib exists (DD-0007).
    pub(crate) publication: Option<(PathBuf, PreparedLoafPublication)>,
    /// The test files checked with generated case entries, each with the text it was checked as: the plan's spans
    /// resolve against these texts, and the checking process's record of them ends with that process. Filled when
    /// the handoff is written; a compile in the checking process reads its own record.
    pub(crate) checked_sources: Vec<(PathBuf, String)>,
    /// The request's captured authored bytes stay live until this native handoff completes.
    pub(crate) captured_sources: Option<PathBuf>,
    /// Checked capture texts moved into an in-process native session. A fresh process uses the leased directory.
    #[serde(skip)]
    pub(crate) captured_texts: BTreeMap<String, String>,
}

impl RustcHandoff {
    /// Run rustc, then write the test report and publish the Loaf; native-boundary failures remain retryable.
    pub(crate) fn compile(self, identity: &DriverIdentity) -> Result<(), Box<dyn std::error::Error>> {
        let _ = self.read_plan_module_selection()?;
        let _captured = crate::spans::captured_source_scope(self.captured_sources, self.captured_texts)?;
        crate::adapter::compile(
            self.plan,
            &self.crate_name,
            &self.output,
            identity,
            &self.externs,
            &self.dependency_search_paths,
            &self.rustc_args,
            &self.access,
            self.shared_store.as_deref(),
            self.kind,
            self.overflow_checks,
            self.library_facades,
            self.plan_record_report.as_deref(),
            Vec::new(),
        )
        .map_err(|error| error.to_string())?;
        if let Some(tests) = self.tests {
            std::fs::write(self.output.with_extension("tests.json"), tests)?;
        }
        if let Some((directory, publication)) = self.publication {
            publication.write(&directory).map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Admit the same selection before either native placement reads its records.
    fn read_plan_module_selection(&self) -> Result<Option<PlanModuleSelection>, String> {
        let Some(products) = &self.plan_record_products else {
            return Ok(None);
        };
        if products.selection.is_some() && self.plan_record_report.is_none() {
            return Err("plan module selection has no native report directory".into());
        }
        products.read_selection()
    }

    /// Read the same compact inputs under the accepted selection so the counted request can observe settled members.
    #[cfg(test)]
    fn read_compact_definitions(&self) -> Result<Vec<incan_mir_plan_native::AdmittedDefinition>, String> {
        let Some(products) = &self.plan_record_products else {
            return Ok(Vec::new());
        };
        let selection = self.read_plan_module_selection()?;
        let expected = match (selection.as_ref(), products.compact_notes) {
            (Some(_), None) => {
                return Err("native plan module selection has no settled compact admission outcome".into());
            }
            (None, Some(_)) => return Err("settled compact admission outcome has no plan module selection".into()),
            (_, expected) => expected,
        };
        let whole = incan_lowering::read_plan_module_admissions(&products.root, selection.as_ref())?;
        if let Some(expected) = expected {
            let observed = incan_mir_plan_native::compact_admission_pin(
                whole
                    .iter()
                    .map(|admission| (admission.note.module.as_str(), admission.note_sha256)),
            )?;
            if expected != observed {
                return Err(format!(
                    "native compact admission set differs from the settled publication (expected {} notes, observed {})",
                    expected.count, observed.count
                ));
            }
        }
        Ok(whole.into_iter().flat_map(|admission| admission.definitions).collect())
    }

    /// Write the handoff beside its output, encoded straight into the file without a whole-plan buffer, and return
    /// its path for the caller that runs it.
    pub(crate) fn hand_off(mut self, timing: &mut Timing) -> Result<PathBuf, Box<dyn std::error::Error>> {
        self.captured_texts.clear();
        self.checked_sources = crate::spans::checked_texts()?;
        let path = self.output.with_extension("rustc-handoff");
        let written = write(&self, &path);
        drop(self);
        if let Err(error) = written {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        timing.mark("rustc_handoff_written");
        Ok(path)
    }
}

/// Run native compilation from a handoff file (`--rustc-handoff PATH`) in this fresh process, removing the file once
/// it is read. A failure is printed and exits as the unit's short-lived command would.
pub(crate) fn serve(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [path] = arguments else {
        return Err("usage: incan-rustc-driver --rustc-handoff PATH".into());
    };
    let mut timing = Timing::requested();
    let encoded = std::fs::read(Path::new(path))?;
    let handoff: RustcHandoff = postcard::from_bytes(&encoded)?;
    drop(encoded);
    std::fs::remove_file(Path::new(path))?;
    timing.mark("rustc_handoff_read");
    crate::spans::retain_checked_sources(&handoff.checked_sources)?;
    let identity = crate::identity::verify(&handoff.sysroot)?;
    timing.mark("rustc_identity_verified");
    handoff.compile(&identity)
}

/// Encode the handoff into its file.
fn write(handoff: &RustcHandoff, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = postcard::to_io(handoff, BufWriter::new(File::create(path)?))?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_mir_plan_native::{
        ADMITTED_MODULE_NOTE, NativeInterface, PLAN_MODULE_SELECTION_PAGE, POSITIONED_MODULE_PAGE,
        WholeModuleAdmission, compact_admission_pin, decode_lowering_event, decode_plan_function_record,
        decode_plan_module, decode_product, encode_native_interface, encode_record_plan, encode_whole_module_admission,
        native_interface_filename,
    };
    use sha2::{Digest, Sha256};
    use std::fs;

    /// The test and both runtime placements carry the accepted selection independently of the staged files.
    fn handoff(root: &Path, selection: Option<[u8; 32]>) -> RustcHandoff {
        let span = incan_mir_plan_native::source_span("handoff.incn", 1, 1);
        RustcHandoff {
            plan: Plan::new(Vec::new(), Vec::new(), Vec::new(), Vec::new(), span),
            crate_name: "handoff".into(),
            output: root.join("output"),
            sysroot: root.join("sysroot"),
            externs: Vec::new(),
            dependency_search_paths: Vec::new(),
            rustc_args: Vec::new(),
            access: ToolchainAccess::default(),
            shared_store: None,
            kind: UnitKind::Program,
            overflow_checks: true,
            library_facades: Vec::new(),
            plan_record_report: Some(root.join("report")),
            plan_record_products: Some(PlanRecordProducts {
                root: root.join("products"),
                selection,
                compact_notes: None,
            }),
            tests: None,
            publication: None,
            checked_sources: Vec::new(),
            captured_sources: None,
            captured_texts: BTreeMap::new(),
        }
    }

    /// Stage exact producer bytes and their named digest without reading authority back from staging.
    fn stage_named(path: &Path, bytes: &[u8]) -> std::io::Result<[u8; 32]> {
        let digest = Sha256::digest(bytes).into();
        let mut companion = path.as_os_str().to_os_string();
        companion.push(".digest");
        std::fs::write(path, bytes)?;
        std::fs::write(PathBuf::from(companion), hex::encode(digest))?;
        Ok(digest)
    }

    /// Another valid graph selection of the same page refuses in both native placements under the original pin.
    #[test]
    fn native_handoff_refuses_a_replaced_named_plan_selection() -> Result<(), Box<dyn std::error::Error>> {
        // Incan's plan_module_authority authored the two graph generations from the same positioned probe page.
        let selection_a = include_bytes!("../tests/fixtures/plan_module_selection/selection-a.crumb");
        let selection_b = include_bytes!("../tests/fixtures/plan_module_selection/selection-b.crumb");
        let positioned = include_bytes!("../tests/fixtures/plan_module_selection/70726f6265.incnmodule.positioned");
        let root = tempfile::tempdir()?;
        let products = root.path().join("products");
        std::fs::create_dir(&products)?;
        let staged = products.join(PLAN_MODULE_SELECTION_PAGE);
        let packet = root.path().join("request.rustc-handoff");

        let scalar = handoff(root.path(), None);
        write(&scalar, &packet)?;
        let scalar: RustcHandoff = postcard::from_bytes(&std::fs::read(&packet)?)?;
        assert!(scalar.read_plan_module_selection()?.is_none());

        let original = stage_named(&staged, selection_a)?;
        let resident = handoff(root.path(), Some(original));
        let accepted = resident
            .read_plan_module_selection()?
            .ok_or("the accepted selection was lost")?;
        assert_eq!(accepted.scope().generation, 1);
        write(&resident, &packet)?;

        let replacement = stage_named(&staged, selection_b)?;
        assert_ne!(replacement, original);
        let foreign = handoff(root.path(), Some(replacement));
        let valid = foreign
            .read_plan_module_selection()?
            .ok_or("the foreign selection was lost")?;
        assert_eq!(valid.scope().generation, 2);
        let accepted_module = accepted
            .module("probe")
            .ok_or("the fixture did not select the probe module")?;
        assert_eq!(accepted_module.digest, hex::encode(Sha256::digest(positioned)));
        assert_eq!(accepted_module.count, 1);
        assert_eq!(Some(accepted_module), valid.module("probe"));

        let detached: RustcHandoff = postcard::from_bytes(&std::fs::read(packet)?)?;
        for request in [&resident, &detached] {
            let authority = request
                .plan_record_products
                .as_ref()
                .ok_or("lost plan record products")?;
            assert_eq!(authority.selection, Some(original));
            assert!(
                matches!(
                    request.read_plan_module_selection(),
                    Err(reason) if reason.contains("does not name the accepted digest")
                ),
                "a staged replacement bypassed the native selection admission"
            );
        }
        Ok(())
    }

    const MODULE: &str = "probe";
    const PAGE: &[u8] = include_bytes!("../tests/fixtures/plan_module_selection/70726f6265.incnmodule.positioned");
    const SELECTION_A: &[u8] = include_bytes!("../tests/fixtures/plan_module_selection/selection-a.crumb");
    const SELECTION_B: &[u8] = include_bytes!("../tests/fixtures/plan_module_selection/selection-b.crumb");
    const PROFILE: &[u8] = b"a86e4d83e0cb13ca8ffccd67993a2ed8ce5a663c53893c0cc802dc181d3e79b6";

    struct Fixture {
        root: tempfile::TempDir,
        note: PathBuf,
        interface: Vec<u8>,
    }

    impl Fixture {
        /// Stage the exact positioned product shared with the Incan selector.
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let root = tempfile::tempdir()?;
            let products = root.path().join("products");
            let note = products
                .join("admissions")
                .join(format!("{}{ADMITTED_MODULE_NOTE}", hex::encode(MODULE)));
            fs::create_dir_all(note.parent().ok_or("compact note has no parent")?)?;
            fs::write(products.join("producer-profile"), PROFILE)?;
            let page = products.join(format!("{}{POSITIONED_MODULE_PAGE}", hex::encode(MODULE)));
            let _ = stage_named(&page, PAGE)?;
            let positioned = decode_plan_module(PAGE)?;
            let [member] = positioned.functions.as_slice() else {
                return Err("the compact fixture must hold exactly one function".into());
            };
            let event = decode_lowering_event(&member.event)?;
            let product = decode_plan_function_record(&event.product)?;
            let (function, externals) = decode_product(&product.function, &[])?;
            let span = function.span.clone();
            let plan = crate::plan::Plan::new(Vec::new(), Vec::new(), vec![function], externals, span);
            let interface = encode_native_interface(&NativeInterface {
                entry: event.key,
                plan: encode_record_plan(&plan)?,
            })?;
            Ok(Self { root, note, interface })
        }

        /// Stage one valid production-encoded selection and its independently captured one-note outcome.
        fn stage_selection(
            &self,
            fixture: &[u8],
        ) -> Result<([u8; 32], CompactAdmissionPin), Box<dyn std::error::Error>> {
            let products = self.root.path().join("products");
            let selection = stage_named(&products.join(PLAN_MODULE_SELECTION_PAGE), fixture)?;
            let authority = incan_lowering::PlanModuleSelection::read(&products, selection)?;
            let selected = authority
                .module(MODULE)
                .ok_or("the selector fixture does not name probe")?;
            assert_eq!(selected.digest, hex::encode(Sha256::digest(PAGE)));
            assert_eq!(selected.count, 1);
            let note = encode_whole_module_admission(&WholeModuleAdmission {
                module: MODULE.into(),
                output: selected.output.clone(),
                digest: selected.digest.clone(),
                count: selected.count,
                selection: hex::encode(selection),
            })?;
            let pin = compact_admission_pin([(MODULE, Sha256::digest(&note).into())])?;
            fs::write(&self.note, &note)?;
            Ok((selection, pin))
        }

        /// Build a decoded handoff through the same Postcard writer used when the worker retires.
        fn requests(
            &self,
            pin: [u8; 32],
            notes: Option<CompactAdmissionPin>,
            interface: bool,
        ) -> Result<[RustcHandoff; 2], Box<dyn std::error::Error>> {
            let root = self.root.path();
            let report = root.join(if interface { "reported" } else { "unreported" });
            fs::create_dir_all(&report)?;
            if interface {
                fs::write(report.join(native_interface_filename()), &self.interface)?;
            }
            let mut resident = handoff(root, Some(pin));
            resident.plan_record_report = Some(report);
            resident.plan_record_products = Some(PlanRecordProducts {
                root: root.join("products"),
                selection: Some(pin),
                compact_notes: notes,
            });
            let packet = root.join("request.rustc-handoff");
            write(&resident, &packet)?;
            let detached = postcard::from_bytes(&fs::read(packet)?)?;
            Ok([resident, detached])
        }
    }

    #[test]
    fn native_compact_outcome_counts_the_same_exact_emitted_set_after_handoff() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = Fixture::new()?;
        for interface in [false, true] {
            let (selection, notes) = fixture.stage_selection(SELECTION_A)?;
            let requests = fixture.requests(selection, Some(notes), interface)?;
            let observed = requests[0].read_compact_definitions()?;
            assert_eq!(
                observed.len(),
                1,
                "the native reader must retain one authoritative definition"
            );
            assert_eq!(observed[0].function, "probe_run");
            assert_eq!(observed[0].identity, "pub::probe::run");
            let unpinned = handoff(fixture.root.path(), None);
            assert!(matches!(
                unpinned.read_compact_definitions(),
                Err(reason) if reason.contains("no independently accepted selection")
            ));
            fs::remove_file(&fixture.note)?;
            for request in &requests {
                assert_eq!(
                    request
                        .plan_record_products
                        .as_ref()
                        .and_then(|products| products.compact_notes),
                    Some(notes)
                );
                assert!(
                    matches!(
                        request.read_compact_definitions(),
                        Err(reason) if reason.contains("expected 1 notes, observed 0")
                    ),
                    "the missing emitted note bypassed the native admission"
                );
            }
            fixture.stage_selection(SELECTION_A)?;
            fs::write(&fixture.note, b"damaged compact note")?;
            for request in &requests {
                assert!(request.read_compact_definitions().is_err());
            }
        }
        Ok(())
    }

    #[test]
    fn native_compact_outcome_refuses_a_coherent_foreign_pair_after_handoff() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = Fixture::new()?;
        for interface in [false, true] {
            let (selection_a, notes_a) = fixture.stage_selection(SELECTION_A)?;
            let requests = fixture.requests(selection_a, Some(notes_a), interface)?;
            assert_eq!(requests[0].read_compact_definitions()?.len(), 1);
            let (selection_b, notes_b) = fixture.stage_selection(SELECTION_B)?;
            assert_ne!(selection_a, selection_b);
            assert_eq!(notes_a.count, notes_b.count);
            let [valid_b, _] = fixture.requests(selection_b, Some(notes_b), interface)?;
            assert_eq!(valid_b.read_compact_definitions()?.len(), 1);
            for request in &requests {
                assert!(
                    matches!(
                        request.read_compact_definitions(),
                        Err(reason) if reason.contains("does not name the accepted digest")
                    ),
                    "a coherent foreign selection and note bypassed the native pin"
                );
            }
            fixture.stage_selection(SELECTION_A)?;
            for request in &fixture.requests(selection_a, Some(notes_b), interface)? {
                assert!(
                    matches!(
                        request.read_compact_definitions(),
                        Err(reason) if reason.contains("expected 1 notes, observed 1")
                    ),
                    "the native reader compared only the number of notes"
                );
            }
        }
        Ok(())
    }

    /// A valid selected request may emit no compact note and still requires its native report location.
    #[test]
    fn native_compact_outcome_accepts_the_empty_set_and_requires_a_report() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new()?;
        let (selection, _) = fixture.stage_selection(SELECTION_A)?;
        fs::remove_file(&fixture.note)?;
        let mut digest = [0; 32];
        hex::decode_to_slice(
            "95ee7d59f26344aeaf5603871ed13d20fbd05638dfc19c804b24d02abaf90d98",
            &mut digest,
        )?;
        let empty = CompactAdmissionPin { count: 0, digest };
        let mut different = empty;
        different.digest[0] ^= 1;
        for with_interface in [false, true] {
            for request in &fixture.requests(selection, None, with_interface)? {
                assert!(
                    matches!(request.read_compact_definitions(), Err(reason) if reason.contains("no settled compact admission outcome")),
                    "a selected request accepted an absent publication in place of the authenticated empty set"
                );
            }
            for request in &fixture.requests(selection, Some(empty), with_interface)? {
                assert!(request.read_compact_definitions()?.is_empty());
            }
            for request in &fixture.requests(selection, Some(different), with_interface)? {
                assert!(
                    matches!(request.read_compact_definitions(), Err(reason) if reason.contains("expected 0 notes, observed 0")),
                    "a selected request accepted a different digest for the empty set"
                );
            }
        }
        let root = fixture.root.path();
        let [mut without_report, _] = fixture.requests(selection, Some(empty), false)?;
        without_report.plan_record_report = None;
        let packet = root.join("no-report.rustc-handoff");
        write(&without_report, &packet)?;
        let detached: RustcHandoff = postcard::from_bytes(&fs::read(packet)?)?;
        for request in [&without_report, &detached] {
            assert!(
                matches!(request.read_compact_definitions(), Err(reason) if reason.contains("no native report directory"))
            );
        }
        let mut scalar = handoff(root, None);
        scalar.plan_record_report = None;
        assert!(scalar.read_compact_definitions()?.is_empty());
        scalar
            .plan_record_products
            .as_mut()
            .ok_or("missing test products")?
            .compact_notes = Some(empty);
        let packet = root.join("no-selection.rustc-handoff");
        write(&scalar, &packet)?;
        let detached: RustcHandoff = postcard::from_bytes(&fs::read(packet)?)?;
        for request in [&scalar, &detached] {
            assert!(
                matches!(request.read_compact_definitions(), Err(reason) if reason.contains("outcome has no plan module selection"))
            );
        }
        Ok(())
    }
}
