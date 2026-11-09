//! Memory-aware rustc codegen concurrency under the unit compilation budget.

use std::ffi::OsString;
use std::process::Command;
use std::sync::OnceLock;

use rustc_data_structures::jobserver::Client;

const MIB: u64 = 1024 * 1024;
const UNIT_BUDGET_BYTES: u64 = 2 * 1024 * MIB;
const CODEGEN_JOB_BYTES: u64 = 256 * MIB;
const MAX_CODEGEN_JOBS: usize = 16;

static JOBSERVER: OnceLock<Result<CodegenJobserver, String>> = OnceLock::new();

/// The owning client remains alive for the process-global rustc client, while one token represents the process's
/// implicit main-thread job.
struct CodegenJobserver {
    client: Client,
}

/// Tokens held out of rustc's jobserver for one serialized compilation.
pub(super) struct CodegenLimit {
    client: Client,
    held: usize,
}

/// Create and publish the bounded jobserver before any resident-worker thread exists.
pub(super) fn initialize() -> Result<(), String> {
    match JOBSERVER.get_or_init(CodegenJobserver::new) {
        Ok(_server) => Ok(()),
        Err(error) => Err(error.clone()),
    }
}

/// Hold every token beyond what the unit's remaining memory can support until rustc has torn codegen down.
pub(super) fn limit() -> Result<CodegenLimit, String> {
    initialize()?;
    let server = match JOBSERVER.get() {
        Some(Ok(server)) => server,
        Some(Err(error)) => return Err(error.clone()),
        None => return Err("codegen jobserver was not initialized".to_owned()),
    };
    let current = incan_platform::current_process_footprint()
        .map_err(|error| format!("cannot read compiler footprint for codegen budget: {error}"))?;
    let physical = incan_platform::physical_memory_bytes()
        .map_err(|error| format!("cannot read physical memory for codegen budget: {error}"))?;
    let jobs = codegen_jobs(current, physical);
    let mut held = 0;
    while held < MAX_CODEGEN_JOBS - jobs {
        if let Err(error) = server.client.acquire_raw() {
            for _ in 0..held {
                let _ = server.client.release_raw();
            }
            return Err(format!("cannot reserve codegen jobserver token: {error}"));
        }
        held += 1;
    }
    Ok(CodegenLimit {
        client: server.client.clone(),
        held,
    })
}

impl CodegenJobserver {
    /// Create the maximum 16-job server, reserve its implicit process token, let rustc inherit it once, then remove
    /// the temporary environment value before worker threads can observe it.
    fn new() -> Result<Self, String> {
        let client =
            Client::new(MAX_CODEGEN_JOBS).map_err(|error| format!("cannot create codegen jobserver: {error}"))?;
        client
            .acquire_raw()
            .map_err(|error| format!("cannot reserve codegen jobserver process token: {error}"))?;
        let flags = jobserver_flags(&client)?;
        let previous = std::env::var_os("CARGO_MAKEFLAGS");
        // SAFETY: driver dispatch calls this before resident observation or rustc starts any threads; the value is
        // removed immediately after rustc clones the jobserver descriptors.
        unsafe { std::env::set_var("CARGO_MAKEFLAGS", flags) };
        let mut warning = None;
        rustc_data_structures::jobserver::initialize_checked(|message| warning = Some(message));
        match previous {
            Some(value) => {
                // SAFETY: this is the same single-threaded initialization scope described above.
                unsafe { std::env::set_var("CARGO_MAKEFLAGS", value) };
            }
            None => {
                // SAFETY: this is the same single-threaded initialization scope described above.
                unsafe { std::env::remove_var("CARGO_MAKEFLAGS") };
            }
        }
        if let Some(message) = warning {
            return Err(message.to_owned());
        }
        Ok(Self { client })
    }
}

impl Drop for CodegenLimit {
    /// Return every token held for this compilation after rustc's codegen threads have stopped.
    fn drop(&mut self) {
        for _ in 0..self.held {
            let _ = self.client.release_raw();
        }
    }
}

/// Read the jobserver environment value produced by the crate without exposing its platform-specific descriptors.
fn jobserver_flags(client: &Client) -> Result<OsString, String> {
    let mut command = Command::new("incan-jobserver-environment");
    client.configure(&mut command);
    command
        .get_envs()
        .find_map(|(name, value)| (name == "CARGO_MAKEFLAGS").then(|| value.map(OsString::from)).flatten())
        .ok_or_else(|| "codegen jobserver produced no CARGO_MAKEFLAGS".to_owned())
}

/// Convert the remaining 2 GiB unit budget into 256 MiB codegen jobs, with one job as the conservative floor.
fn codegen_jobs(current: u64, physical: u64) -> usize {
    let available = UNIT_BUDGET_BYTES.min(physical).saturating_sub(current);
    let jobs = available / CODEGEN_JOB_BYTES;
    match usize::try_from(jobs) {
        Ok(jobs) => jobs.clamp(1, MAX_CODEGEN_JOBS),
        Err(_) => MAX_CODEGEN_JOBS,
    }
}

#[cfg(test)]
mod tests {
    use super::{CODEGEN_JOB_BYTES, UNIT_BUDGET_BYTES, codegen_jobs};

    /// Remaining budget maps directly to 256 MiB codegen jobs.
    #[test]
    fn remaining_budget_sizes_codegen_jobs() {
        assert_eq!(codegen_jobs(UNIT_BUDGET_BYTES / 2, 16 * UNIT_BUDGET_BYTES), 4);
    }

    /// A unit already at its budget still receives the one job rustc needs to make progress.
    #[test]
    fn exhausted_budget_keeps_one_codegen_job() {
        assert_eq!(codegen_jobs(UNIT_BUDGET_BYTES, 16 * UNIT_BUDGET_BYTES), 1);
    }

    /// Installed physical memory bounds the budget on a smaller host.
    #[test]
    fn physical_memory_bounds_codegen_jobs() {
        assert_eq!(codegen_jobs(0, 2 * CODEGEN_JOB_BYTES), 2);
    }
}
