use std::collections::BTreeMap;

use crate::ast;
use crate::loaf_index::LoafIndex;

/// One hidden import injected to back a symbolic helper reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HelperImportSpec {
    dependency_key: String,
    exported_name: String,
    alias: String,
}

/// Deduplicates helper imports requested by desugared output before we splice them into the host program.
#[derive(Debug, Default)]
pub(super) struct HelperImportAccumulator {
    imports: BTreeMap<(String, String), HelperImportSpec>,
}

impl HelperImportAccumulator {
    /// Register one helper import and return the alias that desugared code should call.
    pub(super) fn register(&mut self, dependency_key: &str, exported_name: &str) -> String {
        let key = (dependency_key.to_string(), exported_name.to_string());
        let alias = helper_import_alias(dependency_key, exported_name);
        let spec = HelperImportSpec {
            dependency_key: dependency_key.to_string(),
            exported_name: exported_name.to_string(),
            alias: alias.clone(),
        };
        self.imports.entry(key).or_insert(spec);
        alias
    }

    /// Materialize deterministic hidden imports for all registered helper aliases.
    fn import_declarations(&self) -> Vec<ast::Spanned<ast::Declaration>> {
        let mut declarations = Vec::new();
        for spec in self.imports.values() {
            declarations.push(ast::Spanned::new(
                ast::Declaration::Import(ast::ImportDecl {
                    visibility: ast::Visibility::Private,
                    kind: ast::ImportKind::PubFrom {
                        library: spec.dependency_key.clone(),
                        path: Vec::new(),
                        items: vec![ast::ImportItem {
                            name: spec.exported_name.clone(),
                            alias: Some(spec.alias.clone()),
                        }],
                    },
                    alias: None,
                }),
                ast::Span::default(),
            ));
        }
        declarations
    }
}

/// Prefix shared by every hidden helper import alias.
const HELPER_ALIAS_PREFIX: &str = "__incan_vocab_helper_";

/// Build the hidden import alias used when a desugarer references a provider helper symbol.
///
/// The alias is spliced into the desugared program as an ordinary identifier, so it has to be one, and it names the
/// exact `(dependency, export)` pair, so it has to be injective: two distinct pairs must never share an alias, or
/// the hidden imports for both bind one name and one helper silently shadows the other (#1380). The earlier
/// spelling collapsed every non-alphanumeric character to `_`, which lost both the boundary between the two
/// components and the difference between `-` and `_`, so `("a-b", "c")`, `("a", "b_c")` and `("a_b", "c")` all
/// became `__incan_vocab_helper_a_b_c`.
///
/// The shape is `__incan_vocab_helper_<n>_<dependency>_<export>`. The export is a checked public export name and
/// therefore already an identifier, so it is spelled verbatim. The dependency key is an open alphabet — Cargo-style
/// package names carry `-`, compiler-owned keys carry `.` — so it is escaped by [`escape_dependency_key`] into the
/// identifier alphabet, and `<n>` is the length of that escaped spelling. The length is what makes the pair
/// recoverable: an export may itself start with or contain `_`, so no separator character could mark where the
/// dependency ends, but a length can. Nothing here hashes, so the alias is reproducible across builds and readable
/// in desugared output.
fn helper_import_alias(dependency_key: &str, exported_name: &str) -> String {
    let dependency = escape_dependency_key(dependency_key);
    format!("{HELPER_ALIAS_PREFIX}{}_{dependency}_{exported_name}", dependency.len())
}

/// Spell a dependency key in the identifier alphabet without losing information.
///
/// ASCII alphanumerics pass through. `_` is doubled, and every other character becomes `_<hex code point>_`, so a
/// single `_` followed by a hex digit can only ever open an escape and a doubled `_` can only ever be a literal one.
/// Without the doubling, a key literally spelled `a_2d_b` would coincide with the escaped `a-b`, and the alias would
/// be injective in practice but not by construction.
fn escape_dependency_key(dependency_key: &str) -> String {
    let mut escaped = String::with_capacity(dependency_key.len());
    for ch in dependency_key.chars() {
        match ch {
            ch if ch.is_ascii_alphanumeric() => escaped.push(ch),
            '_' => escaped.push_str("__"),
            other => {
                escaped.push('_');
                escaped.push_str(&format!("{:x}", u32::from(other)));
                escaped.push('_');
            }
        }
    }
    escaped
}

/// Inject hidden `pub::` imports for every helper symbol referenced by desugared output.
pub(super) fn inject_helper_imports(program: &mut ast::Program, helper_imports: &HelperImportAccumulator) {
    let imports = helper_imports.import_declarations();
    if imports.is_empty() {
        return;
    }

    let mut insert_at = 0usize;
    while let Some(declaration) = program.declarations.get(insert_at) {
        match declaration.node {
            ast::Declaration::Docstring(_) | ast::Declaration::Import(_) => insert_at += 1,
            _ => break,
        }
    }
    program.declarations.splice(insert_at..insert_at, imports);
}

/// Rewrite symbolic helper references inside desugared statements into hidden import aliases.
pub(super) fn resolve_helper_bindings_in_statements(
    statements: &mut [incan_vocab::IncanStatement],
    keyword_metadata: Option<&incan_vocab::VocabKeywordMetadata>,
    keyword: &str,
    loaves: &LoafIndex,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<(), String> {
    for statement in statements {
        resolve_helper_bindings_in_statement(statement, keyword_metadata, keyword, loaves, helper_imports)?;
    }
    Ok(())
}

/// Resolve helper references recursively inside one desugared public statement.
fn resolve_helper_bindings_in_statement(
    statement: &mut incan_vocab::IncanStatement,
    keyword_metadata: Option<&incan_vocab::VocabKeywordMetadata>,
    keyword: &str,
    loaves: &LoafIndex,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<(), String> {
    match statement {
        incan_vocab::IncanStatement::Expr(expr) => {
            resolve_helper_bindings_in_expr(expr, keyword_metadata, keyword, loaves, helper_imports)
        }
        incan_vocab::IncanStatement::Return(Some(expr))
        | incan_vocab::IncanStatement::Assign { value: expr, .. }
        | incan_vocab::IncanStatement::Let { value: expr, .. } => {
            resolve_helper_bindings_in_expr(expr, keyword_metadata, keyword, loaves, helper_imports)
        }
        incan_vocab::IncanStatement::If {
            condition,
            then_body,
            else_body,
        } => {
            resolve_helper_bindings_in_expr(condition, keyword_metadata, keyword, loaves, helper_imports)?;
            resolve_helper_bindings_in_statements(then_body, keyword_metadata, keyword, loaves, helper_imports)?;
            resolve_helper_bindings_in_statements(else_body, keyword_metadata, keyword, loaves, helper_imports)
        }
        incan_vocab::IncanStatement::While { condition, body } => {
            resolve_helper_bindings_in_expr(condition, keyword_metadata, keyword, loaves, helper_imports)?;
            resolve_helper_bindings_in_statements(body, keyword_metadata, keyword, loaves, helper_imports)
        }
        incan_vocab::IncanStatement::For { iter, body, .. } => {
            resolve_helper_bindings_in_expr(iter, keyword_metadata, keyword, loaves, helper_imports)?;
            resolve_helper_bindings_in_statements(body, keyword_metadata, keyword, loaves, helper_imports)
        }
        incan_vocab::IncanStatement::Pass | incan_vocab::IncanStatement::Return(None) => Ok(()),
        _ => Ok(()),
    }
}

/// Resolve helper references recursively inside one desugared public expression.
pub(super) fn resolve_helper_bindings_in_expr(
    expr: &mut incan_vocab::IncanExpr,
    keyword_metadata: Option<&incan_vocab::VocabKeywordMetadata>,
    keyword: &str,
    loaves: &LoafIndex,
    helper_imports: &mut HelperImportAccumulator,
) -> Result<(), String> {
    match expr {
        incan_vocab::IncanExpr::Helper(helper_key) => {
            let keyword_metadata = keyword_metadata.ok_or_else(|| {
                format!(
                    "keyword `{keyword}` does not carry provider metadata, so helper `{helper_key}` cannot be resolved"
                )
            })?;
            let exported_name = resolve_helper_export_name(loaves, &keyword_metadata.dependency_key, helper_key)?;
            let alias = helper_imports.register(&keyword_metadata.dependency_key, &exported_name);
            *expr = incan_vocab::IncanExpr::Name(alias);
            Ok(())
        }
        incan_vocab::IncanExpr::List(items) | incan_vocab::IncanExpr::Tuple(items) => {
            for item in items {
                resolve_helper_bindings_in_expr(item, keyword_metadata, keyword, loaves, helper_imports)?;
            }
            Ok(())
        }
        incan_vocab::IncanExpr::Dict(entries) => {
            for (key_expr, value_expr) in entries {
                resolve_helper_bindings_in_expr(key_expr, keyword_metadata, keyword, loaves, helper_imports)?;
                resolve_helper_bindings_in_expr(value_expr, keyword_metadata, keyword, loaves, helper_imports)?;
            }
            Ok(())
        }
        incan_vocab::IncanExpr::Binary(left, _, right) => {
            resolve_helper_bindings_in_expr(left, keyword_metadata, keyword, loaves, helper_imports)?;
            resolve_helper_bindings_in_expr(right, keyword_metadata, keyword, loaves, helper_imports)
        }
        incan_vocab::IncanExpr::Unary(_, value) => {
            resolve_helper_bindings_in_expr(value, keyword_metadata, keyword, loaves, helper_imports)
        }
        incan_vocab::IncanExpr::Call { callee, args } => {
            resolve_helper_bindings_in_expr(callee, keyword_metadata, keyword, loaves, helper_imports)?;
            for arg in args {
                resolve_helper_bindings_in_expr(arg, keyword_metadata, keyword, loaves, helper_imports)?;
            }
            Ok(())
        }
        incan_vocab::IncanExpr::Field { object, .. } => {
            resolve_helper_bindings_in_expr(object, keyword_metadata, keyword, loaves, helper_imports)
        }
        _ => Ok(()),
    }
}

/// Resolve one helper key to the public export name a desugared reference should import.
///
/// An explicit `HelperBinding` is the override, for a key whose export spelling deliberately differs from the key
/// the desugarer emits. With no binding, the key resolves directly against the provider's own checked public
/// surface, which is the same surface an ordinary consumer imports through.
///
/// That default is the point of #1032. A companion that re-states its package's export names is maintaining a
/// second copy of that surface, kept honest only by a test; the copy is what drifts. Resolving the key against the
/// package removes the copy rather than validating it.
fn resolve_helper_export_name(loaves: &LoafIndex, dependency_key: &str, helper_key: &str) -> Result<String, String> {
    let Some(entry) = loaves.entry(dependency_key) else {
        return Err(format!("provider `pub::{dependency_key}` is not loaded"));
    };
    let manifest = &entry.artifact.manifest;
    let Some(vocab) = manifest.vocab.as_ref() else {
        return Err(format!(
            "provider `pub::{dependency_key}` does not expose vocab metadata"
        ));
    };

    let mut matching = vocab
        .provider_manifest
        .helper_bindings
        .iter()
        .filter(|binding| binding.key == helper_key);
    let bound = matching.next();
    if let (Some(binding), Some(duplicate)) = (bound, matching.next()) {
        return Err(format!(
            "provider `pub::{dependency_key}` binds helper `{helper_key}` more than once, to `{}` and `{}`; \
             remove the duplicate so the helper has one spelling",
            binding.exported_name, duplicate.exported_name
        ));
    }

    // With no explicit binding the key names the export directly, so the diagnostics below have to say which of the
    // two the author actually wrote; "binds helper X to missing export X" would read as a broken binding that does
    // not exist.
    let exported_name = match bound {
        Some(binding) => binding.exported_name.as_str(),
        None => helper_key,
    };

    let Some(kind) = manifest.exports.view().helper_export_kind(exported_name) else {
        return Err(match bound {
            Some(_) => format!(
                "provider `pub::{dependency_key}` binds helper `{helper_key}` to missing export `{exported_name}`"
            ),
            None => format!(
                "provider `pub::{dependency_key}` does not export `{helper_key}`; export it, or bind the helper to \
                 the name it should resolve to"
            ),
        });
    };
    if !kind.is_callable() {
        return Err(format!(
            "provider `pub::{dependency_key}` resolves helper `{helper_key}` to {} `{exported_name}`, which cannot \
             be called; a helper must resolve to a function, class, model, newtype, enum variant, partial, or alias",
            kind.label()
        ));
    }
    Ok(exported_name.to_string())
}
