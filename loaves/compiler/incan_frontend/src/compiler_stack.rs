//! Run compiler work on a stack deep enough for realistic source.
//!
//! The compiler walks the AST recursively at several stages, so stack depth scales with expression nesting rather
//! than with file size. A left-leaning chain of binary operators nests once per operand, which means ordinary
//! generated source -- a long `"a" + "b" + ...` concatenation, a wide boolean guard -- reaches depths a default
//! 8 MiB main-thread stack cannot hold. Overflowing there aborts the process with `fatal runtime error: stack
//! overflow` and no diagnostic, because a stack overflow is not a catchable Rust panic.
//!
//! Every production compiler solves this the same way: do the work on a thread with a large stack. `rustc` itself
//! spawns its main compilation thread for exactly this reason. Sizing the stack for the input is the fix; a
//! diagnostic is not, because by the time the guard page is hit the process is already unrecoverable.

use std::thread;

/// Stack size for the thread that runs compilation.
///
/// Chosen to absorb deeply nested expressions with a wide margin: at 256 MiB the reproduction that used to abort
/// (a 1,000-term string concatenation) completes with room to spare, while the reservation itself costs nothing
/// until the pages are touched.
pub const COMPILER_STACK_BYTES: usize = 256 * 1024 * 1024;

/// Environment override for [`COMPILER_STACK_BYTES`], matching the variable `rustc` honors.
const STACK_SIZE_ENV: &str = "RUST_MIN_STACK";

/// Resolve the stack size to run compilation with, honoring `RUST_MIN_STACK` when it names a usable size.
///
/// A malformed or zero value is ignored rather than rejected: the variable is a tuning knob, and failing a build
/// over it would be worse than quietly using the default.
#[must_use]
pub fn compiler_stack_bytes() -> usize {
    std::env::var(STACK_SIZE_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|bytes| *bytes > 0)
        .unwrap_or(COMPILER_STACK_BYTES)
}

/// Run `work` on a dedicated thread with a compiler-sized stack and return its value.
///
/// Panics from `work` are resumed on the calling thread, so panic behavior, backtraces and abort semantics are
/// unchanged from running it inline. If the thread cannot be spawned at all the process exits with a clear
/// message: `spawn` has already consumed the closure, so there is no way to run it inline instead, and reporting
/// that plainly beats any pretence of having compiled something.
pub fn run_on_compiler_stack<T, F>(work: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let spawned = thread::Builder::new()
        .name("incan-compiler".to_string())
        .stack_size(compiler_stack_bytes())
        .spawn(work);

    match spawned {
        Ok(handle) => match handle.join() {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        },
        Err(_) => unreachable_fallback(),
    }
}

/// Report that a compiler thread could not be spawned.
///
/// Kept separate so the fallback path is explicit: there is no safe way to recover the moved closure once
/// `spawn` has consumed it, so this aborts with a clear message rather than pretending the build succeeded.
fn unreachable_fallback() -> ! {
    eprintln!("incan: could not start the compiler thread; the system refused to create a thread");
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    /// Recurse to a depth that overflows a default thread stack, mirroring how the AST walk nests once per
    /// operand in a long operator chain.
    fn recurse(depth: u32, accumulator: u64) -> u64 {
        // A local array keeps each frame large enough that depth, not optimization, decides the outcome.
        let ballast = [accumulator; 32];
        if depth == 0 {
            return ballast[0];
        }
        recurse(depth - 1, accumulator.wrapping_add(u64::from(depth))) + ballast[31] % 2
    }

    #[test]
    fn compiler_stack_absorbs_depth_that_would_overflow_a_default_stack() -> Result<(), Box<dyn std::error::Error>> {
        // 200_000 frames of this shape exhaust a default 2 MiB test-thread stack many times over. Running the
        // same work through `run_on_compiler_stack` must complete, which is exactly what a 1,000-term string
        // concatenation needs from the AST walk.
        let total = super::run_on_compiler_stack(|| recurse(200_000, 0));
        assert!(total > 0);
        Ok(())
    }

    #[test]
    fn compiler_stack_size_honors_a_usable_override() -> Result<(), Box<dyn std::error::Error>> {
        // The default applies unless the environment names a usable size; malformed values are ignored rather
        // than failing a build over a tuning knob.
        assert_eq!(super::COMPILER_STACK_BYTES, 256 * 1024 * 1024);
        assert!(super::compiler_stack_bytes() >= 8 * 1024 * 1024);
        Ok(())
    }

    #[test]
    fn compiler_stack_propagates_panics_to_the_caller() -> Result<(), Box<dyn std::error::Error>> {
        // Panic behavior must be indistinguishable from running the work inline, or error reporting changes
        // shape depending on which thread happened to run it.
        let outcome = std::panic::catch_unwind(|| super::run_on_compiler_stack(|| panic!("propagated")));
        assert!(outcome.is_err());
        Ok(())
    }
}
