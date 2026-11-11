//! Shared source-security facts used by the lexer and every tool that displays source values.

use std::borrow::Cow;

/// Return the Unicode name of a `Bidi_Control` character, or `None` for an ordinary scalar.
pub fn bidirectional_control_name(character: char) -> Option<&'static str> {
    match character {
        '\u{061c}' => Some("ARABIC LETTER MARK"),
        '\u{200e}' => Some("LEFT-TO-RIGHT MARK"),
        '\u{200f}' => Some("RIGHT-TO-LEFT MARK"),
        '\u{202a}' => Some("LEFT-TO-RIGHT EMBEDDING"),
        '\u{202b}' => Some("RIGHT-TO-LEFT EMBEDDING"),
        '\u{202c}' => Some("POP DIRECTIONAL FORMATTING"),
        '\u{202d}' => Some("LEFT-TO-RIGHT OVERRIDE"),
        '\u{202e}' => Some("RIGHT-TO-LEFT OVERRIDE"),
        '\u{2066}' => Some("LEFT-TO-RIGHT ISOLATE"),
        '\u{2067}' => Some("RIGHT-TO-LEFT ISOLATE"),
        '\u{2068}' => Some("FIRST STRONG ISOLATE"),
        '\u{2069}' => Some("POP DIRECTIONAL ISOLATE"),
        _ => None,
    }
}

/// Spell one Unicode scalar as Incan's visible, lowercase source escape.
pub fn unicode_scalar_escape(character: char) -> String {
    format!("\\u{{{:x}}}", u32::from(character))
}

/// Replace every bidirectional control in displayed text with its visible Incan escape.
///
/// The borrowed fast path keeps ordinary diagnostics, hover text and metadata allocation-free. Callers that display
/// source-derived values must use this boundary so an escaped value can never reorder the surrounding tool output.
pub fn escape_bidirectional_controls(text: &str) -> Cow<'_, str> {
    if !text
        .chars()
        .any(|character| bidirectional_control_name(character).is_some())
    {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if bidirectional_control_name(character).is_some() {
            escaped.push_str(&unicode_scalar_escape(character));
        } else {
            escaped.push(character);
        }
    }
    Cow::Owned(escaped)
}

/// Project one source byte offset into the visible text whose controls have expanded to scalar escapes.
pub fn escaped_offset(text: &str, offset: usize) -> usize {
    let bounded = offset.min(text.len());
    let mut projected = 0;
    for (index, character) in text.char_indices() {
        if index >= bounded {
            break;
        }
        projected += match bidirectional_control_name(character) {
            Some(_) => unicode_scalar_escape(character).len(),
            None => character.len_utf8(),
        };
    }
    projected
}

#[cfg(test)]
mod tests {
    use super::{bidirectional_control_name, escape_bidirectional_controls, escaped_offset, unicode_scalar_escape};

    /// The RFC 175 control set is complete and display never returns a raw control.
    #[test]
    fn bidirectional_controls_have_names_and_visible_escapes() {
        let controls = [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}',
            '\u{2067}', '\u{2068}', '\u{2069}',
        ];
        for control in controls {
            assert!(bidirectional_control_name(control).is_some());
            let source = format!("left{control}right");
            let displayed = escape_bidirectional_controls(&source);
            assert_eq!(displayed, format!("left{}right", unicode_scalar_escape(control)));
        }
        assert_eq!(escaped_offset("a\u{202e}b", 4), 9);
    }
}
