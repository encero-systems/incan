//! Checked, usage-derived Rust caller facets for Incan libraries.

use std::collections::{BTreeMap, BTreeSet};

use incan_frontend::ast::Visibility;
use incan_frontend::library_exports::{CheckedExportKind, CheckedExportProjection, CheckedNamedExport};
use incan_frontend::symbols::ResolvedType;
use incan_lang::lang::types::collections::{self, CollectionTypeId};

/// One library-scoped caller facet selected from Rust source paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerFacetSelection {
    /// Incan library crate named by the Rust source.
    pub library: String,
    /// Checked public exports selected for the shared caller namespace.
    pub exports: Vec<String>,
}

/// Generation inputs for one checked caller facet of an Incan library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerFacetRequest {
    /// Source names selected from checked public exports.
    pub exports: BTreeSet<String>,
    /// Stable digest of the selected caller surface.
    pub facet_id: String,
    /// Receipt path the generated identity record points to.
    pub receipt_reference: String,
    /// Target triple of the consuming Rust unit.
    pub target: String,
    /// Build profile of the consuming Rust unit.
    pub profile: String,
}

/// Find `<library>::caller::incan::<export>` paths without interpreting generated Rust.
///
/// The scanner recognizes Rust identifiers and `::` punctuation while skipping comments and string/character
/// literals. It intentionally does not perform name resolution: checked Incan metadata remains authoritative for
/// whether the resulting library and export names are valid.
pub fn scan_rust_caller_paths(source: &str) -> BTreeMap<String, BTreeSet<String>> {
    let tokens = rust_path_tokens(source);
    let mut paths: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (index, window) in tokens.windows(6).enumerate() {
        let [library, separator_1, caller, separator_2, incan, separator_3] = window else {
            continue;
        };
        if separator_1 == "::"
            && caller == "caller"
            && separator_2 == "::"
            && incan == "incan"
            && separator_3 == "::"
            && is_rust_identifier(library)
        {
            let Some(next) = tokens.get(index + 6) else { continue };
            if is_rust_identifier(next) {
                paths.entry(library.clone()).or_default().insert(next.clone());
            } else if next == "{" {
                for export in tokens.iter().skip(index + 7).take_while(|token| token.as_str() != "}") {
                    if is_rust_identifier(export) && export != "as" {
                        paths.entry(library.clone()).or_default().insert(export.clone());
                    }
                }
            }
        }
    }
    paths
}

/// Select and validate the checked public exports referenced by one library's Rust callers.
///
/// Selection fails closed. A missing name includes private declarations because they never enter the checked public
/// export set. Representability is recursive through selected public models and enums, so an otherwise public model
/// cannot smuggle a private field or unsupported Rust/interior callable type into the caller ABI.
pub fn select_checked_caller_exports(
    library: &str,
    requested: &BTreeSet<String>,
    checked_exports: &[CheckedNamedExport],
) -> Result<CallerFacetSelection, String> {
    let exports_by_name = checked_exports
        .iter()
        .map(|export| (export.name.as_str(), export))
        .collect::<BTreeMap<_, _>>();
    let mut selected = Vec::new();
    for name in requested {
        let export = exports_by_name
            .get(name.as_str())
            .ok_or_else(|| format!("caller export `{library}::{name}` is missing or is not public"))?;
        if !is_root_declaration(export) {
            return Err(format!(
                "caller export `{library}::{name}` is not declared in the library entrypoint: `caller::incan` \
                 re-exports entrypoint declarations under their own names, not aliases, re-exports or submodule items"
            ));
        }
        export_representability(export, &exports_by_name)
            .map_err(|reason| format!("caller export `{library}::{name}` is not representable: {reason}"))?;
        selected.push(name.clone());
    }
    Ok(CallerFacetSelection {
        library: library.to_string(),
        exports: selected,
    })
}

/// Whether `export` exposes its own declaration from the library entrypoint under its own name.
///
/// The caller namespace re-exports each selected item as `crate::<name>`, which names exactly such a declaration in
/// the generated crate. An alias, a re-export or a submodule declaration lives at another path, so it is refused
/// rather than projected under a path that does not name it.
fn is_root_declaration(export: &CheckedNamedExport) -> bool {
    matches!(export.identity.projection, CheckedExportProjection::Direct)
        && matches!(export.identity.source_path.as_slice(), [name] if *name == export.name)
}

/// Validate one selected checked export and every nominal type reachable from its public shape.
fn export_representability(
    export: &CheckedNamedExport,
    exports: &BTreeMap<&str, &CheckedNamedExport>,
) -> Result<(), String> {
    match &export.kind {
        CheckedExportKind::Function(function) => {
            if function.is_async {
                return Err("async functions are outside the bounded caller ABI".to_string());
            }
            if !function.type_params.is_empty() {
                return Err("functions with type parameters are unsupported".to_string());
            }
            for parameter in &function.params {
                type_representability(&parameter.ty, exports, &mut BTreeSet::new())?;
            }
            type_representability(&function.return_type, exports, &mut BTreeSet::new())
        }
        CheckedExportKind::Model(model) => {
            if !model.type_params.is_empty() {
                return Err("models with type parameters are unsupported".to_string());
            }
            for field in &model.fields {
                if field.visibility != Visibility::Public {
                    return Err(format!("model field `{}` is not public", field.name));
                }
                type_representability(&field.ty, exports, &mut BTreeSet::new())?;
            }
            Ok(())
        }
        CheckedExportKind::Enum(enum_export) => {
            if !enum_export.type_params.is_empty() {
                return Err("enums with type parameters are unsupported".to_string());
            }
            for variant in &enum_export.variants {
                for field in &variant.fields {
                    type_representability(field, exports, &mut BTreeSet::new())?;
                }
            }
            Ok(())
        }
        CheckedExportKind::Newtype(newtype) if newtype.is_rusttype => {
            Err("rusttype exports are unsupported".to_string())
        }
        _ => Err("this declaration kind is outside the bounded caller ABI".to_string()),
    }
}

/// Validate one checked type, following public nominal models and enums without looping on recursive shapes.
fn type_representability(
    ty: &ResolvedType,
    exports: &BTreeMap<&str, &CheckedNamedExport>,
    visiting: &mut BTreeSet<String>,
) -> Result<(), String> {
    match ty {
        ResolvedType::Int | ResolvedType::Float | ResolvedType::Bool | ResolvedType::Str | ResolvedType::Unit => Ok(()),
        ResolvedType::Generic(name, arguments)
            if matches!(
                collections::from_str(name),
                Some(CollectionTypeId::List | CollectionTypeId::Option | CollectionTypeId::Result)
            ) =>
        {
            for argument in arguments {
                type_representability(argument, exports, visiting)?;
            }
            Ok(())
        }
        ResolvedType::Named(name) => {
            if !visiting.insert(name.clone()) {
                return Ok(());
            }
            let export = exports
                .get(name.as_str())
                .ok_or_else(|| format!("nominal type `{name}` is not a checked public export"))?;
            let result = export_representability(export, exports);
            visiting.remove(name);
            result
        }
        ResolvedType::Function(_, _) => Err("closure or callable values are unsupported".to_string()),
        ResolvedType::RustPath(_) => Err("Rust-native types are unsupported".to_string()),
        ResolvedType::TypeVar(_) => Err("type parameters are unsupported".to_string()),
        other => Err(format!("type `{other}` is unsupported")),
    }
}

/// Tokenize only the identifier and path punctuation needed by [`scan_rust_caller_paths`].
fn rust_path_tokens(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"//") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if bytes[index..].starts_with(b"/*") {
            index += 2;
            let mut depth = 1usize;
            while index < bytes.len() && depth > 0 {
                if bytes[index..].starts_with(b"/*") {
                    depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
        } else if matches!(bytes[index], b'"' | b'\'') {
            let delimiter = bytes[index];
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else if bytes[index] == delimiter {
                    index += 1;
                    break;
                } else {
                    index += 1;
                }
            }
        } else if bytes[index..].starts_with(b"::") {
            tokens.push("::".to_string());
            index += 2;
        } else if matches!(bytes[index], b'{' | b'}' | b',') {
            tokens.push((bytes[index] as char).to_string());
            index += 1;
        } else if (bytes[index] as char).is_ascii_alphabetic() || bytes[index] == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len() && ((bytes[index] as char).is_ascii_alphanumeric() || bytes[index] == b'_') {
                index += 1;
            }
            tokens.push(source[start..index].to_string());
        } else {
            index += 1;
        }
    }
    tokens
}

/// Return whether a token has the lexical shape of a Rust identifier.
fn is_rust_identifier(token: &str) -> bool {
    let mut chars = token.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use incan_frontend::library_exports::collect_checked_public_exports;
    use incan_frontend::typechecker::TypeChecker;
    use incan_frontend::{lexer, parser};

    use super::{scan_rust_caller_paths, select_checked_caller_exports};
    use incan_frontend::library_exports::CheckedExportProjection;

    #[test]
    fn caller_scan_unions_paths_and_ignores_comments_and_literals() {
        let source = r#"
use policy::caller::incan::{Plan, Op};
fn run() { let _ = policy::caller::incan::make_plan(); }
// hidden::caller::incan::ignored
const TEXT: &str = "hidden::caller::incan::ignored";
"#;
        let paths = scan_rust_caller_paths(source);
        let policy = paths
            .get("policy")
            .map(|names| names.iter().cloned().collect::<Vec<_>>());
        assert_eq!(
            policy,
            Some(vec!["Op".to_string(), "Plan".to_string(), "make_plan".to_string()])
        );
        assert!(!paths.contains_key("hidden"));
    }

    #[test]
    fn checked_selection_accepts_the_bounded_plan_shape() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub enum Op:\n    Assign(int, int)\n    Return\n\n\npub model Plan:\n    pub ops: list[Op]\n    pub label: str\n    pub version: int\n\n\npub def make_plan() -> Plan:\n    return Plan(ops=[Op.Return], label=\"ok\", version=1)\n";
        let exports = checked_exports(source)?;
        let requested = BTreeSet::from(["Op".to_string(), "Plan".to_string(), "make_plan".to_string()]);
        let selected = select_checked_caller_exports("policy", &requested, &exports)?;
        assert_eq!(selected.exports, vec!["Op", "Plan", "make_plan"]);
        Ok(())
    }

    #[test]
    fn checked_selection_refuses_private_and_nonrepresentable_exports() -> Result<(), Box<dyn std::error::Error>> {
        let source = "import std.async\n\n\ndef hidden() -> int:\n    return 1\n\n\npub async def later() -> int:\n    return 1\n";
        let exports = checked_exports(source)?;
        let missing = select_checked_caller_exports("policy", &BTreeSet::from(["hidden".to_string()]), &exports)
            .err()
            .ok_or("private export unexpectedly selected")?;
        assert!(
            missing.contains("hidden") && missing.contains("not public"),
            "{missing}"
        );
        let unsupported = select_checked_caller_exports("policy", &BTreeSet::from(["later".to_string()]), &exports)
            .err()
            .ok_or("async export unexpectedly selected")?;
        assert!(
            unsupported.contains("later") && unsupported.contains("async"),
            "{unsupported}"
        );
        Ok(())
    }

    #[test]
    fn checked_selection_refuses_an_export_that_is_not_a_root_declaration() -> Result<(), Box<dyn std::error::Error>> {
        // `caller::incan` re-exports `crate::<name>`, which names only a root declaration under its own name; a
        // re-export or a submodule item lives elsewhere, so selecting it would project a path that does not name it.
        let exports = checked_exports("pub def answer() -> int:\n    return 42\n")?;
        let mut reexported = exports.clone();
        for export in &mut reexported {
            export.identity.projection = CheckedExportProjection::Reexport {
                target_path: vec!["lib".to_string(), "inner".to_string(), "answer".to_string()],
            };
        }
        let mut nested = exports;
        for export in &mut nested {
            export.identity.source_path = vec!["lib".to_string(), "inner".to_string(), "answer".to_string()];
        }
        for exports in [reexported, nested] {
            let refused = select_checked_caller_exports("policy", &BTreeSet::from(["answer".to_string()]), &exports)
                .err()
                .ok_or("a non-root export was selected")?;
            assert!(
                refused.contains("answer") && refused.contains("library entrypoint"),
                "{refused}"
            );
        }
        Ok(())
    }

    /// Parse and typecheck one source module before collecting its public export authority.
    fn checked_exports(source: &str) -> Result<Vec<incan_frontend::library_exports::CheckedNamedExport>, String> {
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("parse errors: {errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker
            .check_program(&program)
            .map_err(|errors| format!("typecheck errors: {errors:?}"))?;
        Ok(collect_checked_public_exports(&program, &checker))
    }
}
