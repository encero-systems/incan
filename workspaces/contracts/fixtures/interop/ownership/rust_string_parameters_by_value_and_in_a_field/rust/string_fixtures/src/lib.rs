//! Rust APIs taking text in the shapes an Incan program hands it: by value, borrowed, inside a struct field and through
//! an `Into<String>` bound.

/// A struct with a public `String` field, functions taking a `String` by value and one taking a `&str`, after the
/// `build_static_str_const_rust_string_struct_field` CLI test's crate.
pub mod tiny_option {
    /// An option with a public name.
    pub struct FunctionOption {
        /// The option's name.
        pub name: String,
        /// Whether the option is enabled.
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
}

/// A method generic over `Into<String>`, as the `rust_method_into_bound_keeps_string_argument_inferable_issue804`
/// CLI test builds it.
pub mod into_method_helper {
    /// A tokenizer without state.
    pub struct Tokenizer;

    impl Tokenizer {
        /// A tokenizer with no state.
        pub fn new() -> Self {
            Self
        }

        /// Convert `input` into a `String`, upper-cased when asked.
        pub fn encode<E: Into<String>>(&self, input: E, uppercase: bool) -> String {
            let text = input.into();
            if uppercase { text.to_uppercase() } else { text }
        }
    }
}
