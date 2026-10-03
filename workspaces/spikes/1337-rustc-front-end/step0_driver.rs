#![feature(rustc_private)]
extern crate rustc_driver;

struct Callbacks;
impl rustc_driver::Callbacks for Callbacks {}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    rustc_driver::run_compiler(&args, &mut Callbacks);
}
