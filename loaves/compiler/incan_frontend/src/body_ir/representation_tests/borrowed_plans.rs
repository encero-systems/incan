//! RFC 164: what a borrow does lowers to a native plan. A borrowed scalar is read through its reference by an
//! operator, a condition or an owned use; a value assigned to an exclusive borrow replaces what it borrows; an owned
//! `mut` local is lent to an exclusive parameter; a local borrow takes a reference to its value. Plans are proven in
//! the Incan spelling: the Rust spelling is refused before lowering.

use super::*;
use std::collections::BTreeMap;

/// The native spellings of a shared and an exclusive borrow, padded to the width of the explicit ones.
const NATIVE: (&str, &str) = ("&         {}", "&mut         {}");
/// The explicit spellings of a shared and an exclusive borrow.
const EXPLICIT: (&str, &str) = ("Borrowed[{}]", "BorrowedMut[{}]");

/// Expand `{borrow:T}` and `{borrow_mut:T}` markers with one pair of spellings.
fn spelled(template: &str, (shared, exclusive): (&str, &str)) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let marker = &rest[start + 1..start + end];
        match marker.split_once(':') {
            Some(("borrow", name)) => out.push_str(&shared.replace("{}", name)),
            Some(("borrow_mut", name)) => out.push_str(&exclusive.replace("{}", name)),
            _ => out.push_str(&rest[start..=start + end]),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// Check, build and lower one library module, returning each named function's rendered plan. A refused Rust reference
/// spelling is set aside, so the plans of both spellings can be compared.
fn planned(source: &str) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    if let Err(errors) = checker.check_program(&program) {
        let errors: Vec<_> = errors
            .iter()
            .filter(|error| !error.message.contains("in Incan, not `&"))
            .collect();
        if !errors.is_empty() {
            return Err(format!("{errors:?}").into());
        }
    }
    let facts = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: crate::body_ir::ContextModules::default(),
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let module = crate::body_ir::build_body_ir_module(&program, &[], &facts, &context);
    let inputs = incan_lowering::SourceInputs {
        sources: vec![source.to_owned()],
        files: vec!["app.incn".to_owned()],
        ..Default::default()
    };
    let mut published = Vec::new();
    let plan = match incan_lowering::unit_plan(std::slice::from_ref(&module), &inputs, 0, "library", &mut published)? {
        incan_lowering::UnitPlanning::Complete(Some(plan)) => plan,
        incan_lowering::UnitPlanning::Complete(None) => return Err("no plan".into()),
        incan_lowering::UnitPlanning::NeedsRuntime(_, requirements) => {
            return Err(format!("needs runtime {requirements:?}").into());
        }
    };
    Ok(plan
        .functions
        .iter()
        .map(|function| (function.name.clone(), format!("{:?}", function.blocks)))
        .collect())
}

/// The rendered plan of the function named `name`.
fn function<'a>(plans: &'a BTreeMap<String, String>, name: &str) -> Result<&'a String, String> {
    plans
        .get(name)
        .ok_or_else(|| format!("missing planned function {name}"))
}

/// Operators, a condition and an owned use read a borrowed scalar through its reference.
#[test]
fn borrowed_scalars_are_read_through_their_reference() -> Result<(), Box<dyn std::error::Error>> {
    let template = "def add(x: {borrow:int}) -> int:\n    return x + 1\n\ndef negate(x: {borrow:int}) -> int:\n    return -x\n\ndef less(a: {borrow:int}, b: int) -> bool:\n    return a < b\n\ndef check(flag: {borrow:bool}) -> int:\n    if flag:\n        return 1\n    return 0\n\ndef peek(x: {borrow:int}) -> int:\n    return x\n\ndef copy_out(flag: {borrow_mut:bool}) -> bool:\n    return flag\n";
    for spelling in [EXPLICIT] {
        let plans = planned(&spelled(template, spelling))?;
        for name in ["add", "negate", "less", "peek"] {
            assert!(
                function(&plans, name)?.contains("projection=Deref(Int)"),
                "{name}: {}",
                function(&plans, name)?
            );
        }
        for name in ["check", "copy_out"] {
            assert!(
                function(&plans, name)?.contains("projection=Deref(Bool)"),
                "{name}: {}",
                function(&plans, name)?
            );
        }
    }
    Ok(())
}

/// A value assigned to an exclusive borrow, or combined into it, replaces what the borrow names: a scalar, text and
/// a list.
#[test]
fn writes_through_an_exclusive_borrow_replace_its_referent() -> Result<(), Box<dyn std::error::Error>> {
    let template = "def set(mut x: {borrow_mut:int}) -> None:\n    x = 3\n\ndef bump(mut x: {borrow_mut:int}, step: {borrow:int}) -> None:\n    x += step\n\ndef rename(mut text: {borrow_mut:str}) -> None:\n    text = \"changed\"\n\ndef reset(mut items: {borrow_mut:list[int]}) -> None:\n    items = [1]\n";
    for spelling in [EXPLICIT] {
        let plans = planned(&spelled(template, spelling))?;
        for (name, referent) in [
            ("set", "Deref(Int)"),
            ("bump", "Deref(Int)"),
            ("rename", "Deref(String)"),
            ("reset", "Deref(List(Int, 1))"),
        ] {
            let plan = function(&plans, name)?;
            assert!(
                plan.contains(&format!("Assign(Place(local=1, projection={referent}")),
                "{name}: {plan}"
            );
        }
    }
    Ok(())
}

/// A borrowed result reached from its source (`from values`, slice 09B) plans exactly as the native result without a
/// `from` clause, in both spellings: the callee returns the reference and its caller reads through it.
#[test]
fn borrowed_results_reached_from_their_source_plan_as_native_results() -> Result<(), Box<dyn std::error::Error>> {
    let template = "def first(values: {borrow:list[int]}, position: int) -> {borrow:int}{source}:\n    item: {borrow:int} = values[position]\n    return item\n\ndef total(values: list[int]) -> int:\n    chosen = first(values, 0)\n    return chosen + 1\n";
    let unsourced = planned(&spelled(template, NATIVE).replace("{source}", "            "))?;
    for spelling in [NATIVE, EXPLICIT] {
        let sourced = planned(&spelled(template, spelling).replace("{source}", " from values"))?;
        assert_eq!(sourced, unsourced);
    }
    Ok(())
}

/// An owned `mut` local is lent to an exclusive parameter, and a local borrow takes a reference to its value.
#[test]
fn owned_values_are_lent_to_borrows() -> Result<(), Box<dyn std::error::Error>> {
    let template = "model Counter:\n    value: int\n\ndef bump(mut c: {borrow_mut:Counter}) -> None:\n    c.value = c.value + 1\n\ndef lend() -> int:\n    mut c = Counter(value=1)\n    bump(c)\n    return c.value\n\ndef first(items: list[int]) -> int:\n    head: {borrow:int} = items[0]\n    return head + 1\n\ndef alias(mut value: int) -> int:\n    mut view: {borrow_mut:int} = value\n    view = 7\n    return value\n";
    for spelling in [EXPLICIT] {
        let plans = planned(&spelled(template, spelling))?;
        assert!(
            function(&plans, "lend")?.contains("MutBorrow("),
            "{}",
            function(&plans, "lend")?
        );
        assert!(
            function(&plans, "first")?.contains("Borrow("),
            "{}",
            function(&plans, "first")?
        );
        assert!(
            function(&plans, "first")?.contains("projection=Deref(Int)"),
            "{}",
            function(&plans, "first")?
        );
        let alias = function(&plans, "alias")?;
        assert!(alias.contains("MutBorrow("), "{alias}");
        assert!(alias.contains("projection=Deref(Int)"), "{alias}");
    }
    Ok(())
}

/// A builtin list or dict method through a borrow plans on the value the borrow names, in both spellings: `append`
/// through an exclusive borrow, `count`, `keys` and `len` through a shared one.
#[test]
fn builtin_methods_through_borrows_plan_on_their_referent() -> Result<(), Box<dyn std::error::Error>> {
    let template = "def grow(mut items: {borrow_mut:list[int]}) -> None:\n    items.append(1)\n\ndef size(items: {borrow:list[int]}) -> int:\n    return items.count(1) + len(items)\n\ndef names(table: {borrow:dict[str, int]}) -> int:\n    return len(table.keys())\n";
    for spelling in [NATIVE, EXPLICIT] {
        let plans = planned(&spelled(template, spelling))?;
        for name in ["grow", "size", "names"] {
            let plan = function(&plans, name)?;
            assert!(!plan.contains("nsupported"), "{name}: {plan}");
        }
    }
    Ok(())
}
