// Step 8's Rust-hosted caller, built by plain rustc. It reaches the Incan library only through RFC 097's caller
// namespace, `<library>::caller::incan`, including a generic function it instantiates itself.
use policy::caller::incan::{Pair, bump, choose, measure, shift};

fn main() {
    let picked = choose(Pair { first: "first", second: "second" }, false);
    println!("{} {} {} {}", bump(41), measure(String::from("incan")), shift(10, 5), picked);
}
