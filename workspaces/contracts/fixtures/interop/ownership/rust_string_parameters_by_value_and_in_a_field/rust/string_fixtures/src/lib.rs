//! Rust APIs taking text in the shapes an Incan program hands it: by value, borrowed, inside a struct field and through
//! an `Into<String>` bound.

use std::marker::PhantomData;

/// A public field beside private storage, without lifetime parameters.
pub struct Mixed {
    hidden: u8,
    /// Readable output.
    pub code: String,
}

/// A lifetime owner with all fields public.
pub struct PublicLifetime<'a> {
    /// Readable output.
    pub code: String,
    /// Lifetime marker.
    pub marker: PhantomData<&'a ()>,
}

/// A public field beside a private lifetime marker.
pub struct MixedLifetime<'a> {
    /// Readable output.
    pub code: String,
    marker: PhantomData<&'a ()>,
}

/// Public fields remain readable when cross-crate construction is forbidden.
#[non_exhaustive]
pub struct NonExhaustive {
    /// Readable output.
    pub code: String,
}

/// An input conversion whose anonymous binder includes a lifetime and a type argument.
pub trait IntoField<'a, T> {
    /// Return the owned field value.
    fn into_field(self) -> T;
}

impl<'a, T> IntoField<'a, T> for T {
    /// Preserve an input that already has the field's type.
    fn into_field(self) -> T {
        self
    }
}

impl Mixed {
    /// Construct mixed storage through a public associated function with an anonymous generic input.
    pub fn new<'a>(value: impl IntoField<'a, String>) -> Self {
        Self {
            code: value.into_field(),
            hidden: 0,
        }
    }
}

/// Return mixed storage without lifetime parameters.
pub fn mixed() -> Mixed {
    Mixed {
        code: "mixed".into(),
        hidden: 0,
    }
}

/// Return fully public storage with a lifetime parameter.
pub fn public_lifetime<'a>() -> PublicLifetime<'a> {
    PublicLifetime {
        code: "public lifetime".into(),
        marker: PhantomData,
    }
}

/// Return mixed storage with a lifetime parameter.
pub fn mixed_lifetime<'a>() -> MixedLifetime<'a> {
    MixedLifetime {
        code: "mixed lifetime".into(),
        marker: PhantomData,
    }
}

/// Return a value whose public fields do not authorize construction.
pub fn non_exhaustive() -> NonExhaustive {
    NonExhaustive {
        code: "non-exhaustive".into(),
    }
}

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
