fn main() {
    let name = String::from("incan").to_string();
    let copy = name.clone();
    let _pair = (copy.clone(), copy.clone());
    let values = vec![1, 2, 3];
    let _collected = values.iter().map(|value| value + 1).collect::<Vec<_>>();
}
