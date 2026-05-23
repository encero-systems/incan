//! Generate Markdown reference docs from `incan_core::lang` registries.
//!
//! This binary renders:
//! - language-core vocabulary registries (keywords, operators, builtins, types, punctuation)
//!
//! Outputs are written under `workspaces/docs-site/docs/language/reference/`.
//!
//! ## Notes
//! - The generated files are meant to be checked into the repo and treated as derived artifacts.
//! - **Do not edit generated files by hand** (`language.md`).
//! - Change source registries under `crates/incan_core/src/lang/` for core language tables.
//! - Re-run this binary so docs remain in sync.
//!
//! ## Examples
//! Run from the workspace root:
//! ```bash
//! cargo run -p incan_core --bin generate_lang_reference
//! ```
//!
//! ## Panics
//! - If the workspace root cannot be resolved.
//! - If output files cannot be written.

use std::fs;
use std::path::{Path, PathBuf};

use incan_core::lang::types::{collections, numerics, stringlike};
use incan_core::lang::{
    builtins, decorators, derives, errors, features, keywords, operators, punctuation, stdlib, surface, traits,
};

/// Reduce trailing blank lines in generated Markdown to at most one empty line.
fn trim_trailing_newlines_to_at_most_two(out: &mut String) {
    let mut count = 0usize;
    for ch in out.chars().rev() {
        if ch == '\n' {
            count += 1;
        } else {
            break;
        }
    }
    while count > 2 {
        out.pop();
        count -= 1;
    }
}

/// Remove all trailing newline characters from generated Markdown.
fn trim_trailing_newlines(out: &mut String) {
    while out.ends_with('\n') {
        out.pop();
    }
}

/// Ensure the generated Markdown buffer ends with exactly one blank line when it already has content.
fn ensure_single_blank_line(out: &mut String) {
    trim_trailing_newlines_to_at_most_two(out);
    if out.is_empty() {
        return;
    }
    if out.ends_with("\n\n") {
        return;
    }
    if out.ends_with('\n') {
        out.push('\n');
    } else {
        out.push_str("\n\n");
    }
}

/// Append a Markdown section heading after normalizing preceding blank lines.
fn start_section(out: &mut String, heading: &str) {
    ensure_single_blank_line(out);
    out.push_str(heading);
    out.push_str("\n\n");
}

fn main() {
    let root = workspace_root();

    let out_dir = root.join("workspaces/docs-site/docs/language/reference");
    if let Err(err) = fs::create_dir_all(&out_dir) {
        panic!("create workspaces/docs-site/docs/language/reference/: {err}");
    }

    write_language_reference(&out_dir.join("language.md"));
    write_feature_inventory_reference(&out_dir.join("feature_inventory.md"));
}

/// Write `workspaces/docs-site/docs/language/reference/language.md`.
///
/// This is a single consolidated reference document generated from `incan_core::lang` registries.
fn write_language_reference(path: &Path) {
    let mut out = String::new();
    out.push_str("# Incan language reference\n\n");
    out.push_str("!!! warning \"Generated file\"\n");
    out.push_str("    Do not edit this page by hand. If it looks wrong/outdated, regenerate it from source and commit the result.\n");
    out.push('\n');
    out.push_str("    Regenerate with: `cargo run -p incan_core --bin generate_lang_reference`\n\n");

    out.push_str("## Contents\n\n");
    out.push_str("- [Keywords](#keywords)\n");
    out.push_str("- [Soft keywords](#soft-keywords)\n");
    out.push_str("- [Standard library namespaces](#standard-library-namespaces)\n");
    out.push_str("- [Builtin exceptions](#builtin-exceptions)\n");
    out.push_str("- [Builtin functions](#builtin-functions)\n");
    out.push_str("- [Decorators](#decorators)\n");
    out.push_str("- [Derives](#derives)\n");
    out.push_str("- [Builtin traits](#builtin-traits)\n");
    out.push_str("- [Operators](#operators)\n");
    out.push_str("- [Punctuation](#punctuation)\n");
    out.push_str("- [Builtin types](#builtin-types)\n");
    out.push_str("- [Surface constructors](#surface-constructors)\n");
    out.push_str("- [Surface functions](#surface-functions)\n");
    out.push_str("- [Built-in collection helpers](#built-in-collection-helpers)\n");
    out.push_str("- [Surface string methods](#surface-string-methods)\n");
    out.push_str("- [Surface types](#surface-types)\n");
    out.push_str("- [Surface methods](#surface-methods)\n\n");

    render_keywords_section(&mut out);
    render_soft_keywords_section(&mut out);
    render_stdlib_namespaces_section(&mut out);
    render_exceptions_section(&mut out);
    render_builtins_section(&mut out);
    render_decorators_section(&mut out);
    render_derives_section(&mut out);
    render_traits_section(&mut out);
    render_operators_section(&mut out);
    render_punctuation_section(&mut out);
    render_types_section(&mut out);

    render_surface_constructors_section(&mut out);
    render_surface_functions_section(&mut out);
    render_builtin_collection_helpers_section(&mut out);
    render_surface_string_methods_section(&mut out);
    render_surface_types_section(&mut out);
    render_surface_methods_section(&mut out);

    trim_trailing_newlines(&mut out);
    out.push('\n');
    if let Err(err) = fs::write(path, out) {
        panic!("write language.md: {err}");
    }
}

/// Write `workspaces/docs-site/docs/language/reference/feature_inventory.md`.
///
/// This generated page is backed by a curated product-level feature registry. It complements `language.md`, which lists
/// lower-level vocabulary such as keywords, operators, builtins, and surface methods.
fn write_feature_inventory_reference(path: &Path) {
    let mut out = String::new();
    out.push_str("# Incan feature inventory\n\n");
    out.push_str("!!! warning \"Generated file\"\n");
    out.push_str(
        "    Do not edit this page by hand. If it looks wrong/outdated, update `crates/incan_core/src/lang/features.rs` and regenerate it.\n",
    );
    out.push('\n');
    out.push_str("    Regenerate with: `cargo run -p incan_core --bin generate_lang_reference`\n\n");
    out.push_str(
        "This page is a generated, present-tense atlas of user-facing Incan capabilities. It is intentionally higher-level than the generated language vocabulary tables: one feature can span syntax, type checking, stdlib source, manifests, tooling, and examples.\n\n",
    );
    out.push_str("Use it when deciding whether code should use an existing Incan surface before adding wrappers, Rust fallbacks, or project-local conventions.\n\n");

    out.push_str("## Contents\n\n");
    out.push_str("- [All features](#all-features)\n");
    out.push_str("- [Feature details](#feature-details)\n\n");

    render_features_summary_section(&mut out);
    render_features_detail_section(&mut out);

    trim_trailing_newlines(&mut out);
    out.push('\n');
    if let Err(err) = fs::write(path, out) {
        panic!("write feature_inventory.md: {err}");
    }
}

fn markdown_table_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn markdown_code(value: &str) -> String {
    format!("`{}`", value.replace('`', "\\`"))
}

fn markdown_links(links: &[features::FeatureLink]) -> String {
    links
        .iter()
        .map(|link| format!("[{}]({})", link.label, link.path))
        .collect::<Vec<_>>()
        .join(", ")
}

fn canonical_forms_cell(forms: &[&str]) -> String {
    if forms.is_empty() {
        return "-".to_string();
    }
    forms
        .iter()
        .map(|form| markdown_table_cell(&markdown_code(form)))
        .collect::<Vec<_>>()
        .join("<br>")
}

fn render_features_summary_section(out: &mut String) {
    start_section(out, "## All features");

    out.push_str(
        "| Feature | Category | Since | Activation | Canonical forms | Summary | Prefer over | References |\n",
    );
    out.push_str("|---|---|---:|---|---|---|---|---|\n");

    for feature in features::FEATURES {
        let name = markdown_table_cell(feature.name);
        let category = format!("{:?}", feature.category);
        let since = feature.since;
        let activation = markdown_table_cell(feature.activation);
        let canonical_forms = canonical_forms_cell(feature.canonical_forms);
        let summary = markdown_table_cell(feature.summary);
        let prefer_over = markdown_table_cell(feature.prefer_over);
        let references = markdown_links(feature.references);
        out.push_str(&format!(
            "| {name} | {category} | {since} | {activation} | {canonical_forms} | {summary} | {prefer_over} | {references} |\n"
        ));
    }
    out.push('\n');
}

fn render_features_detail_section(out: &mut String) {
    start_section(out, "## Feature details");

    for feature in features::FEATURES {
        out.push_str(&format!("### {}\n\n", feature.name));
        out.push_str(&format!("- **Id:** `{:?}`\n", feature.id));
        out.push_str(&format!("- **Category:** `{:?}`\n", feature.category));
        out.push_str(&format!("- **Since:** `{}`\n", feature.since));
        out.push_str(&format!("- **RFC:** `{}`\n", feature.introduced_in_rfc));
        out.push_str(&format!("- **Stability:** `{:?}`\n", feature.stability));
        out.push_str(&format!("- **Activation:** {}\n", feature.activation));
        out.push_str(&format!("- **Use instead of:** {}\n", feature.prefer_over));
        out.push_str(&format!("- **References:** {}\n\n", markdown_links(feature.references)));
        out.push_str(feature.summary);
        out.push_str("\n\n");
        if !feature.canonical_forms.is_empty() {
            out.push_str("Canonical forms:\n\n");
            for form in feature.canonical_forms {
                out.push_str(&format!("- `{}`\n", form.replace('`', "\\`")));
            }
            out.push('\n');
        }
    }
}

/// Render the keyword registry table and examples.
fn render_keywords_section(out: &mut String) {
    start_section(out, "## Keywords");

    out.push_str("Reservation describes how a spelling is reserved: `Hard` keywords are always reserved by the lexer, `Contextual` keywords are recognized only in parser-owned syntactic positions, and `Soft` keywords are reserved after importing their activating `std.*` namespace.\n\n");

    out.push_str(
        "| Id | Canonical | Aliases | Reservation | Activation | Category | Usage | RFC | Since | Stability |\n",
    );
    out.push_str("|----|---|---|---|---|---|---|---|---|---|\n");

    for k in keywords::KEYWORDS {
        let id = format!("{:?}", k.id);
        let canonical = format!("`{}`", k.canonical);
        let aliases = if k.aliases.is_empty() {
            String::new()
        } else {
            k.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let activation = keywords::activation(k.id)
            .map(|ns| format!("`std.{}`", ns))
            .unwrap_or_else(|| "-".to_string());
        let reservation = match k.activation {
            keywords::KeywordActivation::Hard => "Hard",
            keywords::KeywordActivation::Contextual => "Contextual",
            keywords::KeywordActivation::Soft { .. } => "Soft",
        };
        let category = format!("{:?}", k.category);
        let usage = if k.usage.is_empty() {
            String::new()
        } else {
            k.usage
                .iter()
                .map(|u| format!("{:?}", u))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let rfc = k.introduced_in_rfc;
        let since = k.since;
        let stability = format!("{:?}", k.stability);

        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {reservation} | {activation} | {category} | {usage} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    out.push_str("### Examples\n\n");
    out.push_str("Only keywords with examples are listed here.\n\n");
    for k in keywords::KEYWORDS {
        if k.examples.is_empty() {
            continue;
        }
        out.push_str(&format!("#### `{:?}`\n\n", k.id));
        for ex in k.examples {
            out.push_str("```incan\n");
            out.push_str(ex.code);
            out.push_str("\n```\n\n");
            if let Some(note) = ex.note {
                out.push_str(note);
                out.push_str("\n\n");
            }
        }
    }
}

/// Render import-activated soft keywords.
fn render_soft_keywords_section(out: &mut String) {
    start_section(out, "## Soft keywords");
    out.push_str("Soft keywords are only reserved when their activating `std.*` namespace is imported.\n\n");

    out.push_str("| Id | Canonical | Activated by | Category | Usage | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");

    for k in keywords::KEYWORDS
        .iter()
        .filter(|k| keywords::activation(k.id).is_some())
    {
        let Some(activation_ns) = keywords::activation(k.id) else {
            continue;
        };
        let id = format!("{:?}", k.id);
        let canonical = format!("`{}`", k.canonical);
        let activated_by = format!("`std.{activation_ns}`");
        let category = format!("{:?}", k.category);
        let usage = if k.usage.is_empty() {
            String::new()
        } else {
            k.usage
                .iter()
                .map(|u| format!("{:?}", u))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let rfc = k.introduced_in_rfc;
        let since = k.since;
        let stability = format!("{:?}", k.stability);

        out.push_str(&format!(
            "| {id} | {canonical} | {activated_by} | {category} | {usage} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render standard-library namespaces and the soft keywords they activate.
fn render_stdlib_namespaces_section(out: &mut String) {
    start_section(out, "## Standard library namespaces");
    out.push_str("| Namespace | Feature gate | Submodules | Activates soft keywords |\n");
    out.push_str("|---|---|---|---|\n");

    for ns in stdlib::STDLIB_NAMESPACES {
        let namespace = format!("`std.{}`", ns.name);
        let feature_gate = ns.feature.map_or_else(|| "-".to_string(), |f| format!("`{f}`"));
        let submodules = if ns.submodules.is_empty() {
            "-".to_string()
        } else {
            ns.submodules
                .iter()
                .map(|sub| format!("`std.{}.{}`", ns.name, sub))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let namespace_soft_keywords = keywords::soft_keywords_for_namespace(ns.name);
        let soft_keywords = if namespace_soft_keywords.is_empty() {
            "-".to_string()
        } else {
            namespace_soft_keywords
                .iter()
                .map(|id| format!("`{}`", keywords::as_str(*id)))
                .collect::<Vec<_>>()
                .join(", ")
        };

        out.push_str(&format!(
            "| {namespace} | {feature_gate} | {submodules} | {soft_keywords} |\n"
        ));
    }
    out.push('\n');
}

/// Render builtin exception metadata and examples.
fn render_exceptions_section(out: &mut String) {
    start_section(out, "## Builtin exceptions");

    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    for e in errors::EXCEPTIONS {
        let id = format!("{:?}", e.id);
        let canonical = format!("`{}`", e.canonical);
        let aliases = if e.aliases.is_empty() {
            String::new()
        } else {
            e.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = e.description;
        let rfc = e.introduced_in_rfc;
        let since = e.since;
        let stability = format!("{:?}", e.stability);

        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    out.push_str("### Examples\n\n");
    out.push_str("Only exceptions with examples are listed here.\n\n");
    for e in errors::EXCEPTIONS {
        if e.examples.is_empty() {
            continue;
        }
        out.push_str(&format!("#### `{:?}`\n\n", e.id));
        for ex in e.examples {
            out.push_str("```incan\n");
            out.push_str(ex.code);
            out.push_str("\n```\n\n");
            if let Some(note) = ex.note {
                out.push_str(note);
                out.push_str("\n\n");
            }
        }
    }
}

/// Render builtin function metadata.
fn render_builtins_section(out: &mut String) {
    start_section(out, "## Builtin functions");

    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    for b in builtins::BUILTIN_FUNCTIONS {
        let id = format!("{:?}", b.id);
        let canonical = format!("`{}`", b.canonical);
        let aliases = if b.aliases.is_empty() {
            String::new()
        } else {
            b.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = b.description;
        let rfc = b.introduced_in_rfc;
        let since = b.since;
        let stability = format!("{:?}", b.stability);

        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render builtin decorator metadata.
fn render_decorators_section(out: &mut String) {
    start_section(out, "## Decorators");

    out.push_str(
        r#"User-defined decorators are valid on top-level `def` / `async def` declarations and instance methods. A decorator is an ordinary callable value that receives the decorated function or method callable and returns the callable that should replace it:

```incan
def parse(value: int) -> int:
    return value

def as_int(func: (int) -> str) -> (int) -> int:
    return parse

@as_int
def label(value: int) -> str:
    return "value"

def main() -> None:
    result = label(1)  # int
```

Stacked decorators apply bottom-up, matching Python's declaration model: the decorator closest to `def` receives the original function value first, and the outer decorators receive each previous result. Decorator factories such as `@logged("name")` are checked by first evaluating the factory expression as a callable-producing expression and then applying the produced decorator to the function value.

!!! tip "Coming from Python?"
    Python decorators can replace a function with any object. Incan user-defined function decorators are stricter: the decorator input is the decorated callable, and the result must also be callable. Python's `Callable[[A, B], R]` corresponds to Incan's `(A, B) -> R`; `=>` is only for closure expressions, not callable types. Use `(F) -> F` when a decorator preserves the original callable signature, and spell the source and replacement callable types separately when it intentionally changes the signature, such as `((str) -> R) -> ((str, str) -> R)`.

Decorator factories can be generic over the decorated function type. This is the usual shape for registry, catalog, routing, telemetry, and validation decorators that record metadata but return the original function unchanged:

```incan
def registered[F](function_ref: str) -> ((F) -> F):
    return (func) => func

@registered("inql.functions.col")
pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)
```

The compiler infers `F` from the decorated function when the factory result is applied. If inference needs help, pass the decorated function type explicitly on the decorator factory call:

```incan
@registered[(str) -> ColumnExpr]("inql.functions.col")
pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)
```

The post-decoration binding keeps the concrete callable signature of the decorated function unless the decorator deliberately returns a different callable shape. Checked API metadata and imports observe that concrete signature, not the generic helper's `F`.

Method decorators receive an unbound callable shape with the receiver first. A decorator on `def label(self, value: int) -> str` sees `(&Box, int) -> str`; a decorator on `def bump(mut self, value: int) -> int` sees `(&mut Box, int) -> int`. The wrapper passes the actual receiver borrow through to the decorated callable, so method decorators do not require cloning the receiver.

Class, model, trait, enum, newtype, field, alias, and module decorators remain limited to compiler-owned decorators. Compiler-owned decorators such as `@derive`, `@route`, `@rust.extern`, `@rust.allow`, `@staticmethod`, `@classmethod`, and `@requires` keep their existing special behavior.

"#,
    );

    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    for d in decorators::DECORATORS {
        let id = format!("{:?}", d.id);
        let canonical = format!("`@{}`", d.canonical);
        let aliases = if d.aliases.is_empty() {
            String::new()
        } else {
            d.aliases
                .iter()
                .map(|a| format!("`@{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = d.description;
        let rfc = d.introduced_in_rfc;
        let since = d.since;
        let stability = format!("{:?}", d.stability);

        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render builtin derive metadata.
fn render_derives_section(out: &mut String) {
    start_section(out, "## Derives");

    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    for d in derives::DERIVES {
        let id = format!("{:?}", d.id);
        let canonical = format!("`{}`", d.canonical);
        let aliases = if d.aliases.is_empty() {
            String::new()
        } else {
            d.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = d.description;
        let rfc = d.introduced_in_rfc;
        let since = d.since;
        let stability = format!("{:?}", d.stability);

        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render builtin trait metadata.
fn render_traits_section(out: &mut String) {
    start_section(out, "## Builtin traits");

    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    for t in traits::TRAITS {
        let id = format!("{:?}", t.id);
        let canonical = format!("`{}`", t.canonical);
        let aliases = if t.aliases.is_empty() {
            String::new()
        } else {
            t.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = t.description;
        let rfc = t.introduced_in_rfc;
        let since = t.since;
        let stability = format!("{:?}", t.stability);

        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render operator metadata and explanatory notes.
fn render_operators_section(out: &mut String) {
    start_section(out, "## Operators");

    out.push_str("### Notes\n\n");
    out.push_str("- **Precedence**: Higher binds tighter (e.g. `*` > `+`). Values are relative and must be consistent with the parser.\n");
    out.push_str("- **Associativity**: How operators of the same precedence group (left-to-right vs right-to-left).\n");
    out.push_str(
        "- **Fixity**: Whether the operator is used as a prefix unary operator or an infix binary operator.\n",
    );
    out.push_str(
        "- **KeywordSpelling**: Whether the operator token is spelled as a reserved word (e.g. `and`, `not`).\n\n",
    );

    out.push_str(
        "| Id | Spellings | Precedence | Associativity | Fixity | KeywordSpelling | RFC | Since | Stability |\n",
    );
    out.push_str("|---|---|---:|---|---|---|---|---|---|\n");

    for o in operators::OPERATORS {
        let id = format!("{:?}", o.id);
        let spellings = o
            .spellings
            .iter()
            .map(|s| format!("`{}`", s))
            .collect::<Vec<_>>()
            .join(", ");
        let prec = o.precedence;
        let assoc = format!("{:?}", o.associativity);
        let fixity = format!("{:?}", o.fixity);
        let is_kw = o.is_keyword_spelling;
        let rfc = o.introduced_in_rfc;
        let since = o.since;
        let stability = format!("{:?}", o.stability);

        out.push_str(&format!(
            "| {id} | {spellings} | {prec} | {assoc} | {fixity} | {is_kw} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render punctuation metadata.
fn render_punctuation_section(out: &mut String) {
    start_section(out, "## Punctuation");

    out.push_str("| Id | Canonical | Aliases | Category | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    for p in punctuation::PUNCTUATION {
        let id = format!("{:?}", p.id);
        let canonical = format!("`{}`", p.canonical);
        let aliases = if p.aliases.is_empty() {
            String::new()
        } else {
            p.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let category = format!("{:?}", p.category);
        let rfc = p.introduced_in_rfc;
        let since = p.since;
        let stability = format!("{:?}", p.stability);

        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {category} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render builtin type metadata grouped by type family.
fn render_types_section(out: &mut String) {
    start_section(out, "## Builtin types");

    let table_header: &'static str =
        "| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n|---|---|---|---|---|---|---|\n";

    out.push_str("### String-like\n\n");
    out.push_str(table_header);
    for t in stringlike::STRING_LIKE_TYPES {
        let id = format!("{:?}", t.id);
        let canonical = format!("`{}`", t.canonical);
        let aliases = if t.aliases.is_empty() {
            String::new()
        } else {
            t.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = t.description;
        let rfc = t.introduced_in_rfc;
        let since = t.since;
        let stability = format!("{:?}", t.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    out.push_str("\n### Numerics\n\n");
    out.push_str(table_header);
    for t in numerics::NUMERIC_TYPES {
        let id = format!("{:?}", t.id);
        let canonical = format!("`{}`", t.canonical);
        let aliases = if t.aliases.is_empty() {
            String::new()
        } else {
            t.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = t.description;
        let rfc = t.introduced_in_rfc;
        let since = t.since;
        let stability = format!("{:?}", t.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    out.push_str("\n### Collections / generic bases\n\n");
    out.push_str(table_header);
    for t in collections::COLLECTION_TYPES {
        let id = format!("{:?}", t.id);
        let canonical = format!("`{}`", t.canonical);
        let aliases = if t.aliases.is_empty() {
            String::new()
        } else {
            t.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = t.description;
        let rfc = t.introduced_in_rfc;
        let since = t.since;
        let stability = format!("{:?}", t.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render import-free built-in collection helpers such as `list.repeat(...)`.
fn render_builtin_collection_helpers_section(out: &mut String) {
    start_section(out, "## Built-in collection helpers");

    out.push_str("| Id | Receiver | Member | Signature | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|---|---|\n");

    for helper in surface::collection_helpers::BUILTIN_COLLECTION_HELPERS {
        let id = format!("{:?}", helper.item.id);
        let receiver = format!("`{}`", helper.receiver);
        let member = format!("`{}`", helper.member);
        let signature = format!("`{}`", helper.signature);
        let aliases = if helper.item.aliases.is_empty() {
            String::new()
        } else {
            helper
                .item
                .aliases
                .iter()
                .map(|alias| format!("`{}`", alias))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = helper.item.description;
        let rfc = helper.item.introduced_in_rfc;
        let since = helper.item.since;
        let stability = format!("{:?}", helper.item.stability);
        out.push_str(&format!(
            "| {id} | {receiver} | {member} | {signature} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render compiler-recognized surface constructor metadata.
fn render_surface_constructors_section(out: &mut String) {
    start_section(out, "## Surface constructors");

    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    for c in surface::constructors::CONSTRUCTORS {
        let id = format!("{:?}", c.id);
        let canonical = format!("`{}`", c.canonical);
        let aliases = if c.aliases.is_empty() {
            String::new()
        } else {
            c.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = c.description;
        let rfc = c.introduced_in_rfc;
        let since = c.since;
        let stability = format!("{:?}", c.stability);

        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render compiler-recognized surface function metadata.
fn render_surface_functions_section(out: &mut String) {
    start_section(out, "## Surface functions");

    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    for f in surface::functions::SURFACE_FUNCTIONS {
        let id = format!("{:?}", f.id);
        let canonical = format!("`{}`", f.canonical);
        let aliases = if f.aliases.is_empty() {
            String::new()
        } else {
            f.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = f.description;
        let rfc = f.introduced_in_rfc;
        let since = f.since;
        let stability = format!("{:?}", f.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render compiler-recognized string method metadata.
fn render_surface_string_methods_section(out: &mut String) {
    start_section(out, "## Surface string methods");

    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");

    for m in surface::string_methods::STRING_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render compiler-recognized surface type metadata.
fn render_surface_types_section(out: &mut String) {
    start_section(out, "## Surface types");

    out.push_str("| Id | Canonical | Aliases | Kind | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");

    for t in surface::types::SURFACE_TYPES {
        let id = format!("{:?}", t.item.id);
        let canonical = format!("`{}`", t.item.canonical);
        let aliases = if t.item.aliases.is_empty() {
            String::new()
        } else {
            t.item
                .aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let kind = format!("{:?}", t.kind);
        let desc = t.item.description;
        let rfc = t.item.introduced_in_rfc;
        let since = t.item.since;
        let stability = format!("{:?}", t.item.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {kind} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Render compiler-recognized surface methods grouped by receiver type.
fn render_surface_methods_section(out: &mut String) {
    /// Return the common surface-method table header.
    fn table_header() -> &'static str {
        "| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n|---|---|---|---|---|---|---|\n"
    }

    start_section(out, "## Surface methods");

    // Float
    out.push_str("### float methods\n\n");
    out.push_str(table_header());
    for m in surface::float_methods::FLOAT_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    // List
    out.push_str("\n### List methods\n\n");
    out.push_str(table_header());
    for m in surface::list_methods::LIST_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    // Dict
    out.push_str("\n### Dict methods\n\n");
    out.push_str(table_header());
    for m in surface::dict_methods::DICT_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    // Set
    out.push_str("\n### Set methods\n\n");
    out.push_str(table_header());
    for m in surface::set_methods::SET_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    // Option
    out.push_str("\n### Option methods\n\n");
    out.push_str(table_header());
    for m in surface::option_methods::OPTION_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    // Result
    out.push_str("\n### Result methods\n\n");
    out.push_str(table_header());
    for m in surface::result_methods::RESULT_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    // Iterator
    out.push_str("\n### Iterator methods\n\n");
    out.push_str(table_header());
    for m in surface::iterator_methods::ITERATOR_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    // Frozen containers
    out.push_str("\n### FrozenList methods\n\n");
    out.push_str(table_header());
    for m in surface::frozen_list_methods::FROZEN_LIST_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    out.push_str("\n### FrozenDict methods\n\n");
    out.push_str(table_header());
    for m in surface::frozen_dict_methods::FROZEN_DICT_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    out.push_str("\n### FrozenSet methods\n\n");
    out.push_str(table_header());
    for m in surface::frozen_set_methods::FROZEN_SET_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');

    out.push_str("\n### FrozenBytes methods\n\n");
    out.push_str(table_header());
    for m in surface::frozen_bytes_methods::FROZEN_BYTES_METHODS {
        let id = format!("{:?}", m.id);
        let canonical = format!("`{}`", m.canonical);
        let aliases = if m.aliases.is_empty() {
            String::new()
        } else {
            m.aliases
                .iter()
                .map(|a| format!("`{}`", a))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let desc = m.description;
        let rfc = m.introduced_in_rfc;
        let since = m.since;
        let stability = format!("{:?}", m.stability);
        out.push_str(&format!(
            "| {id} | {canonical} | {aliases} | {desc} | {rfc} | {since} | {stability} |\n"
        ));
    }
    out.push('\n');
}

/// Resolve the workspace root directory.
///
/// ## Returns
/// - The workspace root path (two levels above `crates/incan_core`).
///
/// ## Panics
/// - If the path cannot be resolved (this indicates a broken workspace layout).
fn workspace_root() -> PathBuf {
    // crates/incan_core -> crates -> workspace root
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match manifest_dir.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()) {
        Some(path) => path,
        None => panic!("workspace root (two levels above crates/incan_core)"),
    }
}
