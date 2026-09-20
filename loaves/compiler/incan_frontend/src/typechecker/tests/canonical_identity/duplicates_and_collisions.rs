//! Duplicate and ambiguous bindings: same-spelled module declarations, rejected nominal duplicates, local declarations
//! over imports, ambiguous same-spelled imports, module aliases against builtins, repeated imports, and enum variant
//! aliases.

use super::*;

/// Two same-spelled module declarations are diagnosed by the shared registration mechanism while both declaration
/// sites retain distinct identities for the diagnostic and later inspection.
#[test]
fn duplicate_module_declarations_keep_distinct_identities() -> Result<(), String> {
    let source = r#"
model User:
  name: str

model User:
  age: int
"#;
    let program = parse(source, "duplicate declarations")?;
    let second_span = program.declarations[1].span;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("duplicate module declarations were accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicate = errors
        .iter()
        .find(|error| error.message == "Duplicate definition of 'User'")
        .ok_or_else(|| format!("missing duplicate-binding diagnostic: {errors:?}"))?;
    assert_eq!(
        duplicate.span, second_span,
        "the duplicate declaration is the primary span"
    );
    assert_eq!(
        duplicate
            .notes
            .iter()
            .filter(|note| note.contains("canonical identity"))
            .count(),
        2,
        "the diagnostic must name both canonical identities: {duplicate:?}"
    );
    let user_identities: Vec<&CanonicalSymbolId> = checker
        .type_info()
        .declarations
        .declaration_identities
        .values()
        .filter(|identity| identity.declaration_name == "User")
        .collect();
    assert_eq!(
        user_identities.len(),
        2,
        "both declaration sites must keep their own exported identity"
    );
    assert_ne!(
        user_identities[0], user_identities[1],
        "two declaration sites are two identities, never one merged winner"
    );
    let active = checker
        .symbols
        .lookup("User")
        .ok_or("the first User binding must remain active")?;
    let active_identity = checker
        .symbols
        .identity_of(active)
        .ok_or("the active User binding must retain its identity")?;
    assert_eq!(
        active_identity.declaration_span,
        incan_semantics_core::HirSourceSpan::new(program.declarations[0].span.start, program.declarations[0].span.end),
        "a rejected duplicate cannot change the active lookup identity"
    );
    Ok(())
}

/// Rejected nominal declarations retain evidence but cannot mutate metadata owned by the active first declaration.
#[test]
fn rejected_nominal_duplicates_cannot_mutate_the_first_binding() -> Result<(), String> {
    let cases = [
        (
            r#"
newtype Token = int:
  def first(self) -> int:
    return 1

newtype Token = str:
  def second(self) -> int:
    return 2
"#,
            "Token",
            "first",
            "second",
            "newtype",
        ),
        (
            r#"
enum State:
  First

  def first(self) -> int:
    return 1

enum State:
  SecondOnly

  def second(self) -> int:
    return 2
"#,
            "State",
            "first",
            "second",
            "enum",
        ),
        (
            r#"
trait Root:
  def root(self) -> int: ...

trait Other:
  def other(self) -> int: ...

trait Contract with Root:
  def first(self) -> int: ...

trait Contract with Other:
  def second(self) -> int: ...
"#,
            "Contract",
            "first",
            "second",
            "trait",
        ),
    ];

    for (source, name, first_method, rejected_method, kind) in cases {
        let program = parse(source, &format!("duplicate {kind}"))?;
        let mut checker = TypeChecker::new();
        let errors = match checker.check_program(&program) {
            Ok(()) => return Err(format!("duplicate {kind} declaration was accepted")),
            Err(errors) => errors,
        };
        assert!(
            errors
                .iter()
                .any(|error| error.message == format!("Duplicate definition of '{name}'")),
            "missing duplicate {kind} diagnostic: {errors:?}"
        );
        let active = checker
            .symbols
            .lookup(name)
            .ok_or_else(|| format!("active {kind} missing"))?;
        let (methods, supertraits) = match checker.symbols.get(active).map(|symbol| &symbol.kind) {
            Some(SymbolKind::Type(TypeInfo::Newtype(info))) => (&info.methods, None),
            Some(SymbolKind::Type(TypeInfo::Enum(info))) => (&info.methods, None),
            Some(SymbolKind::Trait(info)) => (&info.methods, Some(&info.supertraits)),
            other => return Err(format!("unexpected active {kind} symbol: {other:?}")),
        };
        assert!(
            methods.contains_key(first_method),
            "first {kind} API must remain active"
        );
        assert!(
            !methods.contains_key(rejected_method),
            "rejected {kind} API mutated the first binding"
        );
        if kind == "enum" {
            assert!(
                !checker.symbols.all_symbols().iter().any(|symbol| {
                    symbol.name == "SecondOnly"
                        && matches!(&symbol.kind, SymbolKind::Variant(info) if info.enum_name == "State")
                }),
                "variants from a rejected enum must not enter the symbol arena"
            );
        }
        if let Some(supertraits) = supertraits {
            assert_eq!(
                supertraits.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
                vec!["Root"],
                "the rejected trait's supertraits must not overwrite the first trait"
            );
        }
    }
    Ok(())
}

/// A local declaration over an imported binding is a duplicate active binding, not implicit shadowing.
#[test]
fn local_declaration_over_import_is_diagnosed() -> Result<(), String> {
    let provider = parse(
        "pub def helper(value: str) -> str:\n  return value\n",
        "collision provider",
    )?;
    let consumer_source = r#"
from lib import helper

def helper(value: int) -> int:
  return value

def read() -> None:
  observed = helper
"#;
    let consumer = parse(consumer_source, "collision consumer")?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    let errors = match checker.check_with_imports(&consumer, &[("lib", &provider)]) {
        Ok(()) => return Err("a declaration implicitly replaced an imported binding".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Duplicate definition of 'helper'"),
        "expected a shared binding collision, got: {errors:?}"
    );
    let imported = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("helper")
        .ok_or("the imported helper identity must be retained")?;
    let observed = identity_at(
        &checker,
        nth_span(consumer_source, "helper", 2)?,
        "post-collision reference",
    )?;
    assert_eq!(
        &observed, imported,
        "the first import remains active after the local declaration is rejected"
    );
    let callable = checker
        .type_info()
        .declarations
        .function_bindings
        .get("helper")
        .ok_or("the active import must retain its callable metadata")?;
    assert_eq!(
        callable.params.first().map(|param| &param.ty),
        Some(&crate::symbols::ResolvedType::Str)
    );
    assert_eq!(callable.return_type, crate::symbols::ResolvedType::Str);
    Ok(())
}

/// Two unaliased imports with the same local spelling and different declaration identities are ambiguous.
#[test]
fn same_spelled_imports_from_different_modules_are_ambiguous() -> Result<(), String> {
    let left = parse("pub model Item:\n  left: int\n", "left provider")?;
    let right = parse("pub model Item:\n  right: str\n", "right provider")?;
    let consumer = parse(
        "from left import Item\nfrom right import Item\n",
        "ambiguous import consumer",
    )?;
    let first = consumer.declarations[0].span;
    let second = consumer.declarations[1].span;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["consumer".to_string()]));
    let errors = match checker.check_with_imports(&consumer, &[("left", &left), ("right", &right)]) {
        Ok(()) => return Err("different declarations shared one imported spelling".to_string()),
        Err(errors) => errors,
    };
    let ambiguous = errors
        .iter()
        .find(|error| error.message == "Ambiguous import binding 'Item'")
        .ok_or_else(|| format!("expected the shared ambiguity diagnostic, got: {errors:?}"))?;
    assert_eq!(ambiguous.span, second);
    assert_eq!(
        ambiguous.related_spans().first().map(|related| related.span),
        Some(first)
    );
    let canonical_notes = ambiguous
        .notes
        .iter()
        .filter(|note| note.contains("canonical identity"))
        .collect::<Vec<_>>();
    assert_eq!(canonical_notes.len(), 2);
    assert_ne!(canonical_notes[0], canonical_notes[1]);
    assert_eq!(
        ambiguous.hints,
        vec!["Use an explicit import alias so each declaration has a distinct local spelling"]
    );
    let target = checker
        .source_import_targets
        .get("Item")
        .ok_or("the active import must retain its source target")?;
    assert_eq!(target.module_path, vec!["left".to_string()]);
    let active_identity = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("Item")
        .ok_or("the active import must retain its resolved identity")?;
    assert_eq!(active_identity.origin, SymbolOrigin::Module(vec!["left".to_string()]));
    Ok(())
}

/// A target-specific source identity cannot leak into a later stdlib import through their shared local spelling.
#[test]
fn source_and_stdlib_import_collision_is_order_independent() -> Result<(), String> {
    let provider = parse("pub def value() -> int:\n  return 1\n", "source import provider")?;
    for source in [
        "from lib import value as shared\nfrom std.collections import OrdinalKey as shared\n",
        "from std.collections import OrdinalKey as shared\nfrom lib import value as shared\n",
    ] {
        let consumer = parse(source, "source and stdlib import collision")?;
        let mut checker = TypeChecker::new();
        let errors = match checker.check_with_imports(&consumer, &[("lib", &provider)]) {
            Ok(()) => return Err("source/stdlib collision was accepted".to_string()),
            Err(errors) => errors,
        };
        let ambiguous = errors
            .iter()
            .find(|error| error.message == "Ambiguous import binding 'shared'")
            .ok_or_else(|| format!("collision classification depended on import order: {errors:?}"))?;
        assert_eq!(
            ambiguous
                .notes
                .iter()
                .filter(|note| note.contains("canonical identity"))
                .count(),
            2,
            "both checked import targets must be proven in either order: {ambiguous:?}"
        );
    }
    Ok(())
}

/// Source aliases preserve target identity without being mislabeled as imports in collision diagnostics.
#[test]
fn local_alias_collisions_are_duplicates_not_ambiguous_imports() -> Result<(), String> {
    let source = r#"
def left() -> int:
  return 1

def right() -> int:
  return 2

same = alias left
same = alias right
"#;
    let program = parse(source, "local alias collision")?;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("two local aliases shared one name".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Duplicate definition of 'same'"),
        "expected a local duplicate diagnostic: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .all(|error| error.message != "Ambiguous import binding 'same'"),
        "a local alias is not an import: {errors:?}"
    );
    Ok(())
}

/// A module import cannot skip the shared registry merely because another concrete binding arrived first.
#[test]
fn module_import_after_concrete_bindings_reports_the_collision() -> Result<(), String> {
    let left = parse("pub model Item:\n  value: int\n", "left provider")?;
    let right = parse("pub def value() -> int:\n  return 1\n", "right provider")?;
    let local_first = parse(
        "def shared() -> int:\n  return 1\n\nimport right as shared\n",
        "local before module import",
    )?;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&local_first, &[("right", &right)]) {
        Ok(()) => return Err("a module import silently skipped a preceding local binding".to_string()),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.message == "Duplicate definition of 'shared'"),
        "expected local/module collision, got: {errors:?}"
    );

    let item_first = parse(
        "from left import Item\nimport right as Item\n",
        "item before module import",
    )?;
    let first = item_first.declarations[0].span;
    let second = item_first.declarations[1].span;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&item_first, &[("left", &left), ("right", &right)]) {
        Ok(()) => return Err("a module import silently skipped a preceding item import".to_string()),
        Err(errors) => errors,
    };
    let ambiguous = errors
        .iter()
        .find(|error| error.message == "Ambiguous import binding 'Item'")
        .ok_or_else(|| format!("expected item/module ambiguity, got: {errors:?}"))?;
    assert_eq!(ambiguous.span, second);
    assert_eq!(
        ambiguous.related_spans().first().map(|related| related.span),
        Some(first)
    );
    assert_eq!(
        ambiguous
            .notes
            .iter()
            .filter(|note| note.contains("canonical identity"))
            .count(),
        2,
        "the item and module imports must both report their proven identities"
    );
    assert_eq!(
        ambiguous.hints,
        vec!["Use an explicit import alias so each declaration has a distinct local spelling"]
    );
    Ok(())
}

/// Module aliases obey the same builtin tiers as every other source binding.
#[test]
fn module_aliases_cannot_replace_output_builtins_but_can_shadow_ordinary_builtins() -> Result<(), String> {
    let provider = parse("pub def value() -> int:\n  return 1\n", "module alias provider")?;
    for name in ["print", "println"] {
        let consumer = parse(&format!("import provider as {name}\n"), "immutable module alias")?;
        let mut checker = TypeChecker::new();
        let errors = match checker.check_with_imports(&consumer, &[("provider", &provider)]) {
            Ok(()) => return Err(format!("module alias replaced immutable {name}")),
            Err(errors) => errors,
        };
        assert!(
            errors
                .iter()
                .any(|error| error.message == format!("Cannot redefine immutable built-in function '{name}'")),
            "expected immutable-builtin diagnostic for module alias {name}, got: {errors:?}"
        );
    }

    let consumer = parse("import provider as len\n", "ordinary builtin module alias")?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("provider", &provider)])
        .map_err(|errors| format!("ordinary builtin alias should typecheck: {errors:?}"))?;
    let binding = checker.symbols.lookup("len").ok_or("the module alias must be active")?;
    assert!(
        matches!(
            checker.symbols.get(binding).map(|symbol| &symbol.kind),
            Some(crate::symbols::SymbolKind::Module(_))
        ),
        "the module alias must replace the ordinary builtin fallback"
    );
    Ok(())
}

/// Repeating one proven import is a duplicate, not an ambiguity, and the diagnostic retains both exact sites and the
/// one shared target identity.
#[test]
fn repeated_proven_import_is_a_duplicate_with_complete_evidence() -> Result<(), String> {
    let provider = parse("pub model Item:\n  value: int\n", "repeat provider")?;
    let consumer = parse("from lib import Item\nfrom lib import Item\n", "repeat consumer")?;
    let first = consumer.declarations[0].span;
    let second = consumer.declarations[1].span;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&consumer, &[("lib", &provider)]) {
        Ok(()) => return Err("a repeated proven import was accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicate = errors
        .iter()
        .find(|error| error.message == "Duplicate definition of 'Item'")
        .ok_or_else(|| format!("missing duplicate-import diagnostic: {errors:?}"))?;
    assert_eq!(duplicate.span, second);
    assert_eq!(
        duplicate.related_spans().first().map(|related| related.span),
        Some(first)
    );
    let identity = checker
        .type_info()
        .declarations
        .resolved_import_identities
        .get("Item")
        .ok_or("the proven import identity must be retained")?
        .render_compact();
    assert_eq!(
        duplicate.notes,
        vec![
            format!("First canonical identity: {identity}"),
            format!("Second canonical identity: {identity}"),
        ]
    );
    Ok(())
}

/// Enum alias collection and semantic validation consume one shared collision answer, so a duplicate alias produces
/// one diagnostic rather than one from each phase.
#[test]
fn duplicate_enum_variant_alias_is_reported_once() -> Result<(), String> {
    let source = r#"
enum Level:
  Info
  Warn
  Warning = alias Warn
  Warning = alias Info
"#;
    let program = parse(source, "duplicate enum variant alias")?;
    let Declaration::Enum(en) = &program.declarations[0].node else {
        return Err("expected enum declaration".to_string());
    };
    let first_alias_span = en.variant_aliases[0].span;
    let rejected_alias_span = en.variant_aliases[1].span;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_program(&program) {
        Ok(()) => return Err("duplicate enum variant alias was accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicates = errors
        .iter()
        .filter(|error| error.message == "Duplicate definition of 'Warning'")
        .collect::<Vec<_>>();
    assert_eq!(
        duplicates.len(),
        1,
        "the shared registry must emit exactly one alias collision: {errors:?}"
    );
    assert_eq!(duplicates[0].related_spans().len(), 1);
    let warn = checker.symbols.lookup("Warn").ok_or("Warn binding missing")?;
    let info = checker.symbols.lookup("Info").ok_or("Info binding missing")?;
    let warning = checker.symbols.lookup("Warning").ok_or("Warning alias missing")?;
    assert_eq!(checker.symbols.identity_of(warning), checker.symbols.identity_of(warn));
    assert_ne!(checker.symbols.identity_of(warning), checker.symbols.identity_of(info));
    let level = checker.symbols.lookup("Level").ok_or("Level binding missing")?;
    let Some(SymbolKind::Type(TypeInfo::Enum(level))) = checker.symbols.get(level).map(|symbol| &symbol.kind) else {
        return Err("Level should retain enum metadata".to_string());
    };
    assert_eq!(level.variant_aliases.get("Warning").map(String::as_str), Some("Warn"));
    let exported = &checker.type_info().declarations.member_declaration_identities;
    assert!(!exported.contains_key(&(first_alias_span.start, first_alias_span.end)));
    assert!(!exported.contains_key(&(rejected_alias_span.start, rejected_alias_span.end)));
    Ok(())
}

/// A valid enum alias is a second binding to the target variant, not a second variant declaration identity.
#[test]
fn enum_variant_alias_preserves_the_target_identity() -> Result<(), String> {
    let checker = check(
        r#"
enum Level:
  Warn
  Warning = alias Warn
"#,
        "enum variant alias identity",
    )?;
    let warn = checker.symbols.lookup("Warn").ok_or("Warn binding missing")?;
    let warning = checker.symbols.lookup("Warning").ok_or("Warning alias missing")?;
    assert_eq!(
        checker.symbols.identity_of(warn),
        checker.symbols.identity_of(warning),
        "an enum alias must carry its target variant's canonical identity"
    );
    Ok(())
}

/// Repeating one checked module import is a duplicate of the same path-namespace declaration, not an ambiguity.
#[test]
fn repeated_module_import_has_one_deterministic_identity() -> Result<(), String> {
    let provider = parse("pub def value() -> int:\n  return 1\n", "module provider")?;
    let consumer = parse(
        "import provider as shared\nimport provider as shared\n",
        "module consumer",
    )?;
    let first = consumer.declarations[0].span;
    let second = consumer.declarations[1].span;
    let mut checker = TypeChecker::new();
    let errors = match checker.check_with_imports(&consumer, &[("provider", &provider)]) {
        Ok(()) => return Err("a repeated module import was accepted".to_string()),
        Err(errors) => errors,
    };
    let duplicate = errors
        .iter()
        .find(|error| error.message == "Duplicate definition of 'shared'")
        .ok_or_else(|| format!("expected repeated-module duplicate, got: {errors:?}"))?;
    assert_eq!(duplicate.span, second);
    assert_eq!(
        duplicate.related_spans().first().map(|related| related.span),
        Some(first)
    );
    let active = checker
        .symbols
        .lookup("shared")
        .ok_or("shared module binding missing")?;
    let identity = checker
        .symbols
        .identity_of(active)
        .ok_or("checked module binding must carry its path identity")?;
    let expected = crate::symbols::SymbolTable::module_path_identity(&["provider".to_string()])
        .ok_or("provider path must produce a module identity")?;
    assert_eq!(identity, &expected);
    assert_eq!(
        duplicate.notes,
        vec![
            format!("First canonical identity: {}", identity.render_compact()),
            format!("Second canonical identity: {}", identity.render_compact()),
        ],
        "the duplicate diagnostic must prove both bindings target the same module"
    );
    Ok(())
}
