//! The module path and item an import-shaped declaration hands the emitter: a relative import names the module the
//! checker resolved by its crate-absolute path (#1766), and an alias of a module member lowers to an import of that
//! member (#1764).

use super::*;
use crate::decl::{IrImportItem, IrImportQualifier, Visibility};

/// Parse one module, keeping lexer and parser failures as test errors.
pub(super) fn parse_module(source: &str, context: &str) -> Result<ast::Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{context} lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("{context} parse failed: {errors:?}"))
}

/// Check `module` at its logical path against its dependencies and lower it the way a multi-module build does.
pub(super) fn lower_module_at(
    module_path: &[&str],
    module: &ast::Program,
    dependencies: &[(&str, &ast::Program)],
) -> Result<IrProgram, String> {
    let module_path = module_path
        .iter()
        .map(|segment| segment.to_string())
        .collect::<Vec<_>>();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_with_imports(module, dependencies)
        .map_err(|errors| format!("{} should typecheck: {errors:?}", module_path.join(".")))?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    lowering.set_current_source_module_name(Some(module_path.join(".")));
    lowering
        .lower_program(module)
        .map_err(|errors| format!("{} lowering failed: {errors:?}", module_path.join(".")))
}

/// Return every import declaration a program lowers, in order, as its qualifier, path, visibility and items.
pub(super) fn lowered_imports(ir: &IrProgram) -> Vec<(IrImportQualifier, Vec<String>, Visibility, Vec<IrImportItem>)> {
    ir.declarations
        .iter()
        .filter_map(|decl| match &decl.kind {
            IrDeclKind::Import {
                qualifier,
                path,
                visibility,
                items,
                ..
            } => Some((*qualifier, path.clone(), *visibility, items.clone())),
            _ => None,
        })
        .collect()
}

/// #1766: `..` climbs from the importing file's directory, so `from ..db.schema import Database` in
/// `store/relative.incn` names the root's `db.schema`. Rust's `super` climbs from the importing module, one level less,
/// so the import reaches the emitter as the crate-absolute path of the module the checker bound.
#[test]
fn relative_import_lowers_to_the_crate_absolute_module_the_checker_bound_issue1766() -> Result<(), String> {
    let schema = parse_module("pub model Database:\n    pub id: int\n", "db.schema")?;
    let relative = parse_module(
        "from ..db.schema import Database\n\n\npub def touch(db: Database) -> str:\n    return f\"relative:{db.id}\"\n",
        "store.relative",
    )?;
    let ir = lower_module_at(&["store", "relative"], &relative, &[("db_schema", &schema)])?;
    let imports = lowered_imports(&ir);
    let (qualifier, path, _, items) = imports
        .iter()
        .find(|(_, _, _, items)| items.iter().any(|item| item.name == "Database"))
        .ok_or_else(|| format!("no import binds `Database`: {imports:?}"))?;
    assert_eq!(*qualifier, IrImportQualifier::Crate, "{imports:?}");
    assert_eq!(path, &vec!["db".to_string(), "schema".to_string()], "{imports:?}");
    assert_eq!(items.len(), 1, "{items:?}");
    Ok(())
}

/// #1766: the `super` spelling of the same climb lands on the same module.
#[test]
fn rust_style_parent_import_lowers_to_the_same_module_issue1766() -> Result<(), String> {
    let schema = parse_module("pub model Database:\n    pub id: int\n", "db.schema")?;
    let relative = parse_module(
        "from super::db::schema import Database\n\n\npub def touch(db: Database) -> int:\n    return db.id\n",
        "store.relative",
    )?;
    let ir = lower_module_at(&["store", "relative"], &relative, &[("db_schema", &schema)])?;
    let imports = lowered_imports(&ir);
    assert!(
        imports
            .iter()
            .any(|(qualifier, path, _, _)| *qualifier == IrImportQualifier::Crate
                && path == &vec!["db".to_string(), "schema".to_string()]),
        "{imports:?}"
    );
    Ok(())
}

/// #1766: `...` climbs two directories from the importing file's own, so `from ...db.schema import Database` in
/// `app/store/relative.incn` names the root's `db.schema`.
#[test]
fn grandparent_relative_import_lowers_to_the_root_module_issue1766() -> Result<(), String> {
    let schema = parse_module("pub model Database:\n    pub id: int\n", "db.schema")?;
    let relative = parse_module(
        "from ...db.schema import Database\n\n\npub def touch(db: Database) -> int:\n    return db.id\n",
        "app.store.relative",
    )?;
    let ir = lower_module_at(&["app", "store", "relative"], &relative, &[("db_schema", &schema)])?;
    let imports = lowered_imports(&ir);
    assert!(
        imports
            .iter()
            .any(|(qualifier, path, _, _)| *qualifier == IrImportQualifier::Crate
                && path == &vec!["db".to_string(), "schema".to_string()]),
        "{imports:?}"
    );
    Ok(())
}

/// Return the import item that binds `local_name`, with the import's path and visibility.
pub(super) fn import_binding<'a>(
    imports: &'a [(IrImportQualifier, Vec<String>, Visibility, Vec<IrImportItem>)],
    local_name: &str,
) -> Result<(&'a Vec<String>, &'a Visibility, &'a IrImportItem), String> {
    imports
        .iter()
        .find_map(|(_, path, visibility, items)| {
            items
                .iter()
                .find(|item| item.source_binding_name() == local_name)
                .map(|item| (path, visibility, item))
        })
        .ok_or_else(|| format!("no import binds `{local_name}`: {imports:?}"))
}

/// #1764: an alias of a module member (`root = math.sqrt`) lowers to the import `from std.math import sqrt as root`
/// with the alias's visibility and the target's identity, so the module binds the projection every call to `root`
/// names. A public and a private alias lower alike.
#[test]
fn module_member_alias_lowers_to_an_import_of_the_member_issue1764() -> Result<(), String> {
    for (visibility_keyword, visibility) in [("pub ", Visibility::Public), ("", Visibility::Private)] {
        let (_, ir, _) = lower_source_with_lowering(&format!(
            "import std.math as math\n\n{visibility_keyword}root = math.sqrt\n\n\ndef main() -> None:\n    println(int(root(16.0)))\n"
        ))?;
        assert!(
            !ir.declarations.iter().any(|decl| matches!(
                &decl.kind,
                IrDeclKind::SymbolAlias { name, .. } if name == "root"
            )),
            "`{visibility_keyword}root` must not stay a symbol alias"
        );
        let imports = lowered_imports(&ir);
        let (path, lowered_visibility, item) = import_binding(&imports, "root")?;
        assert_eq!(path, &vec!["std".to_string(), "math".to_string()]);
        assert_eq!(lowered_visibility, &visibility);
        assert_eq!(item.name, "sqrt");
        assert_eq!(item.alias.as_deref(), Some("root"));
        assert_eq!(
            item.canonical
                .as_ref()
                .map(|identity| identity.declaration_name.as_str()),
            Some("sqrt"),
            "the import carries the member's identity"
        );
    }
    Ok(())
}

/// #1764: an alias of a source module's member names the member through the module binding's own import path.
#[test]
fn source_module_member_alias_lowers_to_an_import_through_the_module_binding_issue1764() -> Result<(), String> {
    let helpers = parse_module("pub def double(value: int) -> int:\n    return value * 2\n", "helpers")?;
    let facade = parse_module("import helpers as h\n\npub twice = h.double\n", "facade")?;
    let ir = lower_module_at(&["facade"], &facade, &[("helpers", &helpers)])?;
    let imports = lowered_imports(&ir);
    let (path, visibility, item) = import_binding(&imports, "twice")?;
    assert_eq!(path, &vec!["helpers".to_string()]);
    assert_eq!(visibility, &Visibility::Public);
    assert_eq!(item.name, "double");
    assert_eq!(item.alias.as_deref(), Some("twice"));
    assert!(item.canonical.is_some(), "{item:?}");
    Ok(())
}

/// An alias of a declaration this module imports directly keeps the symbol-alias shape: its import binds the
/// projection already.
#[test]
fn imported_single_segment_alias_stays_a_symbol_alias() -> Result<(), String> {
    let (_, ir, _) = lower_source_with_lowering(
        "from std.math import sqrt\n\npub root = sqrt\n\n\ndef main() -> None:\n    println(int(root(16.0)))\n",
    )?;
    assert!(
        ir.declarations.iter().any(|decl| matches!(
            &decl.kind,
            IrDeclKind::SymbolAlias { name, .. } if name == "root"
        )),
        "{:?}",
        ir.declarations
    );
    Ok(())
}
