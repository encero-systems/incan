//! A level defined in a private module and named outside it only through its re-export.

mod private {
    /// How deep something sits.
    pub struct Level {
        /// The depth.
        pub depth: i64,
    }
}

pub use private::Level;
