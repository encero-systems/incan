//! The platform's only unsafe Rust: sanctioned host clock, resource-probe, storage-barrier and heap-release bindings
//! behind safe functions.
//!
//! RFC 005 refuses unsafe Rust calls from Incan. This facet contains only the OS calls needed to expose host-aligned
//! nanoseconds and process resource readings, the storage barrier a store publication orders its graph with, and the
//! call that returns freed heap pages to the system; observation, process-tree policy, publication, release policy and
//! encoding stay in Incan. Neither supported clock advances during suspend or follows wall-clock adjustments. Values
//! compare across processes on the same host boot.

#[cfg(target_os = "macos")]
use std::ffi::CStr;
#[cfg(target_os = "macos")]
use std::fmt::Write as _;
use std::io;
#[cfg(target_os = "macos")]
use std::sync::OnceLock;

#[cfg(target_os = "macos")]
type MemoryReader = Option<
    unsafe extern "C" fn(
        libc::task_t,
        libc::vm_address_t,
        libc::vm_size_t,
        *mut *mut libc::c_void,
    ) -> libc::kern_return_t,
>;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn malloc_get_all_zones(
        task: libc::task_t,
        reader: MemoryReader,
        addresses: *mut *mut libc::vm_address_t,
        count: *mut libc::c_uint,
    ) -> libc::kern_return_t;

    /// libmalloc: return free pages of `zone` (every zone when null) to the system, up to `goal` bytes (all when zero).
    fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: libc::size_t) -> libc::size_t;
}

/// Ask the system allocator to return its free heap pages, leaving live allocations untouched.
///
/// The system allocator keeps the pages of freed allocations, and the kernel charges them to the process footprint
/// until the allocator gives them back. macOS asks every malloc zone for pressure relief, which a zone may answer by
/// returning nothing; Linux with glibc trims the heap. When to release is the caller's policy.
#[cfg(target_os = "macos")]
pub fn release_free_heap() {
    // SAFETY: a null zone and a zero goal ask libmalloc to release every free page; no live memory is touched.
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
}

/// Ask the system allocator to return its free heap pages, leaving live allocations untouched.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn release_free_heap() {
    // SAFETY: malloc_trim only returns free heap pages to the system.
    unsafe { libc::malloc_trim(0) };
}

/// Other systems have no heap release; the call returns nothing to the system.
#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
pub fn release_free_heap() {}

/// Read the shared raw monotonic host clock in nanoseconds, excluding time suspended.
///
/// macOS scales `mach_absolute_time` using its timebase; Linux selects `CLOCK_MONOTONIC_RAW`.
/// OS failures, invalid timebases and nanosecond overflow are errors, never fabricated timestamps.
#[cfg(target_os = "macos")]
pub fn raw_uptime_ns() -> io::Result<u64> {
    static TIMEBASE: OnceLock<Result<(u32, u32), i32>> = OnceLock::new();
    let &(numer, denom) = TIMEBASE
        .get_or_init(|| {
            let mut timebase = libc::mach_timebase_info_data_t { numer: 0, denom: 0 };
            // SAFETY: The OS receives a valid, exclusively borrowed output structure of the declared layout.
            let status = unsafe { libc::mach_timebase_info(&mut timebase) };
            if status != libc::KERN_SUCCESS || timebase.numer == 0 || timebase.denom == 0 {
                return Err(status);
            }
            Ok((timebase.numer, timebase.denom))
        })
        .as_ref()
        .map_err(|status| io::Error::other(format!("mach timebase unavailable ({status})")))?;
    // SAFETY: This argument-free OS read has no pointer or caller-state preconditions.
    let ticks = unsafe { libc::mach_absolute_time() };
    let nanos = u128::from(ticks) * u128::from(numer) / u128::from(denom);
    u64::try_from(nanos).map_err(|_| io::Error::other("host clock exceeds u64 nanoseconds"))
}

/// Read this process's current physical footprint in bytes.
///
/// macOS returns the kernel's memory-pressure charge: dirty private pages plus compressed memory. Linux retains the
/// worker lifecycle's existing resident-page approximation until Linux footprint parity is implemented.
#[cfg(target_os = "macos")]
pub fn current_process_footprint() -> io::Result<u64> {
    let pid = libc::c_int::try_from(std::process::id()).map_err(io::Error::other)?;
    // SAFETY: `rusage_info_v4` holds only integers, so the zeroed value is valid.
    let mut usage: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer addresses a writable `rusage_info_v4`, the record the V4 flavor fills.
    if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, (&raw mut usage).cast()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(usage.ri_phys_footprint)
}

/// Read bytes that every macOS malloc zone currently attributes to live allocations.
///
/// This intentionally differs from [`current_process_footprint`]: released allocations stop contributing here even
/// when malloc retains their pages and the kernel continues charging them to the process footprint.
#[cfg(target_os = "macos")]
pub fn current_process_live_allocated() -> io::Result<u64> {
    let mut addresses = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: the current task needs no remote-memory reader. malloc returns an internally owned array valid for this
    // process, and writes its address and element count into the exclusive output pointers.
    let status = unsafe { malloc_get_all_zones(libc::mach_task_self(), None, &raw mut addresses, &raw mut count) };
    if status != libc::KERN_SUCCESS {
        return Err(io::Error::other(format!("malloc zone enumeration failed ({status})")));
    }
    let count = usize::try_from(count).map_err(io::Error::other)?;
    if addresses.is_null() && count != 0 {
        return Err(io::Error::other("malloc zone enumeration returned no addresses"));
    }
    // SAFETY: successful enumeration returned exactly `count` zone addresses, valid in this current process.
    let zones = unsafe { std::slice::from_raw_parts(addresses, count) };
    let mut live = 0_u64;
    for address in zones {
        let mut statistics = libc::malloc_statistics_t {
            blocks_in_use: 0,
            size_in_use: 0,
            max_size_in_use: 0,
            size_allocated: 0,
        };
        // SAFETY: each enumerated address is a live malloc zone and `statistics` is an exclusive output. The
        // allocator owns synchronization for this read-only snapshot.
        unsafe { libc::malloc_zone_statistics(*address as *mut libc::malloc_zone_t, &raw mut statistics) };
        live = live
            .checked_add(u64::try_from(statistics.size_in_use).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("live allocated bytes exceed u64"))?;
    }
    Ok(live)
}

/// Refuse live-allocation observation where the platform has no malloc-zone implementation.
#[cfg(not(target_os = "macos"))]
pub fn current_process_live_allocated() -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "live allocated bytes unavailable",
    ))
}

/// Read this process's current resident memory from `/proc/self/statm`.
#[cfg(target_os = "linux")]
pub fn current_process_footprint() -> io::Result<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm")?;
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| io::Error::other("/proc/self/statm has no resident field"))?
        .parse()
        .map_err(io::Error::other)?;
    // SAFETY: `sysconf` reads a configuration value and has no preconditions.
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).map_err(io::Error::other)?;
    Ok(pages.saturating_mul(page))
}

/// Refuse a footprint reading on a target without an implemented resource probe.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn current_process_footprint() -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "physical footprint unavailable",
    ))
}

/// Return one tab-separated row per live process owned by the current macOS user.
///
/// Each row is `pid`, parent pid, current physical footprint, lifetime maximum physical footprint, resident bytes and
/// the kernel process name. A process that exits between enumeration and either record read is omitted.
#[cfg(target_os = "macos")]
pub fn process_table_text() -> io::Result<String> {
    // SAFETY: a null buffer asks only for the number of processes.
    let needed = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if needed <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pids = vec![0 as libc::pid_t; usize::try_from(needed).map_err(io::Error::other)? + 256];
    let capacity = libc::c_int::try_from(pids.len() * std::mem::size_of::<libc::pid_t>()).map_err(io::Error::other)?;
    // SAFETY: `pids` is writable for `capacity` bytes.
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), capacity) };
    if filled < 0 {
        return Err(io::Error::last_os_error());
    }
    let count = usize::try_from(filled).map_err(io::Error::other)?.min(pids.len());
    // SAFETY: `getuid` has no preconditions.
    let uid = unsafe { libc::getuid() };
    let mut table = String::new();
    for &pid in &pids[..count] {
        if pid <= 0 {
            continue;
        }
        // SAFETY: `proc_bsdinfo` holds only integers and byte arrays, so the zeroed value is valid.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).map_err(io::Error::other)?;
        // SAFETY: `info` is writable for `size` bytes, the record PROC_PIDTBSDINFO fills.
        let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
        if read != size || info.pbi_uid != uid {
            continue;
        }
        // SAFETY: `rusage_info_v4` holds only integers, so the zeroed value is valid.
        let mut usage: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
        // SAFETY: the buffer addresses a writable `rusage_info_v4`, the record the V4 flavor fills.
        if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, (&raw mut usage).cast()) } != 0 {
            continue;
        }
        // SAFETY: the kernel NUL-terminates `pbi_name` within its array; the zeroed buffer ends in NUL otherwise.
        let mut name = unsafe { CStr::from_ptr(info.pbi_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        if name.is_empty() {
            // SAFETY: as above, for the shorter command name.
            name = unsafe { CStr::from_ptr(info.pbi_comm.as_ptr()) }
                .to_string_lossy()
                .into_owned();
        }
        writeln!(
            table,
            "{}\t{}\t{}\t{}\t{}\t{}",
            pid,
            info.pbi_ppid,
            usage.ri_phys_footprint,
            usage.ri_lifetime_max_phys_footprint,
            usage.ri_resident_size,
            name.replace(['\t', '\n'], " ")
        )
        .map_err(io::Error::other)?;
    }
    Ok(table)
}

/// Refuse process-table observation where the platform has no physical-footprint implementation.
#[cfg(not(target_os = "macos"))]
pub fn process_table_text() -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "process resource table unavailable",
    ))
}

/// Read Linux's shared raw monotonic clock; invalid output and nanosecond overflow are errors.
#[cfg(target_os = "linux")]
pub fn raw_uptime_ns() -> io::Result<u64> {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: CLOCK_MONOTONIC_RAW is a valid clock selector and `time` is an exclusive, initialized output.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, &mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let seconds = u64::try_from(time.tv_sec).map_err(|_| io::Error::other("negative host clock"))?;
    let nanos = u64::try_from(time.tv_nsec).map_err(|_| io::Error::other("negative host nanoseconds"))?;
    if nanos >= 1_000_000_000 {
        return Err(io::Error::other("invalid host nanoseconds"));
    }
    seconds
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(nanos))
        .ok_or_else(|| io::Error::other("host clock exceeds u64 nanoseconds"))
}

/// Unsupported targets refuse observation instead of substituting another clock contract.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn raw_uptime_ns() -> io::Result<u64> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "raw host clock unavailable"))
}

/// Order a file's earlier writes on its device ahead of every later write, without flushing the drive's cache.
///
/// Apple filesystems take `F_BARRIERFSYNC`: it synchronizes the file as `fsync` does, then issues a barrier, so
/// writes queued before it reach stable storage before writes queued after it. A filesystem that refuses the barrier
/// gets `F_FULLFSYNC` (`sync_all`) instead, which is stronger, never weaker.
#[cfg(target_os = "macos")]
pub fn barrier_sync(path: &str) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::open(path)?;
    // SAFETY: `file` keeps its descriptor open for the call, and `F_BARRIERFSYNC` takes no third argument.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_BARRIERFSYNC) } != -1 {
        return Ok(());
    }
    file.sync_all()
}

/// Elsewhere `fsync` (`sync_all`) already reaches stable storage in the order it is called.
#[cfg(not(target_os = "macos"))]
pub fn barrier_sync(path: &str) -> io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::{reap_retained_children, retain_child, retained_children};

    /// A child this process retains instead of waiting on is reaped once it exits, so no zombie outlives it.
    #[test]
    fn a_retained_child_is_reaped_once_it_exits() -> Result<(), Box<dyn std::error::Error>> {
        let before = retained_children()?;
        retain_child(std::process::Command::new("/bin/sh").args(["-c", "exit 0"]).spawn()?)?;
        retain_child(std::process::Command::new("/bin/sleep").arg("0.2").spawn()?)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut reaped = 0;
        while reaped < 2 && std::time::Instant::now() < deadline {
            reaped += reap_retained_children()?;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(reaped, 2, "an exited retained child was not reaped");
        assert_eq!(retained_children()?, before);
        Ok(())
    }
}
