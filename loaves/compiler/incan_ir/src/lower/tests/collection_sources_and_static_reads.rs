//! Reads that lowering hands the emitter already typed and shaped: a local bound straight from a module static
//! (#1777), a `const` `FrozenDict` lookup and membership test (#1757), a comprehension over a frozen collection of
//! text (#1757), and `set()` over a generator (#1744).

use super::*;
use crate::expr::BuiltinFn;
use crate::types::SetConstructorIteration;
use incan_lang::lang::types::collections::{self as collection_types, CollectionTypeId};

/// Return the body of the named function.
fn function_body<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a [IrStmt], String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function.body.as_slice()),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))
}

/// Return the value and the Rust-facing annotation of the named `let` in a function body.
fn let_binding<'a>(body: &'a [IrStmt], name: &str) -> Result<(&'a TypedExpr, Option<&'a IrType>), String> {
    body.iter()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::Let {
                name: bound,
                type_annotation,
                value,
                ..
            } if bound == name => Some((value, type_annotation.as_ref())),
            _ => None,
        })
        .ok_or_else(|| format!("missing `let {name}`"))
}

/// Return the expression the named function returns.
fn returned_value<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a TypedExpr, String> {
    match function_body(ir, name)?.last() {
        Some(IrStmt {
            kind: IrStmtKind::Return(Some(expr)),
            ..
        }) => Ok(expr),
        other => Err(format!("`{name}` must end in a return, got {other:?}")),
    }
}

/// #1777: a new local bound straight from a module static aliases the static's storage whether or not it spells its
/// type, so the checked annotation is not spelled on the Rust binding (the binding holds the storage handle, not a
/// value of the annotated type). The unannotated form was already an alias.
#[test]
fn annotated_local_bound_from_a_static_aliases_its_storage_issue1777() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
static COUNT: int = 3


def main() -> None:
    count: int = COUNT
    let fixed: int = COUNT
    mut live: int = COUNT
    plain = COUNT
    live += 1
    println(count + fixed + live + plain)
"#,
    )?;
    let body = function_body(&ir, "main")?;
    for name in ["count", "fixed", "live", "plain"] {
        let (value, annotation) = let_binding(body, name)?;
        assert!(
            matches!(value.kind, IrExprKind::StaticBinding { .. }),
            "`{name}` must alias the static's storage, got {value:?}"
        );
        assert_eq!(value.ty, IrType::Int, "`{name}` keeps the static's value type");
        assert_eq!(
            annotation, None,
            "`{name}` is a storage alias, so its Rust binding carries no value-type annotation"
        );
    }
    Ok(())
}

/// #1757: `table[key]` on a `const` `FrozenDict[K, V]` lowers to an index read typed `V`, and `contains_key` lowers to
/// the keyed membership test a dict takes.
#[test]
fn frozen_dict_index_carries_the_value_type_issue1757() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const TABLE: FrozenDict[str, FrozenList[str]] = {"names": ["alpha", "beta"], "empty": []}


def names() -> FrozenList[str]:
    return TABLE["names"]


def has_names() -> bool:
    return TABLE.contains_key("names")

"#,
    )?;
    let lookup = returned_value(&ir, "names")?;
    let IrExprKind::Index { object, .. } = &lookup.kind else {
        return Err(format!("`names` must lower to an index read, got {lookup:?}"));
    };
    let IrType::NamedGeneric(object_name, object_args) = &object.ty else {
        return Err(format!("the lookup reads the frozen dict, got {:?}", object.ty));
    };
    assert_eq!(
        collection_types::from_str(object_name),
        Some(CollectionTypeId::FrozenDict)
    );
    assert_eq!(
        Some(&lookup.ty),
        object_args.get(1),
        "the lookup is typed as the frozen dict's value type"
    );
    let IrType::NamedGeneric(value_name, _) = &lookup.ty else {
        return Err(format!("the value type is the frozen list, got {:?}", lookup.ty));
    };
    assert_eq!(
        collection_types::from_str(value_name),
        Some(CollectionTypeId::FrozenList)
    );

    let membership = returned_value(&ir, "has_names")?;
    assert!(
        matches!(
            membership.kind,
            IrExprKind::KnownMethodCall {
                kind: MethodKind::Collection(CollectionMethodKind::Contains),
                ..
            }
        ),
        "`contains_key` must lower to keyed membership, got {membership:?}"
    );
    Ok(())
}

/// #1757: a comprehension over a `const` `FrozenList[str]` iterates the source's `list(...)` conversion, which yields
/// the owned `str` items the comprehension binds; a frozen list of `int` is iterated as written.
#[test]
fn comprehension_over_frozen_text_iterates_owned_items_issue1757() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const NAMES: FrozenList[str] = ["alpha", "beta"]
const NUMS: FrozenList[int] = [1, 2]


def joined() -> str:
    return " ".join([name for name in NAMES])


def doubled() -> list[int]:
    return [n * 2 for n in NUMS]
"#,
    )?;
    let joined = returned_value(&ir, "joined")?;
    let IrExprKind::KnownMethodCall { args, .. } = &joined.kind else {
        return Err(format!("`joined` must lower to `str.join`, got {joined:?}"));
    };
    let comprehension = args
        .first()
        .map(|arg| &arg.expr)
        .ok_or_else(|| "`join` takes the comprehension".to_string())?;
    let IrExprKind::ListComp { iterable, .. } = &comprehension.kind else {
        return Err(format!("`join`'s argument is the comprehension, got {comprehension:?}"));
    };
    let IrExprKind::BuiltinCall {
        func: BuiltinFn::CollectionConstructor(CollectionTypeId::List),
        args: sources,
    } = &iterable.kind
    else {
        return Err(format!(
            "the frozen text source must be iterated through `list(...)`, got {iterable:?}"
        ));
    };
    assert_eq!(iterable.ty, IrType::List(Box::new(IrType::String)));
    let source_is_frozen_text = sources.first().is_some_and(|source| match &source.ty {
        IrType::NamedGeneric(name, items) => {
            collection_types::from_str(name) == Some(CollectionTypeId::FrozenList)
                && matches!(items.first(), Some(IrType::String | IrType::StaticStr))
        }
        _ => false,
    });
    assert!(source_is_frozen_text, "the conversion reads the const: {sources:?}");

    let doubled = returned_value(&ir, "doubled")?;
    let IrExprKind::ListComp { iterable, .. } = &doubled.kind else {
        return Err(format!("`doubled` must lower to a comprehension, got {doubled:?}"));
    };
    assert!(
        !matches!(iterable.kind, IrExprKind::BuiltinCall { .. }),
        "a frozen list of `int` is iterated as written, got {iterable:?}"
    );
    Ok(())
}

/// #1744: `set(generator)` lowers to the `Set` constructor over the generator, whose iteration plan collects it
/// through the `Iterator` trait rather than the runtime wrapper's own `collect`.
#[test]
fn set_constructor_over_a_generator_plans_the_iterator_trait_issue1744() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
def numbers(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value


def unique() -> set[int]:
    return set(numbers(3))
"#,
    )?;
    let constructed = returned_value(&ir, "unique")?;
    let IrExprKind::BuiltinCall {
        func: BuiltinFn::CollectionConstructor(CollectionTypeId::Set),
        args,
    } = &constructed.kind
    else {
        return Err(format!(
            "`unique` must lower to the set constructor, got {constructed:?}"
        ));
    };
    let source = args.first().ok_or_else(|| "`set` takes the generator".to_string())?;
    assert_eq!(
        source.ty.set_constructor_source(),
        Some((&IrType::Int, SetConstructorIteration::CollectOwnedIterator)),
        "the generator source is collected through the `Iterator` trait"
    );
    Ok(())
}
