// `Point`, `manhattan` and `make_point` are not declared in this file: the front end injects them.
fn main() {
    let p = Point { x: 3, y: 4 };
    let q = make_point(5);
    println!("{} {} {}", manhattan(p), q.x, q.y);
}
