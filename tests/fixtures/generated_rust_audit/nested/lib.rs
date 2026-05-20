pub fn render(items: Vec<String>) -> String {
    let mut out = String::new();
    let mapped = items.iter().map(|item| item.to_owned()).collect();
    out.push_str(&format!("{:?}", mapped));
    out
}
