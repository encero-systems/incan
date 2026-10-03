// `route` is not declared in this file: the front end injects it. Every `Tracked` is counted when it is dropped, so
// the program can prove each value was dropped exactly once on every path, normal and unwinding.
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};

static DROPS: AtomicUsize = AtomicUsize::new(0);

struct Tracked(i64);

impl Drop for Tracked {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

fn may_panic(item: &Tracked, fail: bool) -> i64 {
    if fail {
        panic!("may_panic failed");
    }
    item.0
}

fn consume(item: Tracked) -> i64 {
    item.0
}

fn main() {
    panic::set_hook(Box::new(|_| {}));
    let taken = route(Tracked(5), true, false);
    let after_taken = DROPS.load(Ordering::SeqCst);
    let kept = route(Tracked(7), false, false);
    let after_kept = DROPS.load(Ordering::SeqCst);
    let unwound = panic::catch_unwind(AssertUnwindSafe(|| route(Tracked(9), true, true))).is_err();
    let after_unwind = DROPS.load(Ordering::SeqCst);
    println!("{taken} {kept} unwound={unwound} drops={after_taken},{after_kept},{after_unwind}");
}
