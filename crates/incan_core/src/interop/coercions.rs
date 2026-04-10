//! Built-in coercion matrix between Incan built-ins and Rust boundary targets (RFC 041).

/// Policy class for an admitted coercion edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoercionPolicy {
    /// Canonical exact lowering (`int -> i64`, `bool -> bool`, etc.).
    Exact,
    /// Borrow-based adaptation (`str -> &str`, `bytes -> &[u8]`).
    Borrow,
    /// Explicitly admitted lossy adaptation (`float -> f32` in the initial RFC matrix).
    Lossy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IncanBoundaryType {
    Primitive(String),
    Option(Box<IncanBoundaryType>),
    Result(Box<IncanBoundaryType>, Box<IncanBoundaryType>),
    Tuple(Vec<IncanBoundaryType>),
    List(Box<IncanBoundaryType>),
    Dict(Box<IncanBoundaryType>, Box<IncanBoundaryType>),
    Set(Box<IncanBoundaryType>),
    FrozenList(Box<IncanBoundaryType>),
    FrozenDict(Box<IncanBoundaryType>, Box<IncanBoundaryType>),
    FrozenSet(Box<IncanBoundaryType>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RustBoundaryType {
    Primitive(String),
    Option(Box<RustBoundaryType>),
    Result(Box<RustBoundaryType>, Box<RustBoundaryType>),
    Tuple(Vec<RustBoundaryType>),
    Vec(Box<RustBoundaryType>),
    HashMap(Box<RustBoundaryType>, Box<RustBoundaryType>),
    HashSet(Box<RustBoundaryType>),
}

/// Return the policy for an admitted builtin edge, or `None` when no implicit edge exists.
///
/// Parameters are Incan source and Rust target boundary type displays. Scalars (`int`, `float`, `str`, `bytes`,
/// `unit`) and structural containers (`Option`, `Result`, `Tuple`, `List`/`Dict`/`Set`, frozen variants) are admitted
/// recursively when their element/slot coercions are admitted.
#[must_use]
pub fn admitted_builtin_coercion(incan_type: &str, rust_target: &str) -> Option<CoercionPolicy> {
    let normalized_incan = normalize_incan_type_name(incan_type);
    let normalized_rust = normalize_rust_target_display(rust_target);

    if let Some(policy) = admitted_scalar_coercion(normalized_incan.as_str(), normalized_rust.as_str()) {
        return Some(policy);
    }

    let incan = parse_incan_boundary_type(normalized_incan.as_str())?;
    let rust = parse_rust_boundary_type(normalized_rust.as_str())?;
    admitted_structural_coercion(&incan, &rust)
}

fn admitted_scalar_coercion(incan_type: &str, rust_target: &str) -> Option<CoercionPolicy> {
    match (incan_type, rust_target) {
        ("int", "i64") => Some(CoercionPolicy::Exact),
        ("float", "f64") => Some(CoercionPolicy::Exact),
        ("float", "f32") => Some(CoercionPolicy::Lossy),
        ("bool", "bool") => Some(CoercionPolicy::Exact),
        ("str", "String") | ("frozenstr", "String") => Some(CoercionPolicy::Exact),
        ("str", "std::string::String") | ("frozenstr", "std::string::String") => Some(CoercionPolicy::Exact),
        ("str", "&String") | ("frozenstr", "&String") => Some(CoercionPolicy::Borrow),
        ("str", "&std::string::String") | ("frozenstr", "&std::string::String") => Some(CoercionPolicy::Borrow),
        ("str", "&alloc::string::String") | ("frozenstr", "&alloc::string::String") => Some(CoercionPolicy::Borrow),
        ("str", "&str") | ("frozenstr", "&str") => Some(CoercionPolicy::Borrow),
        ("bytes", "Vec<u8>") | ("frozenbytes", "Vec<u8>") => Some(CoercionPolicy::Exact),
        ("bytes", "std::vec::Vec<u8>") | ("frozenbytes", "std::vec::Vec<u8>") => Some(CoercionPolicy::Exact),
        ("bytes", "&Vec<u8>") | ("frozenbytes", "&Vec<u8>") => Some(CoercionPolicy::Borrow),
        ("bytes", "&std::vec::Vec<u8>") | ("frozenbytes", "&std::vec::Vec<u8>") => Some(CoercionPolicy::Borrow),
        ("bytes", "&alloc::vec::Vec<u8>") | ("frozenbytes", "&alloc::vec::Vec<u8>") => Some(CoercionPolicy::Borrow),
        ("bytes", "&[u8]") | ("frozenbytes", "&[u8]") => Some(CoercionPolicy::Borrow),
        ("none", "()") | ("unit", "()") => Some(CoercionPolicy::Exact),
        _ => None,
    }
}

fn normalize_incan_type_name(incan_type: &str) -> String {
    incan_type.replace(' ', "").to_ascii_lowercase()
}

fn normalize_rust_target_display(rust_target: &str) -> String {
    rust_target
        .trim()
        .replace("'static", "")
        .replace("'_", "")
        .replace("&mut", "&")
        .replace(' ', "")
}

fn parse_incan_boundary_type(raw: &str) -> Option<IncanBoundaryType> {
    if raw.is_empty() {
        return None;
    }

    if raw.starts_with('(') && raw.ends_with(')') {
        let inner = &raw[1..raw.len() - 1];
        if inner.is_empty() {
            return Some(IncanBoundaryType::Tuple(Vec::new()));
        }
        let elems = split_top_level(inner, ',')
            .into_iter()
            .map(parse_incan_boundary_type)
            .collect::<Option<Vec<_>>>()?;
        return Some(IncanBoundaryType::Tuple(elems));
    }

    if let Some((base, inner)) = split_bracketed(raw, '[', ']') {
        let args = if inner.is_empty() {
            Vec::new()
        } else {
            split_top_level(inner, ',')
        };
        return match base {
            "Option" | "option" if args.len() == 1 => {
                parse_incan_boundary_type(args[0]).map(|t| IncanBoundaryType::Option(Box::new(t)))
            }
            "Result" | "result" if args.len() == 2 => {
                let ok = parse_incan_boundary_type(args[0])?;
                let err = parse_incan_boundary_type(args[1])?;
                Some(IncanBoundaryType::Result(Box::new(ok), Box::new(err)))
            }
            "Tuple" | "tuple" => {
                let elems = args
                    .into_iter()
                    .map(parse_incan_boundary_type)
                    .collect::<Option<Vec<_>>>()?;
                Some(IncanBoundaryType::Tuple(elems))
            }
            "List" | "list" | "Vec" if args.len() == 1 => {
                parse_incan_boundary_type(args[0]).map(|t| IncanBoundaryType::List(Box::new(t)))
            }
            "Dict" | "dict" | "HashMap" if args.len() == 2 => {
                let key = parse_incan_boundary_type(args[0])?;
                let val = parse_incan_boundary_type(args[1])?;
                Some(IncanBoundaryType::Dict(Box::new(key), Box::new(val)))
            }
            "Set" | "set" if args.len() == 1 => {
                parse_incan_boundary_type(args[0]).map(|t| IncanBoundaryType::Set(Box::new(t)))
            }
            "FrozenList" | "frozenlist" if args.len() == 1 => {
                parse_incan_boundary_type(args[0]).map(|t| IncanBoundaryType::FrozenList(Box::new(t)))
            }
            "FrozenDict" | "frozendict" if args.len() == 2 => {
                let key = parse_incan_boundary_type(args[0])?;
                let val = parse_incan_boundary_type(args[1])?;
                Some(IncanBoundaryType::FrozenDict(Box::new(key), Box::new(val)))
            }
            "FrozenSet" | "frozenset" if args.len() == 1 => {
                parse_incan_boundary_type(args[0]).map(|t| IncanBoundaryType::FrozenSet(Box::new(t)))
            }
            _ => Some(IncanBoundaryType::Primitive(raw.to_string())),
        };
    }

    Some(IncanBoundaryType::Primitive(raw.to_string()))
}

fn parse_rust_boundary_type(raw: &str) -> Option<RustBoundaryType> {
    if raw.is_empty() {
        return None;
    }

    if raw.starts_with('(') && raw.ends_with(')') {
        let inner = &raw[1..raw.len() - 1];
        if inner.is_empty() {
            return Some(RustBoundaryType::Tuple(Vec::new()));
        }
        let elems = split_top_level(inner, ',')
            .into_iter()
            .map(parse_rust_boundary_type)
            .collect::<Option<Vec<_>>>()?;
        return Some(RustBoundaryType::Tuple(elems));
    }

    if let Some((base, inner)) = split_bracketed(raw, '<', '>') {
        let args = if inner.is_empty() {
            Vec::new()
        } else {
            split_top_level(inner, ',')
        };

        if rust_base_is(base, &["Option", "std::option::Option", "core::option::Option"]) && args.len() == 1 {
            let inner_ty = parse_rust_boundary_type(args[0])?;
            return Some(RustBoundaryType::Option(Box::new(inner_ty)));
        }
        if rust_base_is(base, &["Result", "std::result::Result", "core::result::Result"]) && args.len() == 2 {
            let ok = parse_rust_boundary_type(args[0])?;
            let err = parse_rust_boundary_type(args[1])?;
            return Some(RustBoundaryType::Result(Box::new(ok), Box::new(err)));
        }
        if rust_base_is(base, &["Vec", "std::vec::Vec", "alloc::vec::Vec"]) && args.len() == 1 {
            let inner_ty = parse_rust_boundary_type(args[0])?;
            return Some(RustBoundaryType::Vec(Box::new(inner_ty)));
        }
        if rust_base_is(
            base,
            &[
                "HashMap",
                "std::collections::HashMap",
                "std::collections::hash_map::HashMap",
            ],
        ) && args.len() == 2
        {
            let key = parse_rust_boundary_type(args[0])?;
            let val = parse_rust_boundary_type(args[1])?;
            return Some(RustBoundaryType::HashMap(Box::new(key), Box::new(val)));
        }
        if rust_base_is(
            base,
            &[
                "HashSet",
                "std::collections::HashSet",
                "std::collections::hash_set::HashSet",
            ],
        ) && args.len() == 1
        {
            let inner_ty = parse_rust_boundary_type(args[0])?;
            return Some(RustBoundaryType::HashSet(Box::new(inner_ty)));
        }
    }

    Some(RustBoundaryType::Primitive(raw.to_string()))
}

fn rust_base_is(base: &str, accepted: &[&str]) -> bool {
    accepted.contains(&base)
}

fn split_bracketed(raw: &str, open: char, close: char) -> Option<(&str, &str)> {
    let open_idx = raw.find(open)?;
    if !raw.ends_with(close) || open_idx >= raw.len().saturating_sub(1) {
        return None;
    }
    let base = &raw[..open_idx];
    let inner = &raw[open_idx + 1..raw.len() - 1];
    Some((base, inner))
}

fn split_top_level(raw: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut angle_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut paren_depth = 0usize;

    for (idx, ch) in raw.char_indices() {
        match ch {
            '<' => angle_depth = angle_depth.saturating_add(1),
            '>' => angle_depth = angle_depth.saturating_sub(1),
            '[' => bracket_depth = bracket_depth.saturating_add(1),
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '(' => paren_depth = paren_depth.saturating_add(1),
            ')' => paren_depth = paren_depth.saturating_sub(1),
            _ => {}
        }

        if ch == delimiter && angle_depth == 0 && bracket_depth == 0 && paren_depth == 0 {
            parts.push(raw[start..idx].trim());
            start = idx + ch.len_utf8();
        }
    }
    parts.push(raw[start..].trim());
    parts
}

fn admitted_structural_coercion(incan: &IncanBoundaryType, rust: &RustBoundaryType) -> Option<CoercionPolicy> {
    match (incan, rust) {
        (IncanBoundaryType::Primitive(incan_name), RustBoundaryType::Primitive(rust_name)) => {
            admitted_scalar_coercion(incan_name.as_str(), rust_name.as_str())
        }
        (IncanBoundaryType::Option(from), RustBoundaryType::Option(to)) => admitted_structural_coercion(from, to),
        (IncanBoundaryType::Result(from_ok, from_err), RustBoundaryType::Result(to_ok, to_err)) => combine_policies(
            admitted_structural_coercion(from_ok, to_ok),
            admitted_structural_coercion(from_err, to_err),
        ),
        (IncanBoundaryType::Tuple(from_elems), RustBoundaryType::Tuple(to_elems)) => {
            if from_elems.len() != to_elems.len() {
                return None;
            }
            let mut policy = CoercionPolicy::Exact;
            for (from_elem, to_elem) in from_elems.iter().zip(to_elems.iter()) {
                let elem_policy = admitted_structural_coercion(from_elem, to_elem)?;
                policy = fold_policy(policy, elem_policy);
            }
            Some(policy)
        }
        (IncanBoundaryType::List(from), RustBoundaryType::Vec(to))
        | (IncanBoundaryType::FrozenList(from), RustBoundaryType::Vec(to))
        | (IncanBoundaryType::Set(from), RustBoundaryType::HashSet(to))
        | (IncanBoundaryType::FrozenSet(from), RustBoundaryType::HashSet(to)) => admitted_structural_coercion(from, to),
        (IncanBoundaryType::Dict(from_k, from_v), RustBoundaryType::HashMap(to_k, to_v))
        | (IncanBoundaryType::FrozenDict(from_k, from_v), RustBoundaryType::HashMap(to_k, to_v)) => combine_policies(
            admitted_structural_coercion(from_k, to_k),
            admitted_structural_coercion(from_v, to_v),
        ),
        _ => None,
    }
}

fn combine_policies(left: Option<CoercionPolicy>, right: Option<CoercionPolicy>) -> Option<CoercionPolicy> {
    Some(fold_policy(left?, right?))
}

fn fold_policy(left: CoercionPolicy, right: CoercionPolicy) -> CoercionPolicy {
    match (left, right) {
        (CoercionPolicy::Lossy, _) | (_, CoercionPolicy::Lossy) => CoercionPolicy::Lossy,
        (CoercionPolicy::Borrow, _) | (_, CoercionPolicy::Borrow) => CoercionPolicy::Borrow,
        _ => CoercionPolicy::Exact,
    }
}

#[cfg(test)]
mod tests {
    use super::{CoercionPolicy, admitted_builtin_coercion};

    #[test]
    fn scalar_edges_still_match() {
        assert_eq!(admitted_builtin_coercion("float", "f32"), Some(CoercionPolicy::Lossy));
        assert_eq!(admitted_builtin_coercion("str", "&str"), Some(CoercionPolicy::Borrow));
        assert_eq!(
            admitted_builtin_coercion("str", "&String"),
            Some(CoercionPolicy::Borrow)
        );
        assert_eq!(
            admitted_builtin_coercion("FrozenStr", "&str"),
            Some(CoercionPolicy::Borrow)
        );
        assert_eq!(
            admitted_builtin_coercion("bytes", "&Vec<u8>"),
            Some(CoercionPolicy::Borrow)
        );
        assert_eq!(admitted_builtin_coercion("Unit", "()"), Some(CoercionPolicy::Exact));
    }

    #[test]
    fn option_coercion_recurses_into_inner_slot() {
        assert_eq!(
            admitted_builtin_coercion("Option[float]", "Option<f32>"),
            Some(CoercionPolicy::Lossy)
        );
    }

    #[test]
    fn result_coercion_combines_ok_and_err_slots() {
        assert_eq!(
            admitted_builtin_coercion("Result[str, float]", "Result<&str, f32>"),
            Some(CoercionPolicy::Lossy)
        );
    }

    #[test]
    fn tuple_coercion_requires_same_arity_and_recursive_admission() {
        assert_eq!(
            admitted_builtin_coercion("(str, float)", "(&str,f32)"),
            Some(CoercionPolicy::Lossy)
        );
        assert_eq!(admitted_builtin_coercion("(int, str)", "(i64)"), None);
    }

    #[test]
    fn list_dict_set_and_frozen_variants_are_supported() {
        assert_eq!(
            admitted_builtin_coercion("List[str]", "Vec<&str>"),
            Some(CoercionPolicy::Borrow)
        );
        assert_eq!(
            admitted_builtin_coercion("Dict[str, float]", "std::collections::HashMap<&str, f32>"),
            Some(CoercionPolicy::Lossy)
        );
        assert_eq!(
            admitted_builtin_coercion("Set[bytes]", "HashSet<&[u8]>"),
            Some(CoercionPolicy::Borrow)
        );
        assert_eq!(
            admitted_builtin_coercion("FrozenList[str]", "std::vec::Vec<String>"),
            Some(CoercionPolicy::Exact)
        );
        assert_eq!(
            admitted_builtin_coercion("FrozenDict[str, float]", "HashMap<&str, f32>"),
            Some(CoercionPolicy::Lossy)
        );
        assert_eq!(
            admitted_builtin_coercion("FrozenSet[bytes]", "std::collections::HashSet<&[u8]>"),
            Some(CoercionPolicy::Borrow)
        );
    }

    #[test]
    fn structural_mismatch_is_rejected() {
        assert_eq!(admitted_builtin_coercion("List[int]", "HashSet<i64>"), None);
        assert_eq!(admitted_builtin_coercion("Option[int]", "Option<String>"), None);
    }
}
