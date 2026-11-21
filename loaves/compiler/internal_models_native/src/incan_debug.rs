//! Debug text in the reference models' spelling, so diagnostics that print a record keep their exact bytes.
//!
//! A reference model prints as `Name(field=value, ...)`, each field value in Rust's debug form; enum variants and
//! collections print as Rust prints them.

/// Implement the reference models' debug spelling for a named-field struct, fields in declaration order.
macro_rules! incan_debug {
    ($name:ident { $($field:ident),* $(,)? }) => {
        impl ::std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                formatter.write_str(concat!(stringify!($name), "("))?;
                let mut separator = "";
                $(
                    let name = stringify!($field);
                    write!(formatter, "{separator}{}={:?}", name.strip_prefix("r#").unwrap_or(name), self.$field)?;
                    separator = ", ";
                )*
                let _ = separator;
                formatter.write_str(")")
            }
        }
    };
}

pub(crate) use incan_debug;
