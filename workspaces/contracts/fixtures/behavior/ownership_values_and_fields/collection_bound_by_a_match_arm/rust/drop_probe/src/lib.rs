//! Test-only observation of drops and heap use, for programs that bind owned values through patterns.
//!
//! A `Probe` is live from its construction or clone until its drop, and its drop prints its name, so a program shows
//! that each value it bound was dropped exactly once. `live_heap_bytes` reads the C allocator's bytes in use for the
//! whole process, every linked library included, so a loop shows that its iterations leave the heap as they found it.
//! Reading the allocator's statistics is an unsafe C call, which RFC 005 keeps out of Incan source; the bindings, the
//! loops and the drops under test stay in Incan.

use std::sync::atomic::{AtomicI64, Ordering};

static LIVE_PROBES: AtomicI64 = AtomicI64::new(0);

#[cfg(target_os = "macos")]
mod allocator {
    /// `malloc_statistics_t` from `<malloc/malloc.h>`.
    #[repr(C)]
    #[derive(Default)]
    struct MallocStatistics {
        blocks_in_use: u32,
        size_in_use: usize,
        max_size_in_use: usize,
        size_allocated: usize,
    }

    unsafe extern "C" {
        /// Write allocator totals for the selected zone, or every zone when `zone` is null.
        fn malloc_zone_statistics(zone: *mut std::ffi::c_void, statistics: *mut MallocStatistics);
    }

    /// Bytes in use across every malloc zone of this process.
    pub fn bytes_in_use() -> i64 {
        let mut statistics = MallocStatistics::default();
        // SAFETY: a null zone asks for every zone's totals, written into the record this call owns.
        unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut statistics) };
        statistics.size_in_use as i64
    }
}

#[cfg(target_os = "linux")]
mod allocator {
    /// `struct mallinfo2` from glibc's `<malloc.h>`.
    #[repr(C)]
    struct MallocInfo {
        arena: usize,
        ordblks: usize,
        smblks: usize,
        hblks: usize,
        hblkhd: usize,
        usmblks: usize,
        fsmblks: usize,
        uordblks: usize,
        fordblks: usize,
        keepcost: usize,
    }

    unsafe extern "C" {
        /// Read glibc allocator totals for the current process without transferring ownership.
        fn mallinfo2() -> MallocInfo;
    }

    /// Bytes in use in the heap arenas and in separately mapped blocks.
    pub fn bytes_in_use() -> i64 {
        // SAFETY: mallinfo2 takes no arguments and returns its totals by value.
        let info = unsafe { mallinfo2() };
        (info.uordblks + info.hblkhd) as i64
    }
}

/// Bytes the C allocator holds for this process right now.
pub fn live_heap_bytes() -> i64 {
    allocator::bytes_in_use()
}

/// A named value whose drop is visible.
#[derive(Debug)]
pub struct Probe {
    name: String,
}

impl Probe {
    /// A new live probe.
    pub fn new(name: &str) -> Probe {
        LIVE_PROBES.fetch_add(1, Ordering::SeqCst);
        Probe { name: name.to_string() }
    }

    /// The probe's name.
    pub fn name(&self) -> String {
        self.name.clone()
    }
}

impl Clone for Probe {
    /// A copy is another live probe with the same name.
    fn clone(&self) -> Probe {
        Probe::new(&self.name)
    }
}

impl Drop for Probe {
    /// Print the name once per dropped probe.
    fn drop(&mut self) {
        LIVE_PROBES.fetch_sub(1, Ordering::SeqCst);
        println!("dropped {}", self.name);
    }
}

/// Probes constructed or cloned and not yet dropped.
pub fn live_probes() -> i64 {
    LIVE_PROBES.load(Ordering::SeqCst)
}
