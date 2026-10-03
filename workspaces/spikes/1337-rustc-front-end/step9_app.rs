// Step 9's Rust binary, built by the driver under the declared `--incan-allow-rustc-private`. It calls the Incan unit, whose code drives the rustc seam; the seam
// then compiles `step9_target.rs` in this same process, with `answer`'s body planned by the Incan code.
// The root unit names `rustc_driver` itself: rustc links `std` from that shared library only when the executable's
// root depends on it directly, so a binary that embeds rustc must say so at its root.
#![feature(rustc_private)]
extern crate rustc_driver;

use planner::caller::incan::compile_answer;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(program), Some(output), Some(sysroot)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: step9_app <program.rs> <output> <sysroot>");
        std::process::exit(2);
    };
    println!("in-process rustc exit status {}", compile_answer(42, program, output, sysroot));
}
