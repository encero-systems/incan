//! Native per-declaration products preserve the formatting frame of an unchanged program entry, and name Incan
//! identities only, ported from the reference lowering's `test_scalar_products`.

use std::collections::BTreeSet;

use super::support::{assign_at, block, copy_at, empty_function, local, place_at, returning};
use crate::body::{
    Callee, CalleeKind, Constant, Operand, OperandKind, Projection, RvalueKind, Terminator, TerminatorKind, Unwind,
};
use crate::plan::{ExternalFunction, Function};
use crate::positions::{PositionChange, WirePositions};
use crate::records::function::{
    decode_product, decode_scalar, encode_product, encode_scalar, place_product_record, transform_product_positions,
};
use crate::types::{PlanType, SourceSpan, source_span};

/// Translate every typed function, frame and runtime span together.
fn relocate_product(encoded: &[u8], file: &str, line: i64) -> Result<Vec<u8>, String> {
    let change = PositionChange::Relocate(String::new(), 0, file.to_owned(), line);
    transform_product_positions(encoded, &WirePositions::new(change), &mut Vec::new())
}

fn text(value: &str, span: &SourceSpan) -> Operand {
    Operand {
        kind: OperandKind::Literal(Constant::Text(value.to_owned())),
        span: span.clone(),
    }
}

fn copy(local: i64, span: &SourceSpan) -> Operand {
    copy_at(local, Projection::Whole, span)
}

/// The formatting frame of an entry that prints an f-string: fixed parts arrays, their shared slices and one
/// instantiated call.
fn formatting_frame(span: &SourceSpan) -> Function {
    let locals = vec![
        local(PlanType::Unit, span),
        local(PlanType::StrArray(2), span),
        local(PlanType::StrArrayRef(2), span),
        local(PlanType::StrSlice, span),
        local(PlanType::StringArray(1), span),
        local(PlanType::StringArrayRef(1), span),
        local(PlanType::StringSlice, span),
    ];
    let statements = vec![
        assign_at(
            1,
            Projection::Whole,
            RvalueKind::Array(vec![text("before", span), text("after", span)]),
            span,
        ),
        assign_at(
            2,
            Projection::Whole,
            RvalueKind::Borrow(place_at(1, Projection::Whole, span)),
            span,
        ),
        assign_at(3, Projection::Whole, RvalueKind::UnsizeSlice(copy(2, span)), span),
        assign_at(4, Projection::Whole, RvalueKind::Array(vec![text("value", span)]), span),
        assign_at(
            5,
            Projection::Whole,
            RvalueKind::Borrow(place_at(4, Projection::Whole, span)),
            span,
        ),
        assign_at(6, Projection::Whole, RvalueKind::UnsizeSlice(copy(5, span)), span),
    ];
    let call = Terminator {
        kind: TerminatorKind::Call(
            Callee {
                kind: CalleeKind::Instantiated(
                    "incan_stdlib::fstring".to_owned(),
                    vec![PlanType::StrSlice, PlanType::StringSlice],
                ),
                span: span.clone(),
            },
            vec![copy(3, span), copy(6, span)],
            place_at(0, Projection::Whole, span),
            1,
            Unwind::Continue,
        ),
        span: span.clone(),
    };
    let blocks = vec![block(statements, call, span), block(Vec::new(), returning(span), span)];
    Function::new(
        "main".to_owned(),
        Vec::new(),
        PlanType::Unit,
        locals,
        blocks,
        span.clone(),
    )
}

/// Fixed formatting arrays, their shared slices and an instantiated call survive one exact product round trip.
#[test]
fn native_product_round_trips_formatting_frame() -> Result<(), String> {
    let span = source_span("formatting_frame.incn", 1, 1);
    let function = formatting_frame(&span);
    assert!(decode_scalar(&encode_scalar(&function)?)? == function);
    let relative = relocate_product(&encode_product(&function, &[], &[], &[])?, "<owner>", 1)?;
    let (moved, runtime) = decode_product(&relocate_product(&relative, "moved.incn", 17)?, &[])?;
    assert!(moved.span.line == 17 && moved.span.file.to_string() == "moved.incn");
    assert_eq!(moved.blocks[0].statements[0].span.line, 17);
    assert!(runtime.is_empty());
    let (original, _runtime) = decode_product(&relocate_product(&relative, "formatting_frame.incn", 1)?, &[])?;
    assert!(original == function);
    Ok(())
}

fn external(path: &str, parameters: Vec<PlanType>, return_type: PlanType, span: &SourceSpan) -> ExternalFunction {
    ExternalFunction {
        path: path.to_owned(),
        type_arguments: Vec::new(),
        parameters,
        return_type,
        span: span.clone(),
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle.as_bytes())
}

/// The stored product names Incan identities, never a native crate path or carrier name. A call into Rust's `std`,
/// `core` or `alloc`, whose paths the toolchain pins, keeps the `rust::<path>` identity Incan source imports it by. A
/// call into any other Rust crate keeps `rust::<path>` too, and restores only in a unit that links that crate.
#[test]
fn native_product_records_expose_only_incan_identities() -> Result<(), String> {
    let span = source_span("interface_audit.incn", 1, 1);
    let mut function = empty_function("main", PlanType::String, &span);
    function.locals = vec![
        local(PlanType::String, &span),
        local(PlanType::StrRef, &span),
        local(PlanType::StrSlice, &span),
        local(PlanType::StringSlice, &span),
    ];
    let runtime = vec![
        external(
            "incan_native_runtime::initialize_panic_hook",
            Vec::new(),
            PlanType::Unit,
            &span,
        ),
        external(
            "incan_std_primitives::strings::str_concat",
            vec![PlanType::StrRef, PlanType::StrRef],
            PlanType::String,
            &span,
        ),
        external(
            "incan_std_primitives::strings::fstring",
            vec![PlanType::StrSlice, PlanType::StringSlice],
            PlanType::String,
            &span,
        ),
        external(
            "incan_native_runtime::println_text",
            vec![PlanType::String],
            PlanType::Unit,
            &span,
        ),
    ];
    let product = encode_product(&function, &runtime, &[], &[])?;
    assert!(product.starts_with(b"CRMB"));
    for path in ["builtin::print", "std::strings::str_concat"] {
        assert!(contains(&product, path), "missing Incan runtime authority {path}");
    }
    for rust_name in ["incan_native_runtime::", "incan_std_primitives::", "incan_stdlib::"] {
        assert!(
            !contains(&product, rust_name),
            "stored product exposes native representation {rust_name}"
        );
    }
    let (restored, restored_runtime) = decode_product(&product, &[])?;
    assert!(restored == function && restored_runtime == runtime);
    for path in [
        "std::vec::Vec::push",
        "core::clone::Clone::clone",
        "alloc::vec::Vec::new",
    ] {
        let standard = vec![external(path, vec![PlanType::StrRef], PlanType::String, &span)];
        let stored = encode_product(&function, &standard, &[], &[])?;
        assert!(
            contains(&stored, &format!("rust::{path}")),
            "stored product does not name `{path}` by its Rust import"
        );
        let (restored_standard, restored_externals) = decode_product(&stored, &[])?;
        assert!(restored_standard == function && restored_externals == standard);
    }
    let other_crate = vec![external(
        "serde_json::to_string",
        vec![PlanType::StrRef],
        PlanType::String,
        &span,
    )];
    let stored = encode_product(&function, &other_crate, &[], &[])?;
    assert!(
        contains(&stored, "rust::serde_json::to_string"),
        "stored product does not name a linked crate's function by its path"
    );
    let placed = place_product_record(&stored, &WirePositions::preserve())?;
    assert!(
        placed.restore(&[], &BTreeSet::new()).is_err(),
        "stored product restored a function of a crate its unit does not link"
    );
    let linked: BTreeSet<String> = ["serde_json".to_owned()].into_iter().collect();
    let (restored_other, restored_externals, _) = placed.restore(&[], &linked)?;
    assert!(restored_other == function && restored_externals == other_crate);
    Ok(())
}
