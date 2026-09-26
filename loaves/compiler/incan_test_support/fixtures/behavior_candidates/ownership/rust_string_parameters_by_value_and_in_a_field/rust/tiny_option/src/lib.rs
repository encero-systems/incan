//! A struct with a public `String` field, functions taking a `String` by value and one taking a `&str`, after the
//! `build_static_str_const_rust_string_struct_field` CLI test's crate.
pub struct FunctionOption {
    pub name: String,
    pub enabled: bool,
}

/// Take the option and give back its name.
pub fn option_name(option: FunctionOption) -> String {
    option.name
}

/// Take the text and give it back upper-cased.
pub fn shout(text: String) -> String {
    text.to_uppercase()
}

/// Take a borrowed text and give it back in angle brackets.
pub fn describe(text: &str) -> String {
    format!("<{text}>")
}
