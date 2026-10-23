/// Native input for #2256's enum-pattern contract; the consumer and its observable behavior are authored in Incan.
#[derive(Clone, Debug)]
pub enum Message {
    /// A record whose fields can be selected independently of written pattern order.
    Record {
        /// The numeric payload.
        count: i64,
        /// The text payload.
        text: String,
        /// A false Boolean proves that payload reads preserve the value, rather than pointer truthiness.
        ready: bool,
    },
    /// A message without payload storage.
    Empty,
    /// An unselected recursive carrier still requires its complete native layout.
    Wrapped(Box<Message>),
    /// A nested Box must use the same storage arity as the direct boxed payload.
    Optional(Option<Box<Message>>),
    /// A named boxed field preserves the actual Box carrier when bound.
    NamedBox {
        /// The native owner exposed to the source binding.
        inner: Box<Message>,
    },
}

impl std::fmt::Display for Message {
    /// Distinguish the native Display implementation from Debug's record representation.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("native message")
    }
}

impl PartialEq for Message {
    /// Treat Empty and Wrapped as one equivalence class, proving that native equality does not compare tags.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Empty | Self::Wrapped(_), Self::Empty | Self::Wrapped(_)) => true,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}

/// Supply a concrete foreign record without making construction part of the pattern contract.
pub fn message() -> Message {
    Message::Record {
        count: 7,
        text: "native".into(),
        ready: false,
    }
}

/// Expose a native Box method signature beside the enum's boxed payload layout.
pub fn boxed_message() -> Box<Message> {
    Box::new(message())
}

/// A public native field retains Box's defaulted allocator in rustc's complete field type.
pub struct Envelope {
    /// The same Box carrier that appears with its omitted default in the enum payload.
    pub message: Box<Message>,
}

impl Message {
    /// Return a shared enum reference inside an optional native result.
    pub fn borrowed_message(&self) -> Option<&Message> {
        Some(self)
    }
}

/// Supply the public native field without requiring native record construction in the consumer.
pub fn message_envelope() -> Envelope {
    Envelope {
        message: boxed_message(),
    }
}

/// Supply a named Box payload for a pattern that uses the native Box API.
pub fn wrapped_message() -> Message {
    Message::NamedBox { inner: boxed_message() }
}

/// A Copy receiver whose Clone implementation exposes an accidental substitution of cloning for copying.
#[derive(Copy)]
pub struct CopyNumber {
    /// Public scalar storage permits the native layout to be verified completely.
    pub value: i64,
}

impl Clone for CopyNumber {
    /// Make a wrong ownership adaptation observable without changing the copied value.
    fn clone(&self) -> Self {
        println!("unexpected Clone");
        Self { value: self.value }
    }
}

impl CopyNumber {
    /// Read a Copy receiver by value while borrowed callers retain their original owner.
    pub fn into_value(self) -> i64 {
        self.value
    }
}

/// The declaration path differs from the public path used by the consumer's pattern.
pub mod category {
    /// A fieldless native enum retained through a public re-export and an optional return type.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Kind {
        /// The first branch of the native function.
        Present,
        /// The other branch proves that both selected variants retain their identities.
        Missing,
    }
}

pub use category::Kind;

/// Return the re-exported native carrier inside a structural Option.
pub fn category(present: bool) -> Option<Kind> {
    Some(if present { Kind::Present } else { Kind::Missing })
}
