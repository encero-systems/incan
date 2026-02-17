//! Generate Markdown reference docs from `incan_core::lang` registries.
//!
//! This binary renders the vocabulary registries (keywords, operators, builtin functions, builtin types, punctuation)
//! into human-readable Markdown tables under `workspaces/docs-site/docs/language/reference/`.
//!
//! ## Notes
//! - The generated files are meant to be checked into the repo and treated as derived artifacts.
//! - Do not edit the generated Markdown by hand; update the registries instead.
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
use incan_core::lang::{builtins, derives, errors, keywords, operators, punctuation, stdlib, surface, traits};

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
}

/// Write `workspaces/docs-site/docs/language/reference/language.md`.
///
/// This is a single consolidated reference document generated from `incan_core::lang` registries.
fn write_language_reference(path: &Path) {
    let mut out = String::new();
    out.push_str("# Incan language reference\n\n");
    out.push_str("!!! warning \"Generated file\"\n");
    out.push_str("    Do not edit this page by hand.\n");
    out.push_str("    If it looks wrong/outdated, regenerate it from source and commit the result.\n");
    out.push('\n');
    out.push_str("    Regenerate with: `cargo run -p incan_core --bin generate_lang_reference`\n\n");

    out.push_str("## Contents\n\n");
    out.push_str("- [Keywords](#keywords)\n");
    out.push_str("- [Soft keywords](#soft-keywords)\n");
    out.push_str("- [Standard library namespaces](#standard-library-namespaces)\n");
    out.push_str("- [Builtin exceptions](#builtin-exceptions)\n");
    out.push_str("- [Builtin functions](#builtin-functions)\n");
    out.push_str("- [Derives](#derives)\n");
    out.push_str("- [Builtin traits](#builtin-traits)\n");
    out.push_str("- [Operators](#operators)\n");
    out.push_str("- [Punctuation](#punctuation)\n");
    out.push_str("- [Builtin types](#builtin-types)\n");
    out.push_str("- [Surface constructors](#surface-constructors)\n");
    out.push_str("- [Surface functions](#surface-functions)\n");
    out.push_str("- [Surface math](#surface-math)\n");
    out.push_str("- [Surface string methods](#surface-string-methods)\n");
    out.push_str("- [Surface types](#surface-types)\n");
    out.push_str("- [Surface methods](#surface-methods)\n\n");

    render_keywords_section(&mut out);
    render_soft_keywords_section(&mut out);
    render_stdlib_namespaces_section(&mut out);
    render_exceptions_section(&mut out);
    render_builtins_section(&mut out);
    render_derives_section(&mut out);
    render_traits_section(&mut out);
    render_operators_section(&mut out);
    render_punctuation_section(&mut out);
    render_types_section(&mut out);

    render_surface_constructors_section(&mut out);
    render_surface_functions_section(&mut out);
    render_surface_math_section(&mut out);
    render_surface_string_methods_section(&mut out);
    render_surface_types_section(&mut out);
    render_surface_methods_section(&mut out);

    trim_trailing_newlines_to_at_most_two(&mut out);
    out.push('\n');
    if let Err(err) = fs::write(path, out) {
        panic!("write language.md: {err}");
    }
}

fn render_keywords_section(out: &mut String) {
    start_section(out, "## Keywords");

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
        let reservation = if keywords::is_soft(k.id) { "Soft" } else { "Hard" };
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

fn render_surface_math_section(out: &mut String) {
    start_section(out, "## Surface math");

    out.push_str("### Functions\n\n");
    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for f in surface::math::MATH_FUNCTIONS {
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

    out.push_str("\n### Constants\n\n");
    out.push_str("| Id | Canonical | Aliases | Description | RFC | Since | Stability |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for c in surface::math::MATH_CONSTANTS {
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

fn render_surface_methods_section(out: &mut String) {
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
