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

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use incan_driver::loaf_publication::PreparedLoafPublication;
use incan_driver::unit::{ToolchainAccess, UnitKind};
use serde::{Deserialize, Serialize};

use crate::identity::DriverIdentity;
use crate::library_exports::LibraryFacade;
use crate::native_timing::Timing;
use crate::plan::Plan;

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
}

impl RustcHandoff {
    /// Run rustc, then write the test report and publish the Loaf; native-boundary failures remain retryable.
    pub(crate) fn compile(self, identity: &DriverIdentity) -> Result<(), Box<dyn std::error::Error>> {
        let _captured = crate::spans::captured_source_scope(self.captured_sources)?;
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

    /// Write the handoff beside its output, encoded straight into the file without a whole-plan buffer, and return
    /// its path for the caller that runs it.
    pub(crate) fn hand_off(mut self, timing: &mut Timing) -> Result<PathBuf, Box<dyn std::error::Error>> {
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
