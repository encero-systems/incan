//! Generated Rust for a static collection read whose argument is read again (#1793), a static dict's `get` returned
//! as `Option[V]`, and a `str` or `FrozenStr` const stored at a destination of the other string type (#1794).

use incan_ir::expr::{IrStaticReferenceKind, VarAccess, VarRefKind};
use incan_ir::{IrExpr, IrExprKind, IrType};

use crate::codegen::IrCodegen;
use crate::conversions::{Conversion, ConversionContext, determine_conversion};
use crate::test_support::parse_program_result;

/// Check, lower and emit one program, returning the generated Rust with all whitespace and the trailing commas of
/// wrapped argument lists removed, so assertions do not depend on how the pretty-printer wraps a line.
fn generated_rust_without_whitespace(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let program = parse_program_result(source)?;
    let code = IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| format!("generation failed: {error:?}"))?;
    Ok(code.split_whitespace().collect::<String>().replace(",)", ")"))
}

/// #1793: a key the program reads again reaches a static dict's `get` and membership test as a view and its `insert`
/// as a copy, in the temporary the storage access reads, so the parameter the arms read afterwards is still there; at
/// its last read it is moved in, and a literal key is stored as an owned `str`. A static index assignment evaluates its
/// value first, so the key read inside the value is a view and the key itself is moved in last; a receiver path that
/// reads the argument's variable (`graph[node].contains(node)`) leaves the argument a view.
#[test]
fn static_collection_arguments_stay_readable_issue1793() -> Result<(), Box<dyn std::error::Error>> {
    let code = generated_rust_without_whitespace(
        r#"
static counts: dict[str, int] = {}
static graph: dict[str, list[str]] = {}


def lookup(name: str) -> str:
    match counts.get(name):
        Some(value) => return f"{name}={value}"
        None => return f"{name} missing"


def record(name: str) -> bool:
    counts.insert(name, 2)
    return name in counts


def bump(name: str) -> None:
    counts[name] = counts.get(name).unwrap_or(0) + 1


def linked(node: str) -> bool:
    return graph[node].contains(node)


def direct(name: str) -> Option[int]:
    return counts.get(name)


def main() -> None:
    counts.insert("a", 1)
    println(lookup("a"))
    println(record("b"))
    bump("a")
    println(linked("a"))
    println(direct("b").unwrap_or(0))
"#,
    )?;
    assert_eq!(
        code.matches("let__incan_static_arg_0=&name;").count(),
        2,
        "the lookups in `lookup` and `bump` view the reused key: {code}"
    );
    assert_eq!(
        code.matches("let__incan_static_arg_0=name.clone();").count(),
        1,
        "the `insert` in `record` copies the reused key: {code}"
    );
    assert!(
        code.contains("let__incan_static_arg_0=name;"),
        "a key at its last read is moved into the temporary: {code}"
    );
    assert!(
        code.contains(".insert(name,__incan_static_rhs)"),
        "the index assignment moves its key in after the value: {code}"
    );
    assert!(
        code.contains("let__incan_static_arg_0=&node;"),
        "an argument the receiver path reads stays a view: {code}"
    );
    assert!(
        code.contains(".get(<_asAsRef<str>>::as_ref(&__incan_static_arg_0)).copied()"),
        "the lookup reads the temporary and copies the value out: {code}"
    );
    assert!(
        code.contains(".insert(__incan_static_arg_0.to_string(),__incan_static_arg_1)"),
        "a literal key is stored as the static's owned `str`: {code}"
    );
    assert!(
        code.contains("->Option<i64>"),
        "`direct` returns the looked-up value as `Option[int]`: {code}"
    );
    Ok(())
}

/// `get` on a dict that is not static storage reads the entry in place and completes the lookup with a copy of the
/// entry, so every dict's `get` answers with the stored value: `copied` for a `Copy` value, `cloned` otherwise.
#[test]
fn local_dict_get_answers_with_the_stored_value() -> Result<(), Box<dyn std::error::Error>> {
    let code = generated_rust_without_whitespace(
        r#"
model Point:
    x: int


def count(table: dict[str, int], name: str) -> int:
    return table.get(name).unwrap_or(0)


def point(table: dict[str, Point], name: str) -> Option[Point]:
    return table.get(name)


def main() -> None:
    println(count({"a": 1}, "a"))
    println(point({}, "a") is None)
"#,
    )?;
    assert!(
        code.contains("table.get(<_asAsRef<str>>::as_ref(&name)).copied().unwrap_or(0)"),
        "a `Copy` value is copied out of the dict: {code}"
    );
    assert!(
        code.contains("table.get(<_asAsRef<str>>::as_ref(&name)).cloned()"),
        "any other value is cloned out of the dict: {code}"
    );
    Ok(())
}

/// A `dict.get` whose result is only read (a `match` or `if let` that reads its bindings with `len`, `println` or not
/// at all) reads the entry in place with no copy, for a generic value, a list of models in a loop, and a Rust value
/// that cannot be copied. A kept result is copied, and a generic value then carries the `Clone` bound it needs: on a
/// generic function and on the `Cache[K, V]` shape, whose generic key also carries the bounds its lookup hashes with.
#[test]
fn dict_get_copies_only_a_kept_value() -> Result<(), Box<dyn std::error::Error>> {
    let code = generated_rust_without_whitespace(
        r#"
from rust::std::sync import Mutex

model Row:
    id: int


model Cache[K, V]:
    items: Dict[K, V]

    def __getitem__(self, key: K) -> Option[V]:
        return self.items.get(key)


def pick[V](table: dict[str, V], key: str) -> Option[V]:
    return table.get(key)


def size[V](table: dict[str, list[V]], key: str) -> int:
    match table.get(key):
        Some(items) => return len(items)
        None => return 0


def total(table: dict[str, list[Row]], keys: list[str]) -> int:
    mut count = 0
    for key in keys:
        match table.get(key):
            Some(rows) => count += len(rows)
            None => count += 0
    return count


def locked(table: dict[str, Mutex[int]], key: str) -> bool:
    match table.get(key):
        Some(_) => return true
        None => return false


def main() -> None:
    println(pick({"a": "x"}, "a").unwrap_or("none"))
    println(size({"a": [1, 2]}, "a"))
    println(total({"a": [Row(id=1)]}, ["a", "b"]))
    empty: dict[str, Mutex[int]] = {}
    println(locked(empty, "a"))
    cache: Cache[str, int] = Cache(items={"a": 1})
    println(cache["a"].unwrap_or(0))
"#,
    )?;
    let in_place_lookups = code.matches("matchtable.get(<_asAsRef<str>>::as_ref(&key)){").count();
    assert_eq!(
        in_place_lookups, 3,
        "`size`, `total` and `locked` read the entry in place: {code}"
    );
    assert!(
        code.contains("V:Clone,>(table:std::collections::HashMap<String,V>,key:String)->Option<V>{returntable.get(<_asAsRef<str>>::as_ref(&key)).cloned();"),
        "`pick` copies the kept value and bounds `V` by `Clone`: {code}"
    );
    assert!(
        code.contains("V,>(table:std::collections::HashMap<String,Vec<V>>"),
        "`size` needs no `Clone` bound: {code}"
    );
    assert!(
        code.contains("impl<K:Eq+std::hash::Hash,V:Clone>Cache<K,V>{")
            && code.contains("returnself.items.get(&key).cloned();"),
        "`Cache.__getitem__` copies the kept value under the bounds its lookup needs: {code}"
    );
    Ok(())
}

/// A lookup whose binding is read after the arm changes the dict (`groups["last"] = []`) or calls a `mut self` method
/// on the dict's owner is copied out first, so the change and the read do not overlap; a change after the last read
/// keeps the in-place read. A read-only lookup over `dict[str, int | str]` matched with type patterns matches the
/// union's variants inside `Some`.
#[test]
fn dict_get_is_copied_when_the_dict_changes_while_it_is_read() -> Result<(), Box<dyn std::error::Error>> {
    let code = generated_rust_without_whitespace(
        r#"
class Groups:
    items: dict[str, list[int]]

    def touch(mut self) -> None:
        self.items["touched"] = []

    def first(mut self, key: str) -> int:
        match self.items.get(key):
            Some(values) =>
                self.touch()
                return len(values)
            None => return 0


def assigned(mut groups: dict[str, list[int]], key: str) -> int:
    match groups.get(key):
        Some(items) =>
            groups["last"] = []
            return len(items)
        None => return 0


def changed_after_reading(mut groups: dict[str, list[int]], key: str) -> int:
    match groups.get(key):
        Some(items) =>
            size = len(items)
            groups["last"] = []
            return size
        None => return 0


def describe(table: dict[str, int | str], key: str) -> str:
    match table.get(key):
        int(number) => return f"int:{number}"
        str(text) => return f"str:{text}"
        None => return "missing"


def main() -> None:
    mut owner = Groups(items={"a": [1]})
    println(owner.first("a"))
    mut groups: dict[str, list[int]] = {"a": [1, 2]}
    println(assigned(groups, "a"))
    println(changed_after_reading(groups, "a"))
    mixed: dict[str, int | str] = {"n": 1, "s": "x"}
    println(describe(mixed, "n"))
"#,
    )?;
    assert!(
        code.contains("matchself.items.get(<_asAsRef<str>>::as_ref(&key)).cloned(){"),
        "`first` copies the entry before its `mut self` call: {code}"
    );
    assert_eq!(
        code.matches("matchgroups.get(<_asAsRef<str>>::as_ref(&key)).cloned(){")
            .count(),
        1,
        "`assigned` copies the entry before the dict changes: {code}"
    );
    assert_eq!(
        code.matches("matchgroups.get(<_asAsRef<str>>::as_ref(&key)){").count(),
        1,
        "`changed_after_reading` reads the entry in place: {code}"
    );
    assert!(
        code.contains("matchtable.get(<_asAsRef<str>>::as_ref(&key)){Some(__IncanUnion"),
        "the union type patterns match inside `Some` over the in-place read: {code}"
    );
    Ok(())
}

/// #1794: a `const` declared `str` is a `'static` string that carries `FrozenStr`, so every `FrozenStr` destination
/// wraps it: a plain return, a `FrozenStr | int` return and argument, a `Some(...)` in an `Option[FrozenStr]` return,
/// argument and binding, an annotated binding, a model field, a list element and a dict value.
#[test]
fn str_const_is_wrapped_at_frozen_str_destinations_issue1794() -> Result<(), Box<dyn std::error::Error>> {
    let code = generated_rust_without_whitespace(
        r#"
const NAME: str = "policy"


model Holder:
    label: FrozenStr


def frozen_or_int(flag: bool) -> FrozenStr | int:
    if flag:
        return NAME
    return 7


def maybe_frozen(flag: bool) -> Option[FrozenStr]:
    if flag:
        return Some(NAME)
    return None


def plain_frozen() -> FrozenStr:
    return NAME


def union_arg(value: FrozenStr | int) -> bool:
    return isinstance(value, str)


def option_arg(value: Option[FrozenStr]) -> bool:
    return value is not None


def main() -> None:
    println(isinstance(frozen_or_int(true), str))
    println(isinstance(maybe_frozen(true), str))
    println(plain_frozen())
    println(union_arg(NAME))
    println(option_arg(Some(NAME)))
    bound: FrozenStr = NAME
    wrapped: Option[FrozenStr] = Some(NAME)
    println(bound)
    println(wrapped is not None)
    held = Holder(label=NAME)
    labels: list[FrozenStr] = [NAME]
    by_key: dict[str, FrozenStr] = {"k": NAME}
    println(held.label)
    println(len(labels) + len(by_key))
"#,
    )?;
    let wrapped_name = "incan_std_core::frozen::FrozenStr::new(NAME)";
    assert_eq!(
        code.matches(wrapped_name).count(),
        10,
        "every `FrozenStr` destination wraps the const: {code}"
    );
    assert!(
        code.contains(&format!("::V0({wrapped_name})")),
        "the union member is the wrapped const: {code}"
    );
    assert!(
        code.contains(&format!("Some({wrapped_name})")),
        "the `Some` payload is the wrapped const: {code}"
    );
    assert!(
        !code.contains("NAME.to_string()") && !code.contains("returnNAME;"),
        "the const never reaches a `FrozenStr` destination unwrapped or as an owned string: {code}"
    );
    Ok(())
}

/// The reverse direction: a `FrozenStr` const at a `str` destination (a `Some(...)` in an `Option[str]` return,
/// argument and binding, a `str | int` return and argument, a plain return, a model field, a list element and a dict
/// value) is converted to an owned string.
#[test]
fn frozen_str_const_is_converted_at_str_destinations_issue1794() -> Result<(), Box<dyn std::error::Error>> {
    let code = generated_rust_without_whitespace(
        r#"
const FROZEN: FrozenStr = "frozen"


model Text:
    text: str


def maybe_text(flag: bool) -> Option[str]:
    if flag:
        return Some(FROZEN)
    return None


def text_or_int(flag: bool) -> str | int:
    if flag:
        return FROZEN
    return 7


def plain_text() -> str:
    return FROZEN


def union_arg(value: str | int) -> bool:
    return isinstance(value, str)


def option_arg(value: Option[str]) -> bool:
    return value is not None


def main() -> None:
    println(isinstance(maybe_text(true), str))
    println(isinstance(text_or_int(true), str))
    println(plain_text())
    println(union_arg(FROZEN))
    println(option_arg(Some(FROZEN)))
    bound: Option[str] = Some(FROZEN)
    println(bound is not None)
    held = Text(text=FROZEN)
    texts: list[str] = [FROZEN]
    by_key: dict[str, str] = {"k": FROZEN}
    println(held.text)
    println(len(texts) + len(by_key))
"#,
    )?;
    assert_eq!(
        code.matches("FROZEN.to_string()").count(),
        9,
        "every `str` destination converts the const: {code}"
    );
    assert!(
        !code.contains("Some(FROZEN)") && !code.contains("returnFROZEN;"),
        "the const never reaches a `str` destination as a `FrozenStr`: {code}"
    );
    Ok(())
}

/// #1794: the conversion planner wraps a `'static` string, the storage of a `const` declared `str`, in `FrozenStr` at
/// every Incan destination that expects one, and at no Rust-facing boundary. A value that already is a `FrozenStr`
/// needs nothing, and a `'static` string at a `str` destination keeps its owned-string conversion.
#[test]
fn planner_wraps_a_static_str_at_frozen_str_destinations_issue1794() {
    let const_read = IrExpr::new(
        IrExprKind::StaticRead {
            name: "NAME".to_string(),
            reference_kind: IrStaticReferenceKind::Source,
        },
        IrType::StaticStr,
    );
    let frozen_var = IrExpr::new(
        IrExprKind::Var {
            name: "label".to_string(),
            access: VarAccess::Read,
            ref_kind: VarRefKind::Value,
        },
        IrType::FrozenStr,
    );
    for context in [
        ConversionContext::IncanFunctionArg,
        ConversionContext::IncanFunctionArgInReturn,
        ConversionContext::StructField,
        ConversionContext::CollectionElement,
        ConversionContext::Assignment,
        ConversionContext::ReturnValue,
    ] {
        assert_eq!(
            determine_conversion(&const_read, Some(&IrType::FrozenStr), context),
            Conversion::ToFrozenStr,
            "{context:?} wraps the const"
        );
        assert_ne!(
            determine_conversion(&frozen_var, Some(&IrType::FrozenStr), context),
            Conversion::ToFrozenStr,
            "{context:?} leaves a `FrozenStr` as it is"
        );
        assert_eq!(
            determine_conversion(&const_read, Some(&IrType::String), context),
            Conversion::ToString,
            "{context:?} still makes an owned `str` from the const"
        );
    }
    for context in [
        ConversionContext::ExternalFunctionArg,
        ConversionContext::MethodArg,
        ConversionContext::MatchScrutinee,
    ] {
        assert_ne!(
            determine_conversion(&const_read, Some(&IrType::FrozenStr), context),
            Conversion::ToFrozenStr,
            "{context:?} is a Rust-facing boundary"
        );
    }
    assert_eq!(
        Conversion::ToFrozenStr.apply(quote::quote! { NAME }).to_string(),
        quote::quote! { incan_std_core::frozen::FrozenStr::new(NAME) }.to_string()
    );
}
