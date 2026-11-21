//! The checked inputs that key one ordinary frame's plan-function product.

/// The immutable producer profile for this request's selected product directory.
pub fn producer_generation(products: Option<&str>) -> Option<String> {
    let directory = products?;
    std::fs::read(std::path::Path::new(directory).join("producer-profile"))
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}
