//! Special enum helper method spellings.

/// Stable identifier for enum helper methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnumHelperId {
    Message,
}

/// Resolve a helper method name to its id.
pub fn from_str(name: &str) -> Option<EnumHelperId> {
    match name {
        "message" => Some(EnumHelperId::Message),
        _ => None,
    }
}

/// Return the canonical spelling for an enum helper method.
pub fn as_str(id: EnumHelperId) -> &'static str {
    match id {
        EnumHelperId::Message => "message",
    }
}
