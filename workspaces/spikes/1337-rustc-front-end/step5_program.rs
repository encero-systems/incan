// `Shape`, `area` and `square` are not declared in this file: the front end injects them.
fn describe(shape: &Shape) -> String {
    match shape {
        Shape::Circle(r) => format!("circle {r}"),
        Shape::Rect(w, h) => format!("rect {w}x{h}"),
        Shape::Empty => "empty".to_string(),
    }
}

fn main() {
    let built = [square(3), square(0)];
    let areas = [area(Shape::Circle(2)), area(Shape::Rect(4, 5)), area(Shape::Empty)];
    println!("{} | {} | {:?}", describe(&built[0]), describe(&built[1]), areas);
}
