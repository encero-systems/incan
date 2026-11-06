//! Callable and `AsFd` bounds read from Rust function source text.

use super::*;

/// Return whether `display` is a Rust callable bound display such as `impl FnMut(&mut T)`.
#[must_use]
pub fn rust_display_is_callable_bound(display: &str) -> bool {
    let mut display = display.trim();
    if let Some(rest) = display.strip_prefix("impl")
        && rust_source_keyword_boundary(display, "impl".len())
    {
        display = rest.trim_start();
    } else if let Some(rest) = display.strip_prefix("dyn")
        && rust_source_keyword_boundary(display, "dyn".len())
    {
        display = rest.trim_start();
    }
    split_top_level_rust_bounds(display)
        .into_iter()
        .any(rust_callable_bound_has_signature)
}

/// Return a source function's callable `Fn*` bound for a generic parameter, if one is declared.
#[must_use]
pub fn rust_source_callable_bound_for_type_param<F>(
    function_source: &str,
    type_param: &str,
    mut normalize_type: F,
) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let type_param = rust_simple_type_param_name(type_param)?;
    let header = rust_source_function_header(function_source)?;
    if let Some(generic_params) = header.generic_params {
        for generic in split_top_level_rust_args(generic_params) {
            let Some((name, bounds)) = generic.split_once(':') else {
                continue;
            };
            if name.trim() == type_param {
                for bound in split_top_level_rust_bounds(bounds) {
                    if let Some(display) = rust_source_callable_bound_display(bound, &mut normalize_type) {
                        return Some(display);
                    }
                }
            }
        }
    }

    let where_idx = find_top_level_rust_keyword(header.tail, "where")?;
    for predicate in split_top_level_rust_args(&header.tail[where_idx + "where".len()..]) {
        let Some((name, bounds)) = predicate.split_once(':') else {
            continue;
        };
        if name.trim() == type_param {
            for bound in split_top_level_rust_bounds(bounds) {
                if let Some(display) = rust_source_callable_bound_display(bound, &mut normalize_type) {
                    return Some(display);
                }
            }
        }
    }
    None
}

/// Return whether a source function's generic parameter is constrained by Rust's `AsFd` trait.
///
/// `AsFd` exposes a borrowed file descriptor, so an Incan call may pass a retained descriptor by shared borrow even
/// when the Rust function writes its parameter as a by-value generic. Rust's source signature does not preserve that
/// ownership distinction after generic substitution; keep the constraint available to interop metadata instead of
/// forcing Incan source to introduce a Rust-owned wrapper merely to retain the descriptor.
#[must_use]
pub fn rust_source_type_param_has_as_fd_bound(function_source: &str, type_param: &str) -> bool {
    let Some(type_param) = rust_simple_type_param_name(type_param) else {
        return false;
    };
    let Some(header) = rust_source_function_header(function_source) else {
        return false;
    };

    let has_as_fd_bound = |bounds: &str| {
        split_top_level_rust_bounds(bounds).into_iter().any(|bound| {
            bound
                .trim()
                .rsplit("::")
                .next()
                .is_some_and(|name| name.trim() == "AsFd")
        })
    };

    if let Some(generic_params) = header.generic_params {
        for generic in split_top_level_rust_args(generic_params) {
            let Some((name, bounds)) = generic.split_once(':') else {
                continue;
            };
            if name.trim() == type_param && has_as_fd_bound(bounds) {
                return true;
            }
        }
    }

    let Some(where_idx) = find_top_level_rust_keyword(header.tail, "where") else {
        return false;
    };
    split_top_level_rust_args(&header.tail[where_idx + "where".len()..])
        .into_iter()
        .filter_map(|predicate| predicate.split_once(':'))
        .any(|(name, bounds)| name.trim() == type_param && has_as_fd_bound(bounds))
}

/// Preserve a borrowed generic parameter's source trait bounds as an `&impl Trait` metadata display.
///
/// Rust-analyzer can erase a source parameter such as `value: &S` into an inferred concrete display. That loses the
/// borrow boundary Incan must reproduce when calling the API. Source metadata is authoritative here: retain the
/// borrowed shape and the generic bounds so typechecking records a shared or mutable borrow for code generation.
#[must_use]
pub fn rust_source_borrowed_type_param_bound_display(function_source: &str, parameter_type: &str) -> Option<String> {
    let (is_mut, type_param) = rust_borrowed_simple_type_param(parameter_type)?;
    let header = rust_source_function_header(function_source)?;

    let bounds = header
        .generic_params
        .and_then(|declarations| rust_source_type_param_bounds(declarations, type_param))
        .or_else(|| {
            let where_idx = find_top_level_rust_keyword(header.tail, "where")?;
            rust_source_type_param_bounds(&header.tail[where_idx + "where".len()..], type_param)
        })?;
    let bounds = split_top_level_rust_bounds(bounds)
        .into_iter()
        .filter(|bound| {
            let bound = bound.trim();
            !bound.is_empty() && !bound.starts_with('?')
        })
        .collect::<Vec<_>>();
    if bounds.is_empty() {
        return None;
    }
    let borrow = if is_mut { "&mut " } else { "&" };
    Some(format!("{borrow}impl {}", bounds.join(" + ")))
}

pub(super) struct RustSourceFunctionHeader<'a> {
    generic_params: Option<&'a str>,
    tail: &'a str,
}

/// Recover the generic parameter list and post-parameter header tail from a Rust function item source string.
pub(super) fn rust_source_function_header(function_source: &str) -> Option<RustSourceFunctionHeader<'_>> {
    for (fn_idx, _) in function_source.match_indices("fn") {
        if !rust_source_keyword_at(function_source, fn_idx, "fn") {
            continue;
        }
        let mut idx = skip_rust_source_whitespace(function_source, fn_idx + "fn".len());
        if let Some(rest) = function_source.get(idx..).and_then(|rest| rest.strip_prefix("r#")) {
            idx = function_source.len() - rest.len();
        }
        let Some(after_name) = skip_rust_source_ident(function_source, idx) else {
            continue;
        };
        idx = after_name;
        idx = skip_rust_source_whitespace(function_source, idx);
        let Some(rest) = function_source.get(idx..) else {
            continue;
        };
        let generic_params = if rest.starts_with('<') {
            let Some(generic_end) = matching_rust_angle_end(function_source, idx) else {
                continue;
            };
            let params = &function_source[idx + 1..generic_end];
            idx = skip_rust_source_whitespace(function_source, generic_end + 1);
            Some(params)
        } else {
            None
        };
        let Some(rest) = function_source.get(idx..) else {
            continue;
        };
        if !rest.starts_with('(') {
            continue;
        }
        let Some(params_end) = matching_rust_paren_end(function_source, idx) else {
            continue;
        };
        let tail_start = params_end + 1;
        let Some(tail) = function_source.get(tail_start..) else {
            continue;
        };
        let tail_end = find_top_level_rust_header_end(tail).unwrap_or(tail.len());
        return Some(RustSourceFunctionHeader {
            generic_params,
            tail: &tail[..tail_end],
        });
    }
    None
}

/// Return a single uppercase generic type-parameter identifier when `text` has no path or compound type syntax.
pub(super) fn rust_simple_type_param_name(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    (!trimmed.is_empty()
        && !trimmed.contains("::")
        && !trimmed.contains(['<', '>', '(', ')', '[', ']', '&', ' '])
        && trimmed.chars().all(rust_source_ident_char)
        && trimmed.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()))
    .then_some(trimmed)
}

/// Return mutability plus a generic parameter name from a Rust source borrow such as `&S` or `&'a mut S`.
pub(super) fn rust_borrowed_simple_type_param(text: &str) -> Option<(bool, &str)> {
    let mut text = text.trim().strip_prefix('&')?.trim_start();
    if text.starts_with('\'') {
        let end = text.find(char::is_whitespace)?;
        text = text[end..].trim_start();
    }
    let (is_mut, text) = if let Some(rest) = text.strip_prefix("mut")
        && rest.chars().next().is_none_or(char::is_whitespace)
    {
        (true, rest.trim_start())
    } else {
        (false, text)
    };
    Some((is_mut, rust_simple_type_param_name(text)?))
}

/// Find the trait-bound text assigned to one generic parameter in an inline or `where` declaration list.
pub(super) fn rust_source_type_param_bounds<'a>(declarations: &'a str, type_param: &str) -> Option<&'a str> {
    for declaration in split_top_level_rust_args(declarations) {
        let (name, bounds) = declaration.split_once(':')?;
        if name.trim() == type_param {
            return Some(bounds);
        }
    }
    None
}

/// Return whether one top-level bound starts with a callable trait and a parenthesized argument list.
pub(super) fn rust_callable_bound_has_signature(bound: &str) -> bool {
    let Some((_, after_name)) = rust_callable_bound_name_and_tail(bound) else {
        return false;
    };
    after_name.starts_with('(') && matching_rust_paren_end(after_name, 0).is_some()
}

/// Normalize one source `Fn*` bound into the stable `impl Fn*(...)` display form used by Rust metadata.
pub(super) fn rust_source_callable_bound_display<F>(bound: &str, mut normalize_type: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let (name, after_name) = rust_callable_bound_name_and_tail(bound)?;
    if !after_name.starts_with('(') {
        return None;
    }
    let args_end = matching_rust_paren_end(after_name, 0)?;
    let args_inner = &after_name[1..args_end];
    let mut args = Vec::new();
    for arg in split_top_level_rust_args(args_inner) {
        args.push(normalize_type(arg)?);
    }
    let after_args = after_name[args_end + 1..].trim_start();
    let ret = if let Some(ret) = after_args.strip_prefix("->") {
        Some(normalize_type(
            split_top_level_rust_bounds(ret).first().copied().unwrap_or(ret),
        )?)
    } else {
        None
    };
    Some(match ret {
        Some(ret) => format!("impl {name}({}) -> {ret}", args.join(", ")),
        None => format!("impl {name}({})", args.join(", ")),
    })
}

/// Split a Rust callable bound into its unqualified callable trait name and the remaining signature text.
pub(super) fn rust_callable_bound_name_and_tail(bound: &str) -> Option<(&'static str, &str)> {
    let bound = bound.trim();
    for name in ["FnOnce", "FnMut", "Fn"] {
        if let Some(rest) = bound.strip_prefix(name) {
            return Some((name, rest.trim_start()));
        }
        for prefix in ["std::ops::", "core::ops::"] {
            if let Some(rest) = bound.strip_prefix(prefix).and_then(|rest| rest.strip_prefix(name)) {
                return Some((name, rest.trim_start()));
            }
        }
    }
    None
}

/// Split Rust trait bounds joined with top-level `+` without splitting inside generic or function syntax.
pub(super) fn split_top_level_rust_bounds(text: &str) -> Vec<&str> {
    let mut bounds = Vec::new();
    let mut start = 0usize;
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' if !text[..idx].ends_with('-') => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            '+' if angle == 0 && paren == 0 && bracket == 0 => {
                let bound = text[start..idx].trim();
                if !bound.is_empty() {
                    bounds.push(bound);
                }
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        bounds.push(tail);
    }
    bounds
}

/// Return the matching `>` for a generic parameter list, ignoring arrows inside callable return types.
pub(super) fn matching_rust_angle_end(text: &str, open_idx: usize) -> Option<usize> {
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    for (idx, ch) in text[open_idx..].char_indices() {
        match ch {
            '<' if paren == 0 && bracket == 0 => angle += 1,
            '>' if paren == 0 && bracket == 0 && previous_rust_source_char(text, open_idx + idx) != Some('-') => {
                angle = angle.checked_sub(1)?;
                if angle == 0 {
                    return Some(open_idx + idx);
                }
            }
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// Return the matching `)` for a parenthesized Rust source region.
pub(super) fn matching_rust_paren_end(text: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in text[open_idx..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open_idx + idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the top-level `{` or `;` that ends a Rust function header.
pub(super) fn find_top_level_rust_header_end(text: &str) -> Option<usize> {
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            '{' | ';' if angle == 0 && paren == 0 && bracket == 0 => return Some(idx),
            _ => {}
        }
    }
    None
}

/// Find a keyword token outside nested generic, function, and slice delimiters.
pub(super) fn find_top_level_rust_keyword(text: &str, keyword: &str) -> Option<usize> {
    let mut angle = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '<' => angle += 1,
            '>' => angle = angle.saturating_sub(1),
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            _ if angle == 0
                && paren == 0
                && bracket == 0
                && text[idx..].starts_with(keyword)
                && rust_source_keyword_at(text, idx, keyword) =>
            {
                return Some(idx);
            }
            _ => {}
        }
    }
    None
}

/// Skip Rust source whitespace starting at `idx`.
pub(super) fn skip_rust_source_whitespace(text: &str, mut idx: usize) -> usize {
    while let Some(ch) = text.get(idx..).and_then(|rest| rest.chars().next()) {
        if !ch.is_whitespace() {
            break;
        }
        idx += ch.len_utf8();
    }
    idx
}

/// Skip a simple Rust identifier starting at `idx`.
pub(super) fn skip_rust_source_ident(text: &str, mut idx: usize) -> Option<usize> {
    let mut saw_ident = false;
    while let Some(ch) = text.get(idx..).and_then(|rest| rest.chars().next()) {
        if !rust_source_ident_char(ch) {
            break;
        }
        saw_ident = true;
        idx += ch.len_utf8();
    }
    saw_ident.then_some(idx)
}

/// Return whether `keyword` starts at `idx` with identifier-token boundaries on both sides.
pub(super) fn rust_source_keyword_at(text: &str, idx: usize, keyword: &str) -> bool {
    text.get(idx..).is_some_and(|rest| rest.starts_with(keyword))
        && previous_rust_source_char(text, idx).is_none_or(|ch| !rust_source_ident_char(ch))
        && rust_source_keyword_boundary(text, idx + keyword.len())
}

/// Return whether `idx` is at an identifier boundary in Rust source text.
pub(super) fn rust_source_keyword_boundary(text: &str, idx: usize) -> bool {
    text.get(idx..)
        .and_then(|rest| rest.chars().next())
        .is_none_or(|ch| !rust_source_ident_char(ch))
}

/// Return whether `ch` belongs to the simple ASCII identifier vocabulary this source scanner accepts.
pub(super) fn rust_source_ident_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

/// Return the source character immediately before `idx`.
pub(super) fn previous_rust_source_char(text: &str, idx: usize) -> Option<char> {
    text.get(..idx).and_then(|prefix| prefix.chars().next_back())
}
