use std::collections::HashMap;

use incan_core::lang::builtins;
use incan_core::lang::errors;
use incan_core::lang::keywords;
use incan_core::lang::operators;
use incan_core::lang::punctuation;
use incan_core::lang::types::{collections, numerics, stringlike};
use std::path::{Path, PathBuf};

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
    let mut seen: HashMap<&'static str, builtins::BuiltinFnId> = HashMap::new();

    for info in builtins::BUILTIN_FUNCTIONS {
        assert_eq!(
            builtins::from_str(info.canonical),
            Some(info.id),
            "builtin canonical spelling not resolvable: {}",
            info.canonical
        );
        assert_eq!(
            builtins::as_str(info.id),
            info.canonical,
            "builtin as_str mismatch for {:?}",
            info.id
        );

        if let Some(prev) = seen.insert(info.canonical, info.id) {
            panic!(
                "duplicate builtin spelling {:?}: {:?} and {:?}",
                info.canonical, prev, info.id
            );
        }

        for &alias in info.aliases {
            assert_eq!(
                builtins::from_str(alias),
                Some(info.id),
                "builtin alias not resolvable: {}",
                alias
            );
            if let Some(prev) = seen.insert(alias, info.id) {
                panic!(
                    "duplicate builtin alias spelling {:?}: {:?} and {:?}",
                    alias, prev, info.id
                );
            }
        }
    }
}

#[test]
fn exceptions_spellings_unique_and_resolvable() {
    let mut seen: HashMap<&'static str, incan_core::errors::ErrorKind> = HashMap::new();

    for info in errors::EXCEPTIONS {
        assert_eq!(
            errors::from_str(info.canonical),
            Some(info.id),
            "exception canonical spelling not resolvable: {}",
            info.canonical
        );
        assert_eq!(
            errors::as_str(info.id),
            info.canonical,
            "exception as_str mismatch for {:?}",
            info.id
        );

        if let Some(prev) = seen.insert(info.canonical, info.id) {
            panic!(
                "duplicate exception spelling {:?}: {:?} and {:?}",
                info.canonical, prev, info.id
            );
        }

        for &alias in info.aliases {
            assert_eq!(
                errors::from_str(alias),
                Some(info.id),
                "exception alias not resolvable: {}",
                alias
            );
            if let Some(prev) = seen.insert(alias, info.id) {
                panic!(
                    "duplicate exception alias spelling {:?}: {:?} and {:?}",
                    alias, prev, info.id
                );
            }
        }
    }
}

#[test]
fn operators_spellings_unique_and_resolvable() {
    let mut seen: HashMap<&'static str, operators::OperatorId> = HashMap::new();

    for info in operators::OPERATORS {
        for &sp in info.spellings {
            assert_eq!(
                operators::from_str(sp),
                Some(info.id),
                "operator spelling not resolvable: {}",
                sp
            );
            if let Some(prev) = seen.insert(sp, info.id) {
                panic!("duplicate operator spelling {:?}: {:?} and {:?}", sp, prev, info.id);
            }
        }
    }
}

#[test]
fn punctuation_spellings_unique_and_resolvable() {
    let mut seen: HashMap<&'static str, punctuation::PunctuationId> = HashMap::new();

    for info in punctuation::PUNCTUATION {
        assert_eq!(
            punctuation::from_str(info.canonical),
            Some(info.id),
            "punctuation canonical spelling not resolvable: {}",
            info.canonical
        );
        assert_eq!(
            punctuation::as_str(info.id),
            info.canonical,
            "punctuation as_str mismatch for {:?}",
            info.id
        );

        if let Some(prev) = seen.insert(info.canonical, info.id) {
            panic!(
                "duplicate punctuation spelling {:?}: {:?} and {:?}",
                info.canonical, prev, info.id
            );
        }

        for &alias in info.aliases {
            assert_eq!(
                punctuation::from_str(alias),
                Some(info.id),
                "punctuation alias not resolvable: {}",
                alias
            );
            if let Some(prev) = seen.insert(alias, info.id) {
                panic!(
                    "duplicate punctuation alias spelling {:?}: {:?} and {:?}",
                    alias, prev, info.id
                );
            }
        }
    }
}

#[test]
fn types_spellings_unique_and_resolvable() {
    // stringlike
    {
        let mut seen: HashMap<&'static str, stringlike::StringLikeId> = HashMap::new();
        for info in stringlike::STRING_LIKE_TYPES {
            if let Some(prev) = seen.insert(info.canonical, info.id) {
                panic!(
                    "duplicate stringlike canonical {:?}: {:?} and {:?}",
                    info.canonical, prev, info.id
                );
            }
            for &alias in info.aliases {
                if let Some(prev) = seen.insert(alias, info.id) {
                    panic!("duplicate stringlike alias {:?}: {:?} and {:?}", alias, prev, info.id);
                }
            }
            assert_eq!(
                stringlike::as_str(info.id),
                info.canonical,
                "stringlike as_str mismatch for {:?}",
                info.id
            );
        }
    }

    // numerics
    {
        let mut seen: HashMap<&'static str, numerics::NumericTypeId> = HashMap::new();
        for info in numerics::NUMERIC_TYPES {
            if let Some(prev) = seen.insert(info.canonical, info.id) {
                panic!(
                    "duplicate numeric canonical {:?}: {:?} and {:?}",
                    info.canonical, prev, info.id
                );
            }
            for &alias in info.aliases {
                if let Some(prev) = seen.insert(alias, info.id) {
                    panic!("duplicate numeric alias {:?}: {:?} and {:?}", alias, prev, info.id);
                }
            }
            assert_eq!(
                numerics::as_str(info.id),
                info.canonical,
                "numeric as_str mismatch for {:?}",
                info.id
            );
        }
    }

    // collections
    {
        let mut seen: HashMap<&'static str, collections::CollectionTypeId> = HashMap::new();
        for info in collections::COLLECTION_TYPES {
            if let Some(prev) = seen.insert(info.canonical, info.id) {
                panic!(
                    "duplicate collection canonical {:?}: {:?} and {:?}",
                    info.canonical, prev, info.id
                );
            }
            for &alias in info.aliases {
                if let Some(prev) = seen.insert(alias, info.id) {
                    panic!("duplicate collection alias {:?}: {:?} and {:?}", alias, prev, info.id);
                }
            }
            assert_eq!(
                collections::as_str(info.id),
                info.canonical,
                "collection as_str mismatch for {:?}",
                info.id
            );
        }
    }
}

// -------------------------------------------------------------------------------------------------
// Drift guardrails for closed-set vocabulary (string literals).
// -------------------------------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("INVARIANT: repo root missing — CARGO_MANIFEST_DIR chain must resolve during tests")
        .to_path_buf()
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
