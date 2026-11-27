//! Module-level structure: compile-time `when feature(...)` headers, `module tests:` blocks, `import` and `from ...
//! import` forms (`pub` re-exports, `rust::` versions and features, parenthesized item lists (#116), keyword path
//! segments and item names) and RFC 171's targeted refusal of the retired `rust.module()` directive.

use super::*;

fn parse_str_with_module_path(source: &str, module_path: Option<&str>) -> Result<Program, Vec<CompileError>> {
    let tokens = lexer::lex(source).map_err(|_| vec![])?;
    parse_with_module_path(&tokens, module_path)
}

fn require_test_module_decl(decl: &Spanned<Declaration>) -> Result<&TestModuleDecl, Vec<CompileError>> {
    match &decl.node {
        Declaration::TestModule(t) => Ok(t),
        _ => Err(vec![CompileError::new(
            "parser test internal error: expected test module declaration".to_string(),
            decl.span,
        )]),
    }
}

/// Each relative spelling climbs the documented number of directories: a run of `n` dots climbs `n - 1`, `super::`
/// one each, and runs separated by whitespace each climb on their own.
#[test]
fn test_parse_relative_import_levels() -> Result<(), Vec<CompileError>> {
    for (source, levels, segments) in [
        ("from ..common import Logger\n", 1, vec!["common"]),
        ("from ...shared.utils import format_date\n", 2, vec!["shared", "utils"]),
        ("from ....shared import format_date\n", 3, vec!["shared"]),
        ("from .....shared import format_date\n", 4, vec!["shared"]),
        ("from ......shared import format_date\n", 5, vec!["shared"]),
        ("from .. .. shared import format_date\n", 2, vec!["shared"]),
        ("from super::common import Logger\n", 1, vec!["common"]),
        (
            "from super::super::shared::utils import format_date\n",
            2,
            vec!["shared", "utils"],
        ),
    ] {
        let program = parse_str(source)?;
        let Some(Declaration::Import(import)) = program.declarations.first().map(|decl| &decl.node) else {
            panic!("{source}: expected an import declaration");
        };
        let ImportKind::From { module, .. } = &import.kind else {
            panic!("{source}: expected a from-import");
        };
        assert_eq!(module.parent_levels, levels, "{source}");
        assert_eq!(module.segments, segments, "{source}");
        assert!(!module.is_absolute, "{source}");
    }
    Ok(())
}

#[test]
fn test_parse_pub_from_in_src_lib_is_public_reexport() -> Result<(), Vec<CompileError>> {
    let source = "pub from widgets import Widget, Layout as UiLayout\n";
    let program = parse_str_with_module_path(source, Some("project/src/lib.incn"))?;
    assert_eq!(program.declarations.len(), 1);

    let Declaration::Import(import) = &program.declarations[0].node else {
        panic!("Expected import declaration");
    };
    assert!(matches!(import.visibility, Visibility::Public));
    let ImportKind::From { module, items } = &import.kind else {
        panic!("Expected from-import");
    };
    assert_eq!(module.segments, vec!["widgets".to_string()]);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].name, "Widget");
    assert_eq!(items[0].alias, None);
    assert_eq!(items[1].name, "Layout");
    assert_eq!(items[1].alias, Some("UiLayout".to_string()));
    Ok(())
}

#[test]
fn test_parse_pub_from_in_src_main_is_public_reexport() -> Result<(), Vec<CompileError>> {
    let source = "pub from widgets import Widget\n";
    let program = parse_str_with_module_path(source, Some("project/src/main.incn"))?;
    assert_eq!(program.declarations.len(), 1);

    let Declaration::Import(import) = &program.declarations[0].node else {
        panic!("Expected import declaration");
    };
    assert!(matches!(import.visibility, Visibility::Public));
    let ImportKind::From { module, items } = &import.kind else {
        panic!("Expected from-import");
    };
    assert_eq!(module.segments, vec!["widgets".to_string()]);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "Widget");
    assert_eq!(items[0].alias, None);
    Ok(())
}

#[test]
fn test_parse_pub_from_rust_in_src_module_is_public_reexport() -> Result<(), Vec<CompileError>> {
    let source = "pub from rust::time import Instant\n";
    let program = parse_str_with_module_path(source, Some("project/src/session/mod.incn"))?;
    assert_eq!(program.declarations.len(), 1);

    let Declaration::Import(import) = &program.declarations[0].node else {
        panic!("Expected import declaration");
    };
    assert!(matches!(import.visibility, Visibility::Public));
    let ImportKind::RustFrom {
        crate_name,
        path,
        items,
        ..
    } = &import.kind
    else {
        panic!("Expected rust from-import");
    };
    assert_eq!(crate_name, "time");
    assert!(path.is_empty());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "Instant");
    assert_eq!(items[0].alias, None);
    Ok(())
}

#[test]
fn test_parse_pub_from_outside_src_is_error() {
    let source = "pub from widgets import Widget\n";
    let result = parse_str_with_module_path(source, Some("project/tests/test_main.incn"));
    assert!(result.is_err(), "Expected parser to reject `pub from` outside src/");
    let err = result.err().unwrap_or_default();
    assert!(
        err[0].message.contains("only valid in modules under `src/`"),
        "Unexpected error: {}",
        err[0].message
    );
}

#[test]
fn test_parse_pub_import_is_error() {
    let source = "pub import widgets\n";
    let result = parse_str_with_module_path(source, Some("project/src/lib.incn"));
    assert!(result.is_err(), "Expected parser to reject `pub import`");
    let err = result.err().unwrap_or_default();
    assert!(
        err[0].message.contains("only supported on `from ... import ...`"),
        "Unexpected error: {}",
        err[0].message
    );
}

// ---- rust.module() edge case tests ----

#[test]
fn module_import_parse_cases() {
    run_parse_cases(MODULE_IMPORT_PARSE_CASES);
}

const MODULE_IMPORT_PARSE_CASES: &[ParseCase] = &[
    ParseCase {
        name: "parses_and_projects_compilation_unit_feature_conditions",
        source: r#"
when feature("json"):
    from std.json import JsonValue

    when feature("pretty"):
        pub def render(value: JsonValue) -> str:
            return "json"

pub def always() -> str:
    return "always"
"#,
        check: |_source, program| {
            assert_eq!(program.declarations.len(), 3);
            assert_eq!(program.declarations[0].required_features, ["json"]);
            assert_eq!(program.declarations[1].required_features, ["json", "pretty"]);
            assert!(program.declarations[2].required_features.is_empty());

            let json_only = program.projected_for_features(&std::collections::BTreeSet::from(["json".to_string()]));
            assert_eq!(json_only.declarations.len(), 2);
            assert!(matches!(json_only.declarations[0].node, Declaration::Import(_)));
            assert!(matches!(json_only.declarations[1].node, Declaration::Function(_)));
            Ok(())
        },
    },
    ParseCase {
        name: "keeps_when_contextual_outside_compile_time_headers",
        source: "def identity(when: int) -> int:\n    return when\n",
        check: |_source, program| {
            assert!(matches!(program.declarations[0].node, Declaration::Function(_)));
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_module_tests_block",
        source: r#"
def add(a: int, b: int) -> int:
  return a + b

module tests:
  from testing import assert_eq

  def test_add() -> None:
    assert add(1, 2) == 3
"#,
        check: |_source, program| {
            assert_eq!(program.declarations.len(), 2);
            let test_module = require_test_module_decl(&program.declarations[1])?;
            assert_eq!(test_module.name, "tests");
            assert_eq!(test_module.body.len(), 2);
            assert!(matches!(test_module.body[0].node, Declaration::Import(_)));
            assert!(matches!(test_module.body[1].node, Declaration::Function(_)));
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_import",
        source: "import polars::prelude as pl",
        check: |_source, program| {
            assert_eq!(program.declarations.len(), 1);
            match &program.declarations[0].node {
                Declaration::Import(i) => {
                    match &i.kind {
                        ImportKind::Module(path) => {
                            assert_eq!(path.segments, vec!["polars".to_string(), "prelude".to_string()]);
                            assert_eq!(path.parent_levels, 0);
                            assert!(!path.is_absolute);
                        }
                        _ => panic!("Expected module import"),
                    }
                    assert_eq!(i.alias, Some("pl".to_string()));
                }
                _ => panic!("Expected import"),
            }
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_rust_import_with_version_and_features",
        source: r#"import rust::tokio @ "1.0" with ["full", "macros"] as rt"#,
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::RustCrate {
                        crate_name,
                        path,
                        version,
                        features,
                    } => {
                        assert_eq!(crate_name, "tokio");
                        assert!(path.is_empty());
                        assert_eq!(version.as_deref(), Some("1.0"));
                        assert_eq!(features, &vec!["full".to_string(), "macros".to_string()]);
                        assert_eq!(i.alias, Some("rt".to_string()));
                    }
                    _ => panic!("Expected rust crate import"),
                },
                _ => panic!("Expected import"),
            }
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_rust_from_with_version_and_features",
        source: r#"from rust::time @ "0.3" with ["formatting"] import Instant"#,
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::RustFrom {
                        crate_name,
                        path,
                        version,
                        features,
                        items,
                    } => {
                        assert_eq!(crate_name, "time");
                        assert!(path.is_empty());
                        assert_eq!(version.as_deref(), Some("0.3"));
                        assert_eq!(features, &vec!["formatting".to_string()]);
                        assert_eq!(items.len(), 1);
                        assert_eq!(items[0].name, "Instant");
                    }
                    _ => panic!("Expected rust from import"),
                },
                _ => panic!("Expected import"),
            }
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_rust_import_with_features_without_inline_version",
        source: r#"import rust::tokio with ["full"]"#,
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(import) => match &import.kind {
                    ImportKind::RustCrate {
                        crate_name,
                        version,
                        features,
                        ..
                    } => {
                        assert_eq!(crate_name, "tokio");
                        assert_eq!(version, &None);
                        assert_eq!(features, &vec!["full".to_string()]);
                    }
                    _ => panic!("Expected rust module import"),
                },
                _ => panic!("Expected import"),
            }
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_pub_library_import_with_alias",
        source: "import pub::mylib as lib\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::PubLibrary { library, path } => {
                        assert_eq!(library, "mylib");
                        assert!(path.is_empty());
                        assert_eq!(i.alias.as_deref(), Some("lib"));
                    }
                    _ => panic!("Expected pub library import"),
                },
                _ => panic!("Expected import"),
            }
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_pub_from_import_parenthesized_items",
        source: "from pub::mylib import (\n    Widget,\n    make_widget as build_widget,\n)\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::PubFrom { library, path, items } => {
                        assert_eq!(library, "mylib");
                        assert!(path.is_empty());
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0].name, "Widget");
                        assert_eq!(items[0].alias, None);
                        assert_eq!(items[1].name, "make_widget");
                        assert_eq!(items[1].alias.as_deref(), Some("build_widget"));
                    }
                    _ => panic!("Expected pub from import"),
                },
                _ => panic!("Expected import"),
            }
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_pub_library_module_namespace",
        source: "from pub::mylib.widgets.codec import Widget\n",
        check: |_source, program| {
            let Declaration::Import(import) = &program.declarations[0].node else {
                return Err(vec![CompileError::new(
                    "expected public module import declaration".to_string(),
                    program.declarations[0].span,
                )]);
            };
            let ImportKind::PubFrom { library, path, items } = &import.kind else {
                return Err(vec![CompileError::new(
                    "expected public module from-import".to_string(),
                    program.declarations[0].span,
                )]);
            };
            assert_eq!(library, "mylib");
            assert_eq!(path, &["widgets".to_string(), "codec".to_string()]);
            assert_eq!(items[0].name, "Widget");
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_pub_import_nested_namespace_with_colon_separator",
        source: "from pub::mylib::widgets import Widget\n",
        check: |_source, program| {
            let Declaration::Import(import) = &program.declarations[0].node else {
                return Err(vec![CompileError::new(
                    "expected public module import declaration".to_string(),
                    program.declarations[0].span,
                )]);
            };
            let ImportKind::PubFrom { library, path, items } = &import.kind else {
                return Err(vec![CompileError::new(
                    "expected public module from-import".to_string(),
                    program.declarations[0].span,
                )]);
            };
            assert_eq!(library, "mylib");
            assert_eq!(path, &["widgets".to_string()]);
            assert_eq!(items[0].name, "Widget");
            Ok(())
        },
    },
    // RFC 005: `from rust.crate import Item` emits a warning and parses successfully.
    ParseCase {
        name: "test_parse_rust_from_import_dot_notation_is_warning",
        source: "from rust.chrono import Utc\n",
        check: |_source, program| {
            assert_eq!(program.warnings.len(), 1, "Expected exactly one warning");
            assert!(
                program.warnings[0].message.contains("::"),
                "Expected warning to mention '::' notation; got: {}",
                program.warnings[0].message
            );
            Ok(())
        },
    },
    // RFC 005: `import rust.crate` emits a warning and parses successfully.
    ParseCase {
        name: "test_parse_rust_import_dot_notation_is_warning",
        source: "import rust.serde_json\n",
        check: |_source, program| {
            assert_eq!(program.warnings.len(), 1, "Expected exactly one warning");
            assert!(
                program.warnings[0].message.contains("::"),
                "Expected warning to mention '::' notation; got: {}",
                program.warnings[0].message
            );
            Ok(())
        },
    },
    // RFC 005: `import rust.std.time` (multi-segment bare import, dot path) recovers fully.
    //
    // Mirrors `test_parse_rust_from_import_multi_dot_notation_is_warning` but for the bare  `import rust.X.Y` form,
    // ensuring `rust_crate_path()` dot-recovery works on both branches.
    ParseCase {
        name: "test_parse_rust_import_multi_dot_notation_is_warning",
        source: "import rust.std.time\n",
        check: |_source, program| {
            assert_eq!(
                program.warnings.len(),
                1,
                "Expected exactly one warning for the leading dot"
            );
            assert!(
                program.warnings[0].message.contains("::"),
                "Expected warning to mention '::' notation; got: {}",
                program.warnings[0].message
            );
            // Verify the path was correctly decomposed: crate=std, path=[time]
            if let Some(decl) = program.declarations.first()
                && let crate::ast::Declaration::Import(import) = &decl.node
            {
                assert!(
                    matches!(
                        &import.kind,
                        crate::ast::ImportKind::RustCrate { crate_name, path, .. }
                        if crate_name == "std" && path == &["time".to_string()]
                    ),
                    "Expected RustCrate {{ crate_name: std, path: [time] }}; got: {:?}",
                    import.kind
                );
            }
            Ok(())
        },
    },
    // RFC 005: `from rust.std.time import Instant` (multi-segment dot path) recovers fully.
    //
    // `rust_crate_path()` accepts both `::` and `.` as separators, so the entire dotted path is consumed and no
    // cascading parse error occurs.
    ParseCase {
        name: "test_parse_rust_from_import_multi_dot_notation_is_warning",
        source: "from rust.std.time import Instant\n",
        check: |_source, program| {
            assert_eq!(
                program.warnings.len(),
                1,
                "Expected exactly one warning for the leading dot"
            );
            assert!(
                program.warnings[0].message.contains("::"),
                "Expected warning to mention '::' notation; got: {}",
                program.warnings[0].message
            );
            Ok(())
        },
    },
    // Single identifier in parentheses: `from db import (CategoryId)`.
    ParseCase {
        name: "test_parse_from_import_parenthesized_single_item",
        source: "from db import (CategoryId)\n",
        check: |_source, program| {
            assert_eq!(program.declarations.len(), 1);
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::From { items, .. } => {
                        assert_eq!(items.len(), 1);
                        assert_eq!(items[0].name, "CategoryId");
                        assert_eq!(items[0].alias, None);
                    }
                    _ => panic!("Expected From import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    // Multiple identifiers in parentheses on one line: `from db import (CategoryId, TagId)`.
    ParseCase {
        name: "test_parse_from_import_parenthesized_multi_item_single_line",
        source: "from db import (CategoryId, TagId)\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::From { items, .. } => {
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0].name, "CategoryId");
                        assert_eq!(items[1].name, "TagId");
                    }
                    _ => panic!("Expected From import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    // Multi-line parenthesized import — the lexer drops newlines inside `(...)` so the parser sees the same token
    // stream as the single-line version.
    ParseCase {
        name: "test_parse_from_import_parenthesized_multi_line",
        source: "from db import (\n    CategoryId,\n    TagId,\n    OtherId\n)\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::From { items, .. } => {
                        assert_eq!(items.len(), 3);
                        assert_eq!(items[0].name, "CategoryId");
                        assert_eq!(items[1].name, "TagId");
                        assert_eq!(items[2].name, "OtherId");
                    }
                    _ => panic!("Expected From import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    // Trailing comma before `)` is allowed: `from db import (CategoryId, TagId,)`.
    ParseCase {
        name: "test_parse_from_import_parenthesized_trailing_comma",
        source: "from db import (CategoryId, TagId,)\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::From { items, .. } => {
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0].name, "CategoryId");
                        assert_eq!(items[1].name, "TagId");
                    }
                    _ => panic!("Expected From import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    // Items with `as` aliases in a parenthesized list.
    ParseCase {
        name: "test_parse_from_import_parenthesized_with_aliases",
        source: "from db import (\n    CategoryId as CatId,\n    TagId,\n)\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::From { items, .. } => {
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0].name, "CategoryId");
                        assert_eq!(items[0].alias, Some("CatId".to_string()));
                        assert_eq!(items[1].name, "TagId");
                        assert_eq!(items[1].alias, None);
                    }
                    _ => panic!("Expected From import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    // `from rust::...` also supports parenthesized items.
    ParseCase {
        name: "test_parse_rust_from_import_parenthesized",
        source: "from rust::serde_json import (\n    Value,\n    Map,\n)\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::RustFrom { crate_name, items, .. } => {
                        assert_eq!(crate_name, "serde_json");
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0].name, "Value");
                        assert_eq!(items[1].name, "Map");
                    }
                    _ => panic!("Expected RustFrom import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    // Mixed aliased/non-aliased items work in parenthesized `from rust::` imports.
    ParseCase {
        name: "test_parse_rust_from_import_parenthesized_mixed_aliases",
        source: "from rust::polars import (\n    DataFrame,\n    Series as S,\n    LazyFrame as LF,\n    Expr,\n)\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::RustFrom { crate_name, items, .. } => {
                        assert_eq!(crate_name, "polars");
                        assert_eq!(items.len(), 4);
                        assert_eq!(items[0].name, "DataFrame");
                        assert_eq!(items[0].alias, None);
                        assert_eq!(items[1].name, "Series");
                        assert_eq!(items[1].alias, Some("S".to_string()));
                        assert_eq!(items[2].name, "LazyFrame");
                        assert_eq!(items[2].alias, Some("LF".to_string()));
                        assert_eq!(items[3].name, "Expr");
                        assert_eq!(items[3].alias, None);
                    }
                    _ => panic!("Expected RustFrom import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    // `from rust::...` with version/feature specifiers also supports parenthesized items.
    ParseCase {
        name: "test_parse_rust_from_import_with_version_and_parens",
        source: "from rust::serde_json @ \"1.0\" with [\"derive\"] import (\n    Value,\n    Map,\n)\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::RustFrom {
                        crate_name,
                        version,
                        features,
                        items,
                        ..
                    } => {
                        assert_eq!(crate_name, "serde_json");
                        assert_eq!(version.as_deref(), Some("1.0"));
                        assert_eq!(features, &["derive".to_string()]);
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0].name, "Value");
                        assert_eq!(items[1].name, "Map");
                    }
                    _ => panic!("Expected RustFrom import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    // Rust item paths may use module names that are Incan keywords (e.g. Substrait `proto::type`).
    ParseCase {
        name: "test_parse_rust_from_import_path_type_keyword_segment",
        source: "from rust::substrait::proto::type import Binary, Boolean\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::RustFrom {
                        crate_name,
                        path,
                        items,
                        ..
                    } => {
                        assert_eq!(crate_name, "substrait");
                        assert_eq!(path, &["proto".to_string(), "type".to_string()]);
                        assert_eq!(items.len(), 2);
                        assert_eq!(items[0].name, "Binary");
                        assert_eq!(items[1].name, "Boolean");
                    }
                    _ => panic!("Expected RustFrom import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    ParseCase {
        name: "test_parse_rust_import_crate_path_type_keyword_segment",
        source: "import rust::substrait::proto::type::Binary\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::RustCrate {
                        crate_name,
                        path,
                        version,
                        features,
                    } => {
                        assert_eq!(crate_name, "substrait");
                        assert_eq!(path, &["proto".to_string(), "type".to_string(), "Binary".to_string()]);
                        assert!(version.is_none());
                        assert!(features.is_empty());
                    }
                    _ => panic!("Expected RustCrate import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
    // `from rust::... import` may name Rust items that are Incan keywords (e.g. `type` module).
    ParseCase {
        name: "test_parse_rust_from_import_keyword_item_with_alias",
        source: "from rust::substrait::proto import type as proto_type\n",
        check: |_source, program| {
            match &program.declarations[0].node {
                Declaration::Import(i) => match &i.kind {
                    ImportKind::RustFrom {
                        crate_name,
                        path,
                        items,
                        ..
                    } => {
                        assert_eq!(crate_name, "substrait");
                        assert_eq!(path, &["proto".to_string()]);
                        assert_eq!(items.len(), 1);
                        assert_eq!(items[0].name, "type");
                        assert_eq!(items[0].alias.as_deref(), Some("proto_type"));
                    }
                    _ => panic!("Expected RustFrom import"),
                },
                _ => panic!("Expected import declaration"),
            }
            Ok(())
        },
    },
];

#[test]
fn module_import_refusal_cases() {
    run_refusal_cases(MODULE_IMPORT_REFUSAL_CASES);
}

const MODULE_IMPORT_REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        name: "rejects_non_feature_compile_time_predicates",
        source: "when target(\"linux\"):\n    const VALUE = 1\n",
        check: |_source, errors| {
            assert!(errors.iter().any(|error| error.message.contains("only `feature")));
        },
    },
    RefusalCase {
        name: "rejects_invalid_compile_time_feature_names",
        source: "when feature(\"dependency/name\"):\n    const VALUE = 1\n",
        check: |_source, errors| {
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("Invalid package feature name"))
            );
        },
    },
    RefusalCase {
        name: "test_duplicate_module_tests_block_is_error",
        source: r#"
module tests:
  pass

module tests:
  pass
"#,
        check: |_source, errors| {
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("Only one `module tests:` block is allowed")),
                "expected duplicate module tests error, got: {:?}",
                errors.iter().map(|error| &error.message).collect::<Vec<_>>()
            );
        },
    },
    RefusalCase {
        name: "test_parse_pub_import_dot_notation_is_error",
        source: "from pub.mylib import Widget\n",
        check: |_source, err| {
            assert!(
                err[0].message.contains("Expected `::` after `pub`"),
                "Unexpected error: {}",
                err[0].message
            );
        },
    },
    // Missing `)` produces a parse error that mentions the closing delimiter.
    RefusalCase {
        name: "test_parse_from_import_parenthesized_unclosed_error",
        source: "from db import (CategoryId, TagId\n",
        check: |_source, err| {
            assert!(
                err[0].message.contains(')') || err[0].message.to_lowercase().contains("close"),
                "Expected error to mention ')'; got: {}",
                err[0].message
            );
        },
    },
    // Empty parenthesized list `from db import ()` is a parse error.
    RefusalCase {
        name: "test_parse_from_import_empty_parens_error",
        source: "from db import ()\n",
        check: |_source, err| {
            assert!(
                err[0].message.to_lowercase().contains("empty") || err[0].message.to_lowercase().contains("cannot"),
                "Expected 'empty' diagnostic; got: {}",
                err[0].message
            );
        },
    },
    RefusalCase {
        name: "test_parse_from_import_rejects_keyword_item_name_for_incan_modules",
        source: "from db import type\n",
        check: |_source, _errors| {},
    },
    RefusalCase {
        name: "test_rust_module_directive_is_retired",
        source: "rust.module(\"incan_std_testing\")\n\ndef foo() -> int:\n    return 1\n",
        check: |_source, errors| {
            assert!(errors.iter().any(|error| {
                error
                    .message
                    .contains("`rust.module(...)` has been removed; import each Rust item through `rust::` instead")
            }));
        },
    },
    RefusalCase {
        name: "test_rust_module_directive_after_docstring_is_retired",
        source: "\"Module docstring\"\nrust.module(\"my_crate::sub\")\n\ndef bar() -> str:\n    return \"hi\"\n",
        check: |_source, errors| {
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("`rust.module(...)` has been removed"))
            );
        },
    },
    RefusalCase {
        name: "test_rust_module_directive_duplicate_is_error",
        source: "rust.module(\"crate_a\")\nrust.module(\"crate_b\")\n\ndef foo() -> int:\n    return 1\n",
        check: |_source, err| {
            let has_duplicate_msg = err
                .iter()
                .any(|e| e.message.contains("`rust.module(...)` has been removed"));
            assert!(
                has_duplicate_msg,
                "Should report retired rust.module(); errors: {:?}",
                err.iter().map(|e| &e.message).collect::<Vec<_>>()
            );
        },
    },
    RefusalCase {
        name: "test_rust_module_directive_not_at_top_is_error",
        source: "def foo() -> int:\n    return 1\n\nrust.module(\"incan_std_testing\")\n",
        check: |_source, err| {
            let has_msg = err
                .iter()
                .any(|e| e.message.contains("`rust.module(...)` has been removed"));
            assert!(
                has_msg,
                "Should report retired rust.module(); errors: {:?}",
                err.iter().map(|e| &e.message).collect::<Vec<_>>()
            );
        },
    },
    RefusalCase {
        name: "test_rust_module_missing_parens",
        source: "rust.module \"foo\"\n\ndef bar() -> int:\n    return 1\n",
        check: |_source, _errors| {},
    },
    RefusalCase {
        name: "test_rust_module_non_string_arg",
        source: "rust.module(42)\n\ndef bar() -> int:\n    return 1\n",
        check: |_source, _errors| {},
    },
    RefusalCase {
        name: "test_rust_module_empty_string",
        source: "rust.module(\"\")\n\n@rust.extern\ndef bar() -> int:\n    ...\n",
        check: |_source, _errors| {},
    },
    RefusalCase {
        name: "test_rust_module_missing_closing_paren",
        source: "rust.module(\"foo\"\n\ndef bar() -> int:\n    return 1\n",
        check: |_source, _errors| {},
    },
];
