//! Canonical RFC 119 target predicates shared by authored dependency and build-fact admission.

/// Validate an exact target triple or canonical `cfg(...)` without evaluating host facts.
pub fn validate(value: &str) -> Result<(), String> {
    let exact = value.split('-').count() >= 3
        && value.split('-').all(|part| !part.is_empty())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if exact {
        return Ok(());
    }
    let inner = value
        .strip_prefix("cfg(")
        .and_then(|inner| inner.strip_suffix(')'))
        .ok_or_else(|| format!("target `{value}` must be an exact target triple or canonical cfg(...)"))?;
    let mut remaining = inner;
    parse_predicate(&mut remaining, 0)?;
    if !remaining.is_empty() {
        return Err(format!("target cfg predicate has trailing construct `{remaining}`"));
    }
    Ok(())
}

/// Parse one canonical predicate and bound nesting before descending into user-authored input.
fn parse_predicate(input: &mut &str, depth: usize) -> Result<(), String> {
    if depth > 64 {
        return Err("target cfg predicate nesting exceeds 64".to_string());
    }
    let length = input
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    let (name, rest) = input.split_at(length);
    if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
        return Err(format!("target cfg predicate requires an identifier at `{input}`"));
    }
    *input = rest;
    if let Some(rest) = input.strip_prefix('(') {
        if !["all", "any", "not"].contains(&name) {
            return Err(format!("target cfg predicate has unknown construct `{name}`"));
        }
        *input = rest;
        let mut count = 0;
        if !input.starts_with(')') {
            loop {
                parse_predicate(input, depth + 1)?;
                count += 1;
                if let Some(rest) = input.strip_prefix(", ") {
                    *input = rest;
                } else {
                    break;
                }
            }
        }
        *input = input
            .strip_prefix(')')
            .ok_or_else(|| "target cfg arguments require canonical `, ` separators and closing `)`".to_string())?;
        if name == "not" && count != 1 {
            return Err("target cfg not(...) requires exactly one argument".to_string());
        }
    } else if let Some(rest) = input.strip_prefix(" = \"") {
        let mut escaped = false;
        let mut end = None;
        for (offset, character) in rest.char_indices() {
            if escaped {
                if !matches!(character, '\\' | '"' | 'n' | 'r' | 't' | '0') {
                    return Err("target cfg value has an unsupported escape".to_string());
                }
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                end = Some(offset);
                break;
            } else if character.is_control() {
                return Err("target cfg value contains a control character".to_string());
            }
        }
        let end = end.ok_or_else(|| "target cfg value requires a closing quote".to_string())?;
        *input = &rest[end + 1..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Admission checks both canonical syntax and exact triples.
    #[test]
    fn amended_loaf_target_conditions_are_canonical() {
        for value in [
            "aarch64-apple-darwin",
            "cfg(unix)",
            "cfg(all(unix, not(target_os = \"macos\")))",
            "cfg(any())",
        ] {
            assert!(super::validate(value).is_ok(), "{value}");
        }
        for value in [
            "linux",
            "cfg()",
            "cfg(unix )",
            "cfg(all(unix,windows))",
            "cfg(not(unix, windows))",
            "cfg(foo(unix))",
            "cfg(target_os=\"linux\")",
            "cfg(unix)junk",
            "cfg(target_os = \"linux)",
        ] {
            assert!(super::validate(value).is_err(), "{value}");
        }
    }
}
