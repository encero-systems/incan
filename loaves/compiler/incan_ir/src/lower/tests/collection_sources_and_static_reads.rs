//! Reads that lowering hands the emitter already typed and shaped: a local bound straight from a module static
//! (#1777), the reads of a `const` frozen collection (#1757): lookups, indexing, `len()`, membership and iteration
//! sources, `set()` over a generator (#1744), and the nested iterator a `flat_map` callback hands its adapter.

use super::*;
use crate::expr::{BuiltinFn, IrGeneratorClause, IteratorMethodKind};
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
/// value of the annotated type). The unannotated form was already an alias. An annotation spelled through a
/// transparent type alias names the same type; one that names a wider type reads the value instead.
#[test]
fn annotated_local_bound_from_a_static_aliases_its_storage_issue1777() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
type Count = int
type Ints = list[int]

static COUNT: int = 3
static ITEMS: list[int] = []


def main() -> None:
    count: int = COUNT
    let fixed: int = COUNT
    mut live: int = COUNT
    plain = COUNT
    aliased: Count = COUNT
    live += 1
    println(count + fixed + live + plain + aliased)
    items: Ints = ITEMS
    items.append(1)
    maybe: Option[int] = COUNT
    println(maybe == Some(3))
"#,
    )?;
    let body = function_body(&ir, "main")?;
    for (name, static_ty) in [
        ("count", IrType::Int),
        ("fixed", IrType::Int),
        ("live", IrType::Int),
        ("plain", IrType::Int),
        ("aliased", IrType::Int),
        ("items", IrType::List(Box::new(IrType::Int))),
    ] {
        let (value, annotation) = let_binding(body, name)?;
        assert!(
            matches!(value.kind, IrExprKind::StaticBinding { .. }),
            "`{name}` must alias the static's storage, got {value:?}"
        );
        assert_eq!(value.ty, static_ty, "`{name}` keeps the static's value type");
        assert_eq!(
            annotation, None,
            "`{name}` is a storage alias, so its Rust binding carries no value-type annotation"
        );
    }
    let (maybe, annotation) = let_binding(body, "maybe")?;
    assert!(
        !matches!(maybe.kind, IrExprKind::StaticBinding { .. }),
        "a wider annotation reads the static's value, got {maybe:?}"
    );
    assert_eq!(annotation, Some(&IrType::Option(Box::new(IrType::Int))));
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

/// #1757: a lookup whose value type is `Copy` (`str` text, `int`) is handed to the emitter grouped, so a conversion
/// applied to the lookup applies to the whole read, and a `'static` text value is converted to the owned `str` the
/// checker typed; a lookup of a frozen collection is left bare.
#[test]
fn frozen_dict_copy_value_lookup_is_grouped_issue1757() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const TABLE: FrozenDict[str, FrozenList[str]] = {"names": ["alpha", "beta"]}
const LABELS: FrozenDict[str, str] = {"x": "ex"}
const SQUARES: FrozenDict[int, int] = {2: 4}


def names() -> FrozenList[str]:
    return TABLE["names"]


def label() -> str:
    return LABELS["x"]


def square() -> int:
    return SQUARES[2]
"#,
    )?;
    let label = returned_value(&ir, "label")?;
    let IrExprKind::InteropCoerce { expr: grouped, .. } = &label.kind else {
        return Err(format!("the `str` value is converted to an owned `str`, got {label:?}"));
    };
    assert_eq!(
        label.ty,
        IrType::String,
        "the read is the owned `str` the checker typed"
    );
    assert_eq!(
        grouped.ty,
        IrType::StaticStr,
        "the conversion reads the stored `'static` text"
    );
    for (name, lookup) in [("label", &**grouped), ("square", returned_value(&ir, "square")?)] {
        let IrExprKind::Block {
            stmts,
            value: Some(value),
        } = &lookup.kind
        else {
            return Err(format!(
                "`{name}` must hand the emitter a grouped lookup, got {lookup:?}"
            ));
        };
        assert!(stmts.is_empty(), "the group carries no statements: {stmts:?}");
        assert!(
            matches!(value.kind, IrExprKind::Index { .. }),
            "the group wraps the lookup itself for `{name}`, got {value:?}"
        );
        assert_eq!(lookup.ty, value.ty, "the group keeps the lookup's type for `{name}`");
    }
    let names = returned_value(&ir, "names")?;
    assert!(
        matches!(names.kind, IrExprKind::Index { .. }),
        "a frozen collection value is read as written, got {names:?}"
    );
    Ok(())
}

/// #1757: a comprehension over a `const` frozen collection iterates the source's `list(...)` conversion, which yields
/// the owned items the comprehension binds: `str` items for a `FrozenList[str]`, `int` items for a `FrozenList[int]`.
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
        matches!(
            iterable.kind,
            IrExprKind::BuiltinCall {
                func: BuiltinFn::CollectionConstructor(CollectionTypeId::List),
                ..
            }
        ),
        "a frozen list of `int` is iterated through `list(...)` as well, got {iterable:?}"
    );
    assert_eq!(iterable.ty, IrType::List(Box::new(IrType::Int)));
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

/// Return whether a type is the named frozen collection family.
fn is_frozen_family(ty: &IrType, family: CollectionTypeId) -> bool {
    matches!(ty, IrType::NamedGeneric(name, _) if collection_types::from_str(name) == Some(family))
}

/// #1757: `len()` on a `const` frozen collection or frozen bytes lowers to the `len(c)` builtin, whose result is the
/// `int` the checker typed rather than the runtime wrapper's `usize` length.
#[test]
fn frozen_len_reads_the_int_length_issue1757() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const NAMES: FrozenList[str] = ["alpha", "beta"]
const TAGS: FrozenSet[int] = {1, 2}
const TABLE: FrozenDict[str, int] = {"a": 1}
const BLOB: bytes = b"ab"


def names_len() -> int:
    return NAMES.len()


def tags_len() -> int:
    return TAGS.len()


def table_len() -> int:
    return TABLE.len()


def blob_len() -> int:
    return BLOB.len()
"#,
    )?;
    for name in ["names_len", "tags_len", "table_len", "blob_len"] {
        let length = returned_value(&ir, name)?;
        assert!(
            matches!(
                &length.kind,
                IrExprKind::BuiltinCall {
                    func: BuiltinFn::Len,
                    args,
                } if args.len() == 1
            ),
            "`{name}` must lower to `len(c)`, got {length:?}"
        );
        assert_eq!(length.ty, IrType::Int, "`{name}` is an `int`");
    }
    Ok(())
}

/// #1757: `items[i]` on a `const` `FrozenList[T]` is an index read typed as the element, and a `'static` text or bytes
/// element is converted to the owned `str` or `bytes` the checker typed.
#[test]
fn frozen_list_index_reads_the_owned_element_issue1757() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const NAMES: FrozenList[str] = ["alpha", "beta"]
const NUMS: FrozenList[int] = [1, 2]
const BLOBS: FrozenList[bytes] = [b"ab"]


def first_name() -> str:
    return NAMES[0]


def last_num() -> int:
    return NUMS[-1]


def first_blob() -> bytes:
    return BLOBS[0]
"#,
    )?;
    for (name, owned_ty, stored_ty) in [
        ("first_name", IrType::String, IrType::StaticStr),
        ("first_blob", IrType::Bytes, IrType::StaticBytes),
    ] {
        let read = returned_value(&ir, name)?;
        let IrExprKind::InteropCoerce { expr: element, .. } = &read.kind else {
            return Err(format!("`{name}` converts the stored element, got {read:?}"));
        };
        assert_eq!(read.ty, owned_ty, "`{name}` reads the owned element");
        assert!(
            matches!(element.kind, IrExprKind::Index { .. }),
            "`{name}` converts the index read itself, got {element:?}"
        );
        assert_eq!(element.ty, stored_ty, "`{name}` reads the stored `'static` element");
    }
    let num = returned_value(&ir, "last_num")?;
    assert!(
        matches!(num.kind, IrExprKind::Index { .. }),
        "an `int` element is read as it is stored, got {num:?}"
    );
    assert_eq!(num.ty, IrType::Int);
    Ok(())
}

/// #1757: `x in c` and `frozen_set.contains(x)` lower to the collection membership test. A `FrozenList` or `FrozenSet`
/// receiver is borrowed, so the test reads the frozen storage as it reads a borrowed list or set; a `FrozenDict` keeps
/// its receiver for the keyed lookup a dict takes.
#[test]
fn frozen_membership_reads_the_collection_test_issue1757() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const NAMES: FrozenList[str] = ["alpha", "beta"]
const TAGS: FrozenSet[str] = {"a"}
const TABLE: FrozenDict[str, int] = {"a": 1}


def in_names(name: str) -> bool:
    return name in NAMES


def in_tags(tag: str) -> bool:
    return TAGS.contains(tag)


def not_in_table(key: str) -> bool:
    return key not in TABLE
"#,
    )?;
    for (name, family) in [
        ("in_names", CollectionTypeId::FrozenList),
        ("in_tags", CollectionTypeId::FrozenSet),
    ] {
        let membership = returned_value(&ir, name)?;
        let IrExprKind::KnownMethodCall {
            receiver,
            kind: MethodKind::Collection(CollectionMethodKind::Contains),
            ..
        } = &membership.kind
        else {
            return Err(format!("`{name}` must lower to membership, got {membership:?}"));
        };
        assert!(
            matches!(receiver.kind, IrExprKind::UnaryOp { op: UnaryOp::Ref, .. }),
            "`{name}` borrows the frozen receiver, got {receiver:?}"
        );
        let IrType::Ref(borrowed) = &receiver.ty else {
            return Err(format!("`{name}`'s receiver is a borrow, got {:?}", receiver.ty));
        };
        assert!(is_frozen_family(borrowed, family), "`{name}` borrows the const itself");
    }
    let negated = returned_value(&ir, "not_in_table")?;
    let IrExprKind::UnaryOp {
        op: UnaryOp::Not,
        operand,
    } = &negated.kind
    else {
        return Err(format!("`not in` negates the membership test, got {negated:?}"));
    };
    let IrExprKind::KnownMethodCall {
        receiver,
        kind: MethodKind::Collection(CollectionMethodKind::Contains),
        ..
    } = &operand.kind
    else {
        return Err(format!("`key not in TABLE` must lower to membership, got {operand:?}"));
    };
    assert!(
        is_frozen_family(&receiver.ty, CollectionTypeId::FrozenDict),
        "a frozen dict keeps its receiver, got {:?}",
        receiver.ty
    );
    Ok(())
}

/// #1757: a comprehension, a generator clause and `set(...)` over a `const` frozen collection read its `list(...)`
/// conversion: the owned elements, or the owned keys of a `FrozenDict`. `set(...)` over a frozen collection whose items
/// are stored as they are typed reads the collection itself.
#[test]
fn iteration_sources_over_frozen_collections_read_owned_items_issue1757() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const NAMES: FrozenList[str] = ["alpha", "beta"]
const TABLE: FrozenDict[str, int] = {"a": 1}
const NUMS: FrozenList[int] = [1, 2]


def keys() -> list[str]:
    return [key for key in TABLE]


def unique_names() -> set[str]:
    return set(NAMES)


def unique_nums() -> set[int]:
    return set(NUMS)


def doubled() -> list[int]:
    items = (n * 2 for n in NUMS)
    return list(items)
"#,
    )?;
    let owned_items = |source: &TypedExpr, family: CollectionTypeId, item: IrType| -> Result<(), String> {
        let IrExprKind::BuiltinCall {
            func: BuiltinFn::CollectionConstructor(CollectionTypeId::List),
            args,
        } = &source.kind
        else {
            return Err(format!("the frozen source is read through `list(...)`, got {source:?}"));
        };
        assert_eq!(source.ty, IrType::List(Box::new(item)));
        assert!(
            args.first().is_some_and(|arg| is_frozen_family(&arg.ty, family)),
            "the conversion reads the const: {args:?}"
        );
        Ok(())
    };

    let keys = returned_value(&ir, "keys")?;
    let IrExprKind::ListComp { iterable, .. } = &keys.kind else {
        return Err(format!("`keys` must lower to a comprehension, got {keys:?}"));
    };
    owned_items(iterable, CollectionTypeId::FrozenDict, IrType::String)?;

    let unique_names = returned_value(&ir, "unique_names")?;
    let IrExprKind::BuiltinCall {
        func: BuiltinFn::CollectionConstructor(CollectionTypeId::Set),
        args,
    } = &unique_names.kind
    else {
        return Err(format!("`unique_names` must lower to `set(...)`, got {unique_names:?}"));
    };
    let source = args.first().ok_or_else(|| "`set` takes the frozen list".to_string())?;
    owned_items(source, CollectionTypeId::FrozenList, IrType::String)?;

    let unique_nums = returned_value(&ir, "unique_nums")?;
    let IrExprKind::BuiltinCall { args, .. } = &unique_nums.kind else {
        return Err(format!("`unique_nums` must lower to `set(...)`, got {unique_nums:?}"));
    };
    assert!(
        args.first()
            .is_some_and(|arg| is_frozen_family(&arg.ty, CollectionTypeId::FrozenList)),
        "a frozen list of `int` is collected as written: {args:?}"
    );

    let (generator, _) = let_binding(function_body(&ir, "doubled")?, "items")?;
    let IrExprKind::Generator { clauses, .. } = &generator.kind else {
        return Err(format!("`list`'s argument is the generator, got {generator:?}"));
    };
    let Some(crate::expr::IrGeneratorClause::For { iterable, .. }) = clauses.first() else {
        return Err(format!("the generator starts with a `for` clause, got {clauses:?}"));
    };
    owned_items(iterable, CollectionTypeId::FrozenList, IrType::Int)
}

/// Return the callback that the `flat_map` call in the named function's returned adapter chain passes its adapter.
fn returned_flat_map_callback<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a TypedExpr, String> {
    let returned = returned_value(ir, name)?;
    let mut call = returned;
    loop {
        match &call.kind {
            IrExprKind::KnownMethodCall {
                kind: MethodKind::Iterator(IteratorMethodKind::FlatMap),
                args,
                ..
            } => {
                return args
                    .first()
                    .map(|arg| &arg.expr)
                    .ok_or_else(|| format!("`{name}` must pass `flat_map` its callback"));
            }
            IrExprKind::KnownMethodCall { receiver, .. } => call = receiver,
            _ => return Err(format!("`{name}` must return a `flat_map` chain, got {returned:?}")),
        }
    }
}

/// A `flat_map` callback hands the adapter the nested iterator it polls one item at a time, never a collected list: a
/// callback returning a generator is passed as written, a list expansion is iterated through `.iter()`, a frozen one
/// through `.iter()` over its owned items, a set expansion through a generator expression over it, and a closure
/// literal keeps its own parameters with its body wrapped.
#[test]
fn flat_map_callbacks_hand_the_adapter_a_nested_iterator() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const WORDS: FrozenList[str] = ["a", "b"]


def pair_gen(n: int) -> Generator[int]:
    yield n
    yield n + 1


def pair_set(n: int) -> set[int]:
    return {n, n + 10}


def pair_list(n: int) -> list[int]:
    return [n, n]


def words(_n: int) -> FrozenList[str]:
    return WORDS


def from_generators(items: list[int]) -> Iterator[int]:
    return items.iter().flat_map(pair_gen)


def from_sets(items: list[int]) -> Iterator[int]:
    return items.iter().flat_map(pair_set)


def from_lists(items: list[int]) -> Iterator[int]:
    return items.iter().flat_map(pair_list)


def from_frozen(items: list[int]) -> Iterator[str]:
    return items.iter().flat_map(words)


def from_closure(items: list[int]) -> list[int]:
    return items.iter().flat_map((n) => [n, n]).collect()
"#,
    )?;

    let from_generators = returned_flat_map_callback(&ir, "from_generators")?;
    assert!(
        !matches!(from_generators.kind, IrExprKind::Closure { .. }),
        "a generator expansion is the nested iterator itself, got {from_generators:?}"
    );

    let from_sets = returned_flat_map_callback(&ir, "from_sets")?;
    let IrExprKind::Closure { body, .. } = &from_sets.kind else {
        return Err(format!(
            "a set expansion must be wrapped in a closure, got {from_sets:?}"
        ));
    };
    let IrExprKind::Generator { clauses, .. } = &body.kind else {
        return Err(format!(
            "a set expansion must be drawn through a generator, got {body:?}"
        ));
    };
    assert!(
        matches!(clauses.as_slice(), [IrGeneratorClause::For { iterable, .. }] if matches!(iterable.ty, IrType::Set(_))),
        "the generator must iterate the set the callback returns, got {clauses:?}"
    );

    for (name, item_ty) in [("from_lists", IrType::Int), ("from_frozen", IrType::String)] {
        let callback = returned_flat_map_callback(&ir, name)?;
        let IrExprKind::Closure { body, .. } = &callback.kind else {
            return Err(format!(
                "`{name}` must wrap its expansion in a closure, got {callback:?}"
            ));
        };
        let IrExprKind::KnownMethodCall {
            receiver,
            kind: MethodKind::Iterator(IteratorMethodKind::Iter),
            ..
        } = &body.kind
        else {
            return Err(format!("`{name}` must iterate its list expansion, got {body:?}"));
        };
        assert_eq!(
            receiver.ty,
            IrType::List(Box::new(item_ty)),
            "`{name}` must iterate an owned list of the flattened items"
        );
    }

    let from_closure = returned_flat_map_callback(&ir, "from_closure")?;
    let IrExprKind::Closure { params, body, .. } = &from_closure.kind else {
        return Err(format!("`from_closure` must stay a closure, got {from_closure:?}"));
    };
    assert!(
        matches!(&body.kind, IrExprKind::KnownMethodCall {
            receiver,
            kind: MethodKind::Iterator(IteratorMethodKind::Iter),
            ..
        } if matches!(receiver.ty, IrType::List(_))),
        "a closure literal's list body must be iterated in place, got {body:?}"
    );
    assert_eq!(
        params.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
        ["n"],
        "a closure literal keeps its own parameter, so Rust infers its type from the adapter's callback slot"
    );
    Ok(())
}
