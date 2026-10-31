//! The platform's only unsafe Rust: a sanctioned raw host-clock binding behind one safe function.
//!
//! RFC 005 refuses unsafe Rust calls from Incan. This facet contains only the OS calls needed to expose
//! host-aligned nanoseconds; observation policy and encoding stay in Incan. Neither supported clock advances
//! during suspend or follows wall-clock adjustments. Values compare across processes on the same host boot.

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
