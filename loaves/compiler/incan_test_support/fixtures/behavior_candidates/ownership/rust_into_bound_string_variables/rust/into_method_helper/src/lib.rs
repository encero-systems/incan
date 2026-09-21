//! A method generic over `Into<String>`, as the `rust_method_into_bound_keeps_string_argument_inferable_issue804`
//! CLI test builds it.
pub struct Tokenizer;

impl Tokenizer {
    /// A tokenizer with no state.
    pub fn new() -> Self {
        Self
    }

    /// Convert `input` into a `String`, upper-cased when asked.
    pub fn encode<E: Into<String>>(&self, input: E, uppercase: bool) -> String {
        let text = input.into();
        if uppercase {
            text.to_uppercase()
        } else {
            text
        }
    }
}
