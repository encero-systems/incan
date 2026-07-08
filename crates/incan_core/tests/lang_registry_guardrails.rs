use std::collections::HashMap;

use incan_core::lang::builtins;
use incan_core::lang::derives;
use incan_core::lang::errors;
use incan_core::lang::features;
use incan_core::lang::keywords;
use incan_core::lang::magic_methods;
use incan_core::lang::operators;
use incan_core::lang::punctuation;
use incan_core::lang::registry::{RFC, Since};
use incan_core::lang::surface::types::{SurfaceTypeCategory, SurfaceTypeId, SurfaceTypeOwner};
use incan_core::lang::surface::{constructors, functions, iterator_methods, result_methods, types as surface_types};
use incan_core::lang::testing;
use incan_core::lang::traits;
use incan_core::lang::types::{collections, numerics, stringlike};
use std::path::{Path, PathBuf};

struct RegistryRoundTrip<'a, Id, Info> {
    label: &'a str,
    expected_len: usize,
    items: &'a [Info],
    id_of: fn(&Info) -> Id,
    canonical_of: fn(&Info) -> &'static str,
    aliases_of: fn(&Info) -> &'static [&'static str],
    from_str: fn(&str) -> Option<Id>,
    as_str: fn(Id) -> &'static str,
}

fn assert_registry_round_trip<Id, Info>(registry: RegistryRoundTrip<'_, Id, Info>)
where
    Id: Copy + Eq + std::fmt::Debug,
{
    let RegistryRoundTrip {
        label,
        expected_len,
        items,
        id_of,
        canonical_of,
        aliases_of,
        from_str,
        as_str,
    } = registry;

    assert_eq!(items.len(), expected_len, "{label} table length changed");

    let mut seen: HashMap<&'static str, Id> = HashMap::new();

    for info in items {
        let id = id_of(info);
        let canonical = canonical_of(info);
        assert_eq!(
            from_str(canonical),
            Some(id),
            "{label} canonical spelling not resolvable: {canonical}"
        );
        assert_eq!(as_str(id), canonical, "{label} as_str mismatch for {:?}", id);

        if let Some(prev) = seen.insert(canonical, id) {
            panic!("duplicate {label} spelling {:?}: {:?} and {:?}", canonical, prev, id);
        }

        for &alias in aliases_of(info) {
            assert_eq!(from_str(alias), Some(id), "{label} alias not resolvable: {alias}");
            if let Some(prev) = seen.insert(alias, id) {
                panic!("duplicate {label} alias spelling {:?}: {:?} and {:?}", alias, prev, id);
            }
        }
    }
}

#[test]
fn keywords_spellings_unique_and_resolvable() {
    let mut seen: HashMap<&'static str, keywords::KeywordId> = HashMap::new();

    for info in keywords::KEYWORDS {
        assert_eq!(
            keywords::from_str(info.canonical),
            Some(info.id),
            "keyword canonical spelling not resolvable: {}",
            info.canonical
        );
        assert_eq!(
            keywords::as_str(info.id),
            info.canonical,
            "keyword as_str mismatch for {:?}",
            info.id
        );

        if let Some(prev) = seen.insert(info.canonical, info.id) {
            panic!(
                "duplicate keyword spelling {:?}: {:?} and {:?}",
                info.canonical, prev, info.id
            );
        }

        for &alias in info.aliases {
            assert_eq!(
                keywords::from_str(alias),
                Some(info.id),
                "keyword alias not resolvable: {}",
                alias
            );
            if let Some(prev) = seen.insert(alias, info.id) {
                panic!(
                    "duplicate keyword alias spelling {:?}: {:?} and {:?}",
                    alias, prev, info.id
                );
            }
        }
    }
}

#[test]
fn builtins_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "builtin",
        expected_len: 18,
        items: builtins::BUILTIN_FUNCTIONS,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: builtins::from_str,
        as_str: builtins::as_str,
    });
}

#[test]
fn exceptions_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "exception",
        expected_len: 7,
        items: errors::EXCEPTIONS,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: errors::from_str,
        as_str: errors::as_str,
    });
}

#[test]
fn operators_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "operator",
        expected_len: 42,
        items: operators::OPERATORS,
        id_of: |info| info.id,
        canonical_of: |info| info.spellings[0],
        aliases_of: |info| &info.spellings[1..],
        from_str: operators::from_str,
        as_str: |id| operators::info_for(id).spellings[0],
    });
}

#[test]
fn punctuation_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "punctuation",
        expected_len: 16,
        items: punctuation::PUNCTUATION,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: punctuation::from_str,
        as_str: punctuation::as_str,
    });
}

#[test]
fn punctuation_provenance_matches_introducing_release() {
    let expected = [
        (punctuation::PunctuationId::Comma, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::Colon, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::Question, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::At, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::Pipe, RFC::_040, Since(0, 3)),
        (punctuation::PunctuationId::Dot, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::ColonColon, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::Arrow, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::FatArrow, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::Ellipsis, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::LParen, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::RParen, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::LBracket, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::RBracket, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::LBrace, RFC::_000, Since(0, 1)),
        (punctuation::PunctuationId::RBrace, RFC::_000, Since(0, 1)),
    ];

    assert_eq!(expected.len(), punctuation::PUNCTUATION.len());

    for (id, introduced_in_rfc, since) in expected {
        let info = punctuation::info_for(id);
        assert_eq!(info.introduced_in_rfc, introduced_in_rfc, "wrong RFC for {id:?}");
        assert_eq!(info.since, since, "wrong since-version for {id:?}");
    }
}

#[test]
fn types_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "stringlike",
        expected_len: 5,
        items: stringlike::STRING_LIKE_TYPES,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: stringlike::from_str,
        as_str: stringlike::as_str,
    });

    assert_registry_round_trip(RegistryRoundTrip {
        label: "numeric",
        expected_len: 15,
        items: numerics::NUMERIC_TYPES,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: numerics::from_str,
        as_str: numerics::as_str,
    });

    assert_registry_round_trip(RegistryRoundTrip {
        label: "collection",
        expected_len: 10,
        items: collections::COLLECTION_TYPES,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: collections::from_str,
        as_str: collections::as_str,
    });
}

#[test]
fn collection_rust_display_bases_are_not_ordinary_source_aliases() {
    assert_eq!(
        collections::from_rust_display_base("std::collections::HashSet"),
        Some(collections::CollectionTypeId::Set)
    );
    assert_eq!(
        collections::from_rust_display_base("HashMap"),
        Some(collections::CollectionTypeId::Dict)
    );
    assert_eq!(collections::from_str("HashSet"), None);
}

#[test]
fn derives_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "derive",
        expected_len: 11,
        items: derives::DERIVES,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |_| &[],
        from_str: derives::from_str,
        as_str: derives::as_str,
    });
}

#[test]
fn traits_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "trait",
        expected_len: 19,
        items: traits::TRAITS,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |_| &[],
        from_str: traits::from_str,
        as_str: traits::as_str,
    });
}

#[test]
fn feature_inventory_has_required_metadata() {
    assert_eq!(features::FEATURES.len(), 53, "feature inventory length changed");

    let mut seen = HashMap::new();
    for feature in features::FEATURES {
        if let Some(prev) = seen.insert(feature.name, feature.id) {
            panic!(
                "duplicate feature name {:?}: {:?} and {:?}",
                feature.name, prev, feature.id
            );
        }
        assert!(
            !feature.activation.trim().is_empty(),
            "feature {:?} is missing activation guidance",
            feature.id
        );
        assert!(
            !feature.summary.trim().is_empty(),
            "feature {:?} is missing a summary",
            feature.id
        );
        assert!(
            !feature.prefer_over.trim().is_empty(),
            "feature {:?} is missing prefer-over guidance",
            feature.id
        );
        assert!(
            !feature.references.is_empty(),
            "feature {:?} should link to at least one reference page",
            feature.id
        );
        assert!(
            !feature.canonical_forms.is_empty(),
            "feature {:?} should list at least one canonical form",
            feature.id
        );
    }
}

#[test]
fn magic_methods_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "magic method",
        expected_len: 7,
        items: magic_methods::MAGIC_METHODS,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |_| &[],
        from_str: magic_methods::from_str,
        as_str: magic_methods::as_str,
    });
}

#[test]
fn constructors_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "constructor",
        expected_len: 4,
        items: constructors::CONSTRUCTORS,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: constructors::from_str,
        as_str: constructors::as_str,
    });
}

#[test]
fn surface_functions_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "surface function",
        expected_len: 10,
        items: functions::SURFACE_FUNCTIONS,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: functions::from_str,
        as_str: functions::as_str,
    });
}

#[test]
fn iterator_methods_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "iterator method",
        expected_len: 21,
        items: iterator_methods::ITERATOR_METHODS,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: iterator_methods::from_str,
        as_str: iterator_methods::as_str,
    });
}

#[test]
fn result_methods_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "result method",
        expected_len: 8,
        items: result_methods::RESULT_METHODS,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: result_methods::from_str,
        as_str: result_methods::as_str,
    });
}

#[test]
fn testing_assert_helpers_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "testing assert helper",
        expected_len: 9,
        items: testing::TESTING_ASSERT_HELPERS,
        id_of: |info| info.id,
        canonical_of: |info| info.canonical,
        aliases_of: |info| info.aliases,
        from_str: testing::assert_helper_from_str,
        as_str: testing::assert_helper_as_str,
    });
}

#[test]
fn surface_types_spellings_unique_and_resolvable() {
    assert_registry_round_trip(RegistryRoundTrip {
        label: "surface type",
        expected_len: 23,
        items: surface_types::SURFACE_TYPES,
        id_of: |info| info.item.id,
        canonical_of: |info| info.item.canonical,
        aliases_of: |info| info.item.aliases,
        from_str: surface_types::from_str,
        as_str: surface_types::as_str,
    });
}

#[test]
fn surface_types_have_explicit_ownership_metadata() {
    for info in surface_types::SURFACE_TYPES {
        let id = info.item.id;

        assert_eq!(surface_types::owner(id), info.ownership.owner);
        assert_eq!(surface_types::category(id), info.ownership.category);
        assert_eq!(surface_types::stdlib_module_path(id), info.ownership.stdlib_module_path);
        assert!(
            !info.ownership.rationale.trim().is_empty(),
            "surface type {:?} must explain why incan_core owns its spelling",
            id
        );
    }

    let globally_available: Vec<SurfaceTypeId> = surface_types::SURFACE_TYPES
        .iter()
        .filter(|info| surface_types::is_global(info.item.id))
        .map(|info| info.item.id)
        .collect();
    assert_eq!(
        globally_available,
        vec![
            SurfaceTypeId::Vec,
            SurfaceTypeId::HashMap,
            SurfaceTypeId::ValidationError
        ]
    );

    let interop_types: Vec<SurfaceTypeId> = surface_types::types_for_owner(SurfaceTypeOwner::Interop)
        .map(|info| info.item.id)
        .collect();
    assert_eq!(interop_types, vec![SurfaceTypeId::Vec, SurfaceTypeId::HashMap]);

    let web_types: Vec<SurfaceTypeId> = surface_types::types_in_category(SurfaceTypeCategory::Web)
        .map(|info| info.item.id)
        .collect();
    assert_eq!(
        web_types,
        vec![
            SurfaceTypeId::App,
            SurfaceTypeId::Response,
            SurfaceTypeId::Html,
            SurfaceTypeId::Json,
            SurfaceTypeId::Query,
            SurfaceTypeId::Path,
            SurfaceTypeId::Body,
            SurfaceTypeId::Request,
        ]
    );

    let validation_types: Vec<SurfaceTypeId> = surface_types::types_in_category(SurfaceTypeCategory::Validation)
        .map(|info| info.item.id)
        .collect();
    assert_eq!(validation_types, vec![SurfaceTypeId::ValidationError]);
}

// -------------------------------------------------------------------------------------------------
// Drift guardrails for closed-set vocabulary (string literals).
// -------------------------------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(|p| p.parent()) else {
        panic!("INVARIANT: repo root missing — CARGO_MANIFEST_DIR chain must resolve during tests");
    };
    root.to_path_buf()
}

fn collect_rs_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    files
}

fn find_string_literals(paths: &[PathBuf], literals: &[&str]) -> Vec<String> {
    fn is_comment_line(line: &str) -> bool {
        let trimmed = line.trim_start();
        trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!")
    }

    let mut hits: Vec<String> = Vec::new();
    for path in paths {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if is_comment_line(line) {
                continue;
            }
            for &literal in literals {
                let needle = format!("\"{literal}\"");
                if line.contains(&needle) {
                    hits.push(format!("{}:{}: {}", path.display(), idx + 1, line.trim()));
                }
            }
        }
    }
    hits
}

fn compiler_layer_rs_files() -> Vec<PathBuf> {
    let root = repo_root();
    let targets = [
        root.join("src/frontend"),
        root.join("src/backend"),
        root.join("src/lsp"),
    ];
    let mut files = Vec::new();
    for dir in &targets {
        files.extend(collect_rs_files(dir));
    }
    files
}

#[test]
fn no_builtin_trait_string_literals_in_compiler_layers() {
    let files = compiler_layer_rs_files();

    let trait_literals = [
        "Debug",
        "Display",
        "Eq",
        "PartialEq",
        "Ord",
        "PartialOrd",
        "Hash",
        "Clone",
        "Default",
        "From",
        "Into",
        "TryFrom",
        "TryInto",
        "Iterator",
        "IntoIterator",
        "Error",
        "Iterable",
        "Sum",
    ];

    let hits = find_string_literals(&files, &trait_literals);
    assert!(
        hits.is_empty(),
        "builtin trait spellings must come from incan_core::lang::traits; found:\n{}",
        hits.join("\n")
    );
}

#[test]
fn no_constructor_string_literals_in_compiler_layers() {
    let files = compiler_layer_rs_files();

    let constructor_literals = ["Ok", "Err", "Some", "None"];
    let hits = find_string_literals(&files, &constructor_literals);
    assert!(
        hits.is_empty(),
        "constructor spellings must come from incan_core::lang::surface::constructors; found:\n{}",
        hits.join("\n")
    );
}

#[test]
fn no_frozen_collection_string_literals_in_compiler_layers() {
    let files = compiler_layer_rs_files();

    let frozen_literals = ["FrozenList", "FrozenSet", "FrozenDict"];
    let hits = find_string_literals(&files, &frozen_literals);
    assert!(
        hits.is_empty(),
        "frozen collection spellings must come from incan_core::lang::types::collections; found:\n{}",
        hits.join("\n")
    );
}
