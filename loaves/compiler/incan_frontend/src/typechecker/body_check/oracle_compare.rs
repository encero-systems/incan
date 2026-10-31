//! Full native comparison for cold oracles only; never a schema, identity or production input.

/// Sort unordered Debug containers while preserving variants, sequences, scalars and quoted bytes. This exact test
/// comparator is never used for production identity, storage or semantic projection.
pub(super) fn canonical_debug(value: &str) -> String {
    debug_container(&mut value.chars().peekable(), None, false)
}

/// Consume one delimited Debug container, sorting maps, sets and named fields while retaining every payload value.
fn debug_container(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, close: Option<char>, sort: bool) -> String {
    let mut parts = Vec::new();
    let mut part = String::new();
    while let Some(character) = chars.next() {
        match character {
            '"' | '\'' => {
                part.push(character);
                while let Some(quoted) = chars.next() {
                    part.push(quoted);
                    if quoted == '\\' {
                        if let Some(escaped) = chars.next() {
                            part.push(escaped);
                        }
                    } else if quoted == character {
                        break;
                    }
                }
            }
            '{' | '[' | '(' => {
                let ending = match character {
                    '{' => '}',
                    '[' => ']',
                    _ => ')',
                };
                part.push(character);
                part.push_str(&debug_container(chars, Some(ending), character == '{'));
                part.push(ending);
            }
            ',' if close.is_some() => parts.push(std::mem::take(&mut part).trim().to_owned()),
            ending if Some(ending) == close => break,
            _ => part.push(character),
        }
    }
    if !part.is_empty() {
        parts.push(part.trim().to_owned());
    }
    if sort {
        parts.sort();
    }
    parts.join(",")
}
