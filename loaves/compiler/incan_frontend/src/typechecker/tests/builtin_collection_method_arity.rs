//! The builtin list, dict and set methods (frozen or not) take positional arguments only, as many as the registry arity
//! states (#1783): `items.remove(index)` returns `None`, and `counts.get(key)` takes one key and returns an `Option`.
//! `Dict.get` is the one method the registry arity does not cover, since it takes one key or a key and a default
//! (#1561); the checker validates its own arity in `check_expr::access`.

use super::*;

#[test]
fn list_remove_returns_none_and_dict_get_takes_one_key_issue1783() -> Result<(), String> {
    // The collections reference documents what the checker accepts: `items.remove(index)` removes in place and
    // returns `None`, `counts.get(key)` takes one key and returns an `Option`, and `counts.get(key, default)` takes a
    // default of the value type and returns the value itself.
    check_str(
        r#"
static counts: dict[str, int] = {}

def has(name: str) -> bool:
    match counts.get(name):
        Some(_) => return true
        None => return false

def count(name: str) -> int:
    return counts.get(name, 0)

def drop_first(mut values: list[int]) -> int:
    values.remove(0)
    return len(values)
"#,
    )
    .map_err(|errors| format!("the documented forms must check: {errors:?}"))?;

    // Each refused spelling is refused with its own diagnostic.
    for (source, expected) in [
        (
            "def drop(mut values: list[int]) -> None:\n    values.remove()\n",
            "List.remove() expects 1 argument(s), got 0",
        ),
        (
            "def take_first(mut values: list[int]) -> int:\n    return values.remove(0)\n",
            "Return type mismatch: expected 'int'",
        ),
    ] {
        let Err(errors) = check_str(source) else {
            return Err(format!("{source:?} must be refused"));
        };
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "expected `{expected}` for {source:?}, got {:?}",
            errors.iter().map(|error| error.message.as_str()).collect::<Vec<_>>()
        );
    }
    Ok(())
}

#[test]
fn builtin_collection_methods_refuse_a_wrong_argument_count_issue1783() -> Result<(), String> {
    // Every builtin list, dict and set method is typed from the method registry, so each call is checked against the
    // registry arity with positional arguments only; without the check an extra argument was dropped from the generated
    // call and a keyword argument was taken for a positional one.
    let Err(errors) = check_str(
        r#"
def misuse(mut values: list[int], mut counts: dict[str, int], mut seen: set[int]) -> None:
    values.append(1, 2)
    values.pop(0)
    values.swap(0)
    counts.insert("a")
    counts.keys(1)
    seen.add()
    values.remove(banana=0)
    counts.get(key="a")
    values.count(value=1)
    values.index(*values)
"#,
    ) else {
        return Err("every miscounted call must be refused".to_string());
    };
    let messages = errors.iter().map(|error| error.message.as_str()).collect::<Vec<_>>();
    for expected in [
        "List.append() expects 1 argument(s), got 2",
        "List.pop() expects 0 argument(s), got 1",
        "List.swap() expects 2 argument(s), got 1",
        "Dict.insert() expects 2 argument(s), got 1",
        "Dict.keys() expects 0 argument(s), got 1",
        "Set.add() expects 1 argument(s), got 0",
        "Unexpected keyword argument 'banana' when calling 'List.remove'",
        "Unexpected keyword argument 'key' when calling 'Dict.get'",
        "Unexpected keyword argument 'value' when calling 'List.count'",
        "Cannot use `*` unpacking when calling 'List.index'",
    ] {
        assert!(
            messages.iter().any(|message| message.contains(expected)),
            "missing `{expected}` in {messages:?}"
        );
    }
    Ok(())
}
