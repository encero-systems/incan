//! The platform's only unsafe Rust: sanctioned host clock and resource-probe bindings behind safe functions.
//!
//! RFC 005 refuses unsafe Rust calls from Incan. This facet contains only the OS calls needed to expose host-aligned
//! nanoseconds and process resource readings; observation, process-tree policy and encoding stay in Incan. Neither
//! supported clock advances during suspend or follows wall-clock adjustments. Values compare across processes on the
//! same host boot.

#[cfg(target_os = "macos")]
use std::ffi::CStr;
#[cfg(target_os = "macos")]
use std::fmt::Write as _;
use std::io;
#[cfg(target_os = "macos")]
use std::sync::OnceLock;

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
