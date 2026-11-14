//! Return freed heap pages to the system at the compiler's phase boundaries.
//!
//! The system allocator keeps the pages of freed allocations dirty, and the kernel charges them to the process
//! footprint until the allocator gives them back. A compilation frees its checked program, Body IR and lowering
//! contexts just before rustc starts; without a release here, rustc's analysis and LLVM's code generation allocate on
//! top of those retained pages and the driver's peak counts both. The resident worker also releases after every
//! request, so the footprint its host judges is what the worker retains. Heap pressure relief needs allocator FFI that
//! Incan refuses as unsafe, so it stays in the driver until the platform's allocator-control boundary exists.

#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// libmalloc: return free pages of `zone` (every zone when null) to the system, up to `goal` bytes (all when zero).
    fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: libc::size_t) -> libc::size_t;
}

/// Return every free heap page to the system; live allocations are untouched.
#[cfg(target_os = "macos")]
pub(crate) fn release_free_memory() {
    // SAFETY: a null zone and a zero goal ask libmalloc to release every free page; no memory is touched.
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
}

/// Return every free heap page to the system; live allocations are untouched.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub(crate) fn release_free_memory() {
    // SAFETY: malloc_trim only returns free heap pages to the system.
    unsafe { libc::malloc_trim(0) };
}

/// No heap release on other systems.
#[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
pub(crate) fn release_free_memory() {}
