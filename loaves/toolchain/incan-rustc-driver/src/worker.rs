//! The compiler worker's executable entry. The Incan worker host (`incan_worker_host`) owns the private socket, peer
//! identity, the `incan.compiler-worker/1` protocol and every lifecycle decision. This entry asks the host for one
//! request at a time, runs it through the entry point its short-lived command uses, and gives the host the outcome
//! together with the process's physical footprint. A compiler panic is contained here and the adapter's process-global
//! state restored, so the host answers a typed compiler fault and decides what happens next.
//!
//! Bootstrap boundary (DD-0013, "Language and platform boundary", adaptation 3): besides the entry itself, the Rust
//! kept here is what dev.8 Incan cannot express. Redirecting the process console for one activation (`console`) and
//! reading the physical footprint or returning freed heap need FFI that Incan refuses as unsafe; they move to the
//! core's resource probes. The caller watch is a thread that observes the caller's socket while the compiler runs:
//! dev.8 Incan cannot hand a socket to a thread, because a closure for `std::thread::spawn` cannot own a value without
//! a native `Clone`, and shared carriers (`Arc`, channels, the platform's `Connection`) lack retained `Send` evidence.
//! What a departed caller means is the host's decision (`incan_worker_host::abandon`).

use std::os::fd::RawFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use incan_worker_host::{ActivationOutcome, CallerWatch, CompileRequest, WorkerCommand};

mod console;

/// Serve the requests the worker host admits until it retires.
pub(crate) fn serve(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut host = incan_worker_host::start(&arguments.to_vec(), footprint())?;
    while let Some(request) = incan_worker_host::next_request(&mut host) {
        let mut timing = crate::native_timing::Timing::requested();
        let watch =
            incan_worker_host::caller_watch(&mut host).map(|watch| Abandonment::watch(watch, request.retire_after));
        let outcome = activate(&request);
        timing.mark("worker_compiled");
        // Disarm before replying. A one-activation process has no next caller, so its observer can end with it.
        drop(watch);
        if request.retire_after {
            timing.mark("worker_observer_detached");
        } else {
            timing.mark("worker_observer_joined");
            release_free_memory();
            timing.mark("worker_heap_released");
        }
        incan_worker_host::complete(&mut host, outcome, footprint());
        timing.mark("worker_completed");
    }
    Ok(())
}

/// Run one admitted request with its caller's console files and working directory. The reported status is the one
/// the short-lived command exits with: its own status, or the status of a returned error, which [`crate::report`]
/// prints as `main` does. A panic is contained and reported as such once the adapter's state is usable again.
fn activate(request: &CompileRequest) -> ActivationOutcome {
    let console = match console::Capture::open(
        Path::new(&request.stdout),
        Path::new(&request.stderr),
        Path::new(&request.cwd),
    ) {
        Ok(console) => console,
        Err(error) => return ActivationOutcome::Unavailable(error.to_string()),
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&request.command, &request.arguments)
    }));
    let ended = match outcome {
        Ok(Ok(code)) => ActivationOutcome::Exited(code.into()),
        Ok(Err(error)) => ActivationOutcome::Exited(crate::report(error.as_ref()).into()),
        Err(_) => {
            crate::adapter::recover();
            ActivationOutcome::Panicked
        }
    };
    if let Err(error) = console.restore() {
        return ActivationOutcome::UnsafeCleanup(format!("compiler console restore failed: {error}"));
    }
    ended
}

/// Run one request through the entry point its short-lived command uses, returning that command's status.
fn run(command: &WorkerCommand, arguments: &[String]) -> Result<i32, Box<dyn std::error::Error>> {
    match command {
        WorkerCommand::Unit => crate::frontend::compile_resident(arguments).map(|()| 0),
        WorkerCommand::Check => crate::check(arguments),
        WorkerCommand::Format => crate::format(arguments),
    }
}

/// The process's physical footprint in bytes, or -1 when it cannot be read.
fn footprint() -> i64 {
    physical_footprint()
        .ok()
        .and_then(|bytes| i64::try_from(bytes).ok())
        .unwrap_or(-1)
}

/// The process's current physical footprint in bytes, the measure macOS accounts memory pressure by. Clean pages of
/// mapped files are not the worker's to give back and are not counted.
#[cfg(target_os = "macos")]
fn physical_footprint() -> std::io::Result<u64> {
    let pid = libc::c_int::try_from(std::process::id()).map_err(std::io::Error::other)?;
    // SAFETY: `rusage_info_v4` holds only integers, so the zeroed value is valid.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let buffer = (&raw mut info).cast::<libc::rusage_info_t>();
    // SAFETY: `buffer` addresses a writable `rusage_info_v4`, the record the V4 flavor fills, for this call only.
    if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, buffer) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(info.ri_phys_footprint)
}

/// The process's current resident memory in bytes: its resident pages in `/proc/self/statm` times the page size.
#[cfg(target_os = "linux")]
fn physical_footprint() -> std::io::Result<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm")?;
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| std::io::Error::other("/proc/self/statm has no resident field"))?
        .parse()
        .map_err(std::io::Error::other)?;
    // SAFETY: `sysconf` reads a configuration value and has no preconditions.
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).map_err(std::io::Error::other)?;
    Ok(pages.saturating_mul(page))
}

/// No footprint reading on other systems, so the host never retires a worker there for its footprint.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn physical_footprint() -> std::io::Result<u64> {
    Err(std::io::Error::other("no physical footprint reading on this system"))
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// libmalloc: return free pages of `zone` (every zone when null) to the system, up to `goal` bytes (all when zero).
    fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: libc::size_t) -> libc::size_t;
}

/// Return the heap a request freed to the system, so the footprint the host judges is what the worker retains.
#[cfg(target_os = "macos")]
fn release_free_memory() {
    // SAFETY: a null zone and a zero goal ask libmalloc to release every free page; no memory is touched.
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
}

/// Return the heap a request freed to the system, so the footprint the host judges is what the worker retains.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release_free_memory() {
    // SAFETY: malloc_trim only returns free heap pages to the system.
    unsafe { libc::malloc_trim(0) };
}

/// No heap release on other systems.
#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
fn release_free_memory() {}

/// Observes an activation's caller while the compiler runs and hands a caller that left to the host's `abandon`,
/// which withdraws the worker's socket and ends the process.
struct Abandonment {
    done: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The host requires process teardown after this activation; no observer can outlive that process.
    retiring: bool,
}

impl Abandonment {
    /// Observe the activation's caller, ending with the process only when the host requires one-activation teardown.
    fn watch(watch: CallerWatch, retiring: bool) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let watching = done.clone();
        let thread = std::thread::spawn(move || {
            let Ok(descriptor) = RawFd::try_from(watch.descriptor) else {
                return;
            };
            while !watching.load(Ordering::Acquire) {
                if caller_left(descriptor) && !watching.load(Ordering::Acquire) {
                    incan_worker_host::abandon(watch);
                    return;
                }
            }
        });
        Self {
            done,
            thread: Some(thread),
            retiring,
        }
    }
}

impl Drop for Abandonment {
    /// Disarm before the reply. Resident workers join before reusing descriptors; retiring workers end every thread
    /// with their process, so waiting for the observer's polling timeout serves no purpose.
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && !self.retiring
        {
            let _ = thread.join();
        }
    }
}

/// Wait up to 100 ms for the caller's side of the connection to close; a caller sends nothing more while it waits.
fn caller_left(caller: RawFd) -> bool {
    let mut descriptor = libc::pollfd {
        fd: caller,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: the descriptor is initialized and the host keeps the connection open for the duration of this call.
    if unsafe { libc::poll(&mut descriptor, 1, 100) } <= 0 {
        return false;
    }
    if descriptor.revents & libc::POLLHUP != 0 {
        return true;
    }
    let mut byte = [0_u8; 1];
    // SAFETY: the buffer is one writable byte and the descriptor is the caller's open connection.
    let read = unsafe { libc::recv(caller, byte.as_mut_ptr().cast(), 1, libc::MSG_PEEK) };
    if read > 0 {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    read == 0
}
