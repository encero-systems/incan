//! rustc in a fresh driver process, so the back end starts on a clean heap.
//!
//! Checking and lowering leave the driver's heap fragmented: the allocator keeps the pages of freed checking and
//! lowering values, the process footprint counts them, and rustc's analysis and LLVM would allocate on top of them.
//! The driver that checked and lowered a unit therefore writes everything native compilation reads into one handoff
//! file beside the unit's output, frees its own heap, and runs rustc in a fresh driver process from that file
//! (`--rustc-handoff PATH`). That process never checks or lowers. The file is removed as soon as the process ends.
//! The process inherits the console, working directory and environment, so rustc's diagnostics and timing records
//! reach the same places they did in process.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use incan_driver::unit::{ToolchainAccess, UnitKind};
use serde::{Deserialize, Serialize};

use crate::adapter::DriverError;
use crate::library_exports::LibraryFacade;
use crate::native_timing::Timing;
use crate::plan::Plan;

/// The status the rustc process exits with when preflight refuses the plan itself, a deterministic refusal.
const PLAN_REFUSAL: i32 = 3;

/// Everything native compilation reads, owned so a fresh driver process can decode it. The driver identity is not
/// carried: the rustc process verifies it again from the sysroot.
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
}

/// How native compilation in the rustc process ended. That process already printed any failure.
pub(crate) enum RustcOutcome {
    Compiled,
    PlanRefused,
    Failed,
}

/// Write the handoff beside its output, return this process's freed heap, run rustc in a fresh driver process, and
/// remove the handoff whether or not that process succeeded.
pub(crate) fn run(handoff: RustcHandoff, timing: &mut Timing) -> Result<RustcOutcome, Box<dyn std::error::Error>> {
    let path = handoff.output.with_extension("rustc-handoff");
    let written = write(&handoff, &path);
    drop(handoff);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    // The plan was the last checking or lowering value this process held; return its pages before rustc runs.
    crate::heap::release_free_memory();
    timing.mark("rustc_handoff_written");
    let status = Command::new(std::env::current_exe()?)
        .arg("--rustc-handoff")
        .arg(&path)
        .status();
    let removed = std::fs::remove_file(&path);
    let status = status.map_err(|error| format!("the rustc process could not start: {error}"))?;
    removed.map_err(|error| format!("the rustc handoff {} could not be removed: {error}", path.display()))?;
    timing.mark("rustc_process_exited");
    Ok(match status.code() {
        Some(0) => RustcOutcome::Compiled,
        Some(PLAN_REFUSAL) => RustcOutcome::PlanRefused,
        Some(_) => RustcOutcome::Failed,
        None => return Err(format!("the rustc process ended without a status: {status}").into()),
    })
}

/// Run native compilation from a handoff file (`--rustc-handoff PATH`) and return the status the process exits with.
pub(crate) fn serve(arguments: &[String]) -> Result<i32, Box<dyn std::error::Error>> {
    let [path] = arguments else {
        return Err("usage: incan-rustc-driver --rustc-handoff PATH".into());
    };
    let mut timing = Timing::requested();
    let encoded = std::fs::read(Path::new(path))?;
    let handoff: RustcHandoff = postcard::from_bytes(&encoded)?;
    drop(encoded);
    timing.mark("rustc_handoff_read");
    let RustcHandoff {
        plan,
        crate_name,
        output,
        sysroot,
        externs,
        dependency_search_paths,
        rustc_args,
        access,
        shared_store,
        kind,
        overflow_checks,
        library_facades,
        plan_record_report,
    } = handoff;
    let identity = crate::identity::verify(&sysroot)?;
    timing.mark("rustc_identity_verified");
    let compiled = crate::adapter::compile(
        plan,
        &crate_name,
        &output,
        &identity,
        &externs,
        &dependency_search_paths,
        &rustc_args,
        &access,
        shared_store.as_deref(),
        kind,
        overflow_checks,
        library_facades,
        plan_record_report.as_deref(),
    );
    match compiled {
        Ok(()) => Ok(0),
        Err(error) => {
            eprintln!("{error}");
            Ok(if matches!(error, DriverError::Plan(_)) {
                PLAN_REFUSAL
            } else {
                1
            })
        }
    }
}

/// Encode the handoff straight into its file, without a whole-plan buffer.
fn write(handoff: &RustcHandoff, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = postcard::to_io(handoff, BufWriter::new(File::create(path)?))?;
    writer.flush()?;
    Ok(())
}
