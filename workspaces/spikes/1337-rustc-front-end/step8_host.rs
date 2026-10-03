// Step 8's Rust dependency: the three Rust signatures from #1337's boundary matrix, built by plain rustc as an rlib.

pub fn increment(value: i64) -> i64 {
    value + 1
}

pub fn text_len(value: &str) -> i64 {
    value.chars().count() as i64
}

pub fn apply(value: i64, callback: impl FnOnce(i64) -> i64) -> i64 {
    callback(value)
}
