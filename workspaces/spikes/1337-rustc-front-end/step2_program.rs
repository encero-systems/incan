// Declaration only. The body below is never used: the front end supplies `answer`'s MIR.
fn answer() -> i64 { loop {} }

fn main() { println!("answer() = {}", answer()); }
