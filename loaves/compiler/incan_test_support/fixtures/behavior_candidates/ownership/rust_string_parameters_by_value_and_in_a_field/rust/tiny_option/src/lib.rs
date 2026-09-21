//! A struct with a public `String` field and functions taking a `String` by value, after the
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
