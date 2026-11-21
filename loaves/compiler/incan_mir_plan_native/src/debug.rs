//! Incan's structural `Debug` text for plan records: `Name(field=value, ...)`, each value in its own `Debug` form.
//!
//! Plan records were Incan models, whose implicit `Debug` writes the source call form rather than Rust's braced
//! struct form; driver diagnostics that print a plan record keep that text.

/// Implement Incan's structural `Debug` text on a plan record from its source name and declaration-order fields.
#[macro_export]
#[doc(hidden)]
macro_rules! structural_debug {
    ($type:ty, $name:literal, $first:ident $(, $field:ident)* $(,)?) => {
        impl ::std::fmt::Debug for $type {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str(concat!($name, "(", stringify!($first), "="))?;
                ::std::fmt::Debug::fmt(&self.$first, formatter)?;
                $(
                    formatter.write_str(concat!(", ", stringify!($field), "="))?;
                    ::std::fmt::Debug::fmt(&self.$field, formatter)?;
                )*
                formatter.write_str(")")
            }
        }
    };
}
