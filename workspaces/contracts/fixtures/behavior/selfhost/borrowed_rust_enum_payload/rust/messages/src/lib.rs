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
