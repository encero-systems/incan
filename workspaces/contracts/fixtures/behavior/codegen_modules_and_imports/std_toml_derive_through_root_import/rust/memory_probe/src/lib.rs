//! Test-only process resource measurement for #2426; remove this bridge when Incan can call the host API safely.
//!
//! RFC 005 excludes unsafe Rust calls, including `libc::getrusage` and `std::mem::zeroed`, even inside `unsafe:`.
//! The contract's allocation and iteration remain in Incan; this bridge only reads the host's resident high-water mark.

/// Return this process's peak resident set in bytes on macOS and Linux without invoking a process-inspection command.
pub fn peak_resident_bytes() -> std::io::Result<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the complete output on success; an error returns before reading that storage.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful getrusage call above initialized this value.
    let usage = unsafe { usage.assume_init() };
    let peak = u64::try_from(usage.ru_maxrss).map_err(std::io::Error::other)?;
    if cfg!(target_os = "macos") {
        return Ok(peak);
    }
    peak.checked_mul(1024)
        .ok_or_else(|| std::io::Error::other("peak resident byte count overflowed"))
}
