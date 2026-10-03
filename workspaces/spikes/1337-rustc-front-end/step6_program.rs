// `Pair`, `choose` and `larger_of` are not declared in this file: the front end injects them.
fn main() {
    let word = choose(Pair { first: "incan", second: "rust" }, true);
    let number = choose(Pair { first: 1.5, second: 2.5 }, false);
    println!("{} {} {}", word, number, larger_of(3, 9));
}
