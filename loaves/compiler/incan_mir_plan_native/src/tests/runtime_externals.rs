//! Scalar plan products retain the exact linked callable in both the external table and call terminators.

use std::collections::BTreeSet;

use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
    encode_incan_symbol_identity,
};

use super::support::{assign_at, block, empty_function, local, place_at, returning};
use crate::body::{
    Callee, CalleeKind, Constant, Operand, OperandKind, Projection, RvalueKind, Terminator, TerminatorKind, Unwind,
};
use crate::plan::{ExternalFunction, Function};
use crate::positions::WirePositions;
use crate::records::function::{
    decode_product, decode_runtime, encode_product, encode_runtime, place_product_record, product_nominals,
};
use crate::types::{PlanType, source_span};

/// Round-trip runtime, package function and method calls, and refuse uncertified external identities.
#[test]
fn product_keeps_a_rust_external() -> Result<(), String> {
    let method = package_symbol(
        "incan_stdlib_system",
        &["fs", "path"],
        "read_text",
        SymbolNamespace::Member,
        SemanticSourceTargetKind::Method,
    );
    let function = package_symbol(
        "incan_stdlib_data",
        &["json"],
        "parse_jsonl",
        SymbolNamespace::OrdinaryLexical,
        SemanticSourceTargetKind::Function,
    );
    let read_text = format!("incan_stdlib_system::{method}");
    let span = source_span("lists.incn", 44, 1);
    let call = TerminatorKind::Call(
        Callee {
            kind: CalleeKind::External(read_text.clone()),
            span: span.clone(),
        },
        Vec::new(),
        place_at(0, Projection::Whole, &span),
        1,
        Unwind::Continue,
    );
    let instance = Function::new(
        "append_list_3_105_110_116".to_owned(),
        Vec::new(),
        PlanType::Unit,
        vec![local(PlanType::Unit, &span)],
        vec![
            block(
                Vec::new(),
                Terminator {
                    kind: call,
                    span: span.clone(),
                },
                &span,
            ),
            block(Vec::new(), returning(&span), &span),
        ],
        span.clone(),
    );
    let push = ExternalFunction {
        path: "std::vec::Vec::push".to_owned(),
        type_arguments: vec![PlanType::Int],
        parameters: vec![PlanType::Int],
        return_type: PlanType::Unit,
        span: span.clone(),
    };
    let mut runtime = vec![push.clone()];
    for path in [
        read_text.clone(),
        format!("incan_stdlib_data::{function}"),
        "incan_std_data::json::__private::stringify_or_raise".to_owned(),
    ] {
        runtime.push(ExternalFunction { path, ..push.clone() });
    }
    let references = product_nominals(&instance, &runtime, &[])?;
    let product = encode_product(&instance, &runtime, &[], &[])?;
    let (restored, restored_runtime) = decode_product(&product, &references)?;
    assert!(restored == instance, "the instance did not survive its product");
    assert!(
        restored_runtime == runtime,
        "the Rust external did not survive its product"
    );
    let model = package_symbol(
        "incan_stdlib_system",
        &["fs", "path"],
        "Path",
        SymbolNamespace::OrdinaryLexical,
        SemanticSourceTargetKind::Model,
    );
    for path in [
        format!("foreign::{method}"),
        "foreign::__incan_v7_deadcafe".to_owned(),
        "unqualified_host_function".to_owned(),
        format!("incan_stdlib_system::{model}"),
    ] {
        let wrong_provider = ExternalFunction { path, ..push.clone() };
        assert!(encode_product(&instance, &[wrong_provider], &[], &[]).is_err());
    }
    Ok(())
}

/// A Loaf's Rust-facet function, called and required, round-trips by the path its crate links it at: the producer
/// profile a product is keyed by fingerprints every dependency interface, so the path is fixed for the product's key.
#[test]
fn product_keeps_a_loaf_rust_facet_external() -> Result<(), String> {
    let (caller, runtime) = facet_caller();
    let references = product_nominals(&caller, &runtime, &[])?;
    let product = encode_product(&caller, &runtime, &[], &[])?;
    let (restored, restored_runtime) = decode_product(&product, &references)?;
    assert!(
        restored == caller,
        "the call of a Rust-facet function did not survive its product"
    );
    assert!(
        restored_runtime == runtime,
        "a Rust-facet requirement did not survive its product"
    );
    Ok(())
}

/// A product naming a Rust-facet function restores only in a unit that links the function's crate; one naming
/// another crate is refused rather than reaching rustc as an unresolved path. A unit's runtime inventory, which its
/// native interface carries without that check, keeps the bounded identities.
#[test]
fn a_restored_product_names_only_crates_its_unit_links() -> Result<(), String> {
    let (caller, runtime) = facet_caller();
    let references = product_nominals(&caller, &runtime, &[])?;
    let product = encode_product(&caller, &runtime, &[], &[])?;
    let placed = place_product_record(&product, &WirePositions::preserve())?;
    let linked: BTreeSet<String> = ["incan_stdlib_primitives", "internal_models_native", "incan_loaf_names"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let (restored, restored_runtime, _) = placed.restore(&references, &linked)?;
    assert!(restored == caller, "a product of linked crates did not restore");
    assert!(
        restored_runtime == runtime,
        "a product of linked crates lost its demand"
    );
    for name in ["incan_stdlib_primitives", "internal_models_native", "incan_loaf_names"] {
        let mut unlinked = linked.clone();
        unlinked.remove(name);
        assert!(
            placed.restore(&references, &unlinked).is_err(),
            "a product naming {name} restored in a unit that does not link it"
        );
    }
    assert!(
        encode_runtime(&runtime).is_err(),
        "a runtime inventory kept a linked-crate identity"
    );
    Ok(())
}

/// A Rust constant, of Rust's own crates or of a Loaf's Rust facet, round-trips by the path its crate exports it at,
/// and a product reading a Loaf's constant restores only in a unit that links that Loaf's crate.
#[test]
fn product_keeps_a_rust_constant() -> Result<(), String> {
    let span = source_span("profiles.incn", 7, 5);
    let constant = |path: &str| Operand {
        kind: OperandKind::Literal(Constant::RustItem(path.to_owned(), PlanType::StaticText)),
        span: span.clone(),
    };
    let mut reader = empty_function("profile", PlanType::StaticText, &span);
    reader.blocks[0].statements = vec![
        assign_at(
            0,
            Projection::Whole,
            RvalueKind::Use(constant("std::env::consts::OS")),
            &span,
        ),
        assign_at(
            0,
            Projection::Whole,
            RvalueKind::Use(constant("internal_models_native::CHECKER_PROFILE")),
            &span,
        ),
    ];
    let product = encode_product(&reader, &[], &[], &[])?;
    let (restored, _) = decode_product(&product, &[])?;
    assert!(restored == reader, "a Rust constant did not survive its product");
    let placed = place_product_record(&product, &WirePositions::preserve())?;
    assert!(
        placed.restore(&[], &BTreeSet::new()).is_err(),
        "a product reading a Loaf's constant restored in a unit that does not link its crate"
    );
    let linked: BTreeSet<String> = ["internal_models_native".to_owned()].into_iter().collect();
    let (linked_reader, _, _) = placed.restore(&[], &linked)?;
    assert!(
        linked_reader == reader,
        "a product reading a linked crate's constant did not restore"
    );
    Ok(())
}

/// A primitive receiver's method, such as `f64::powf` or `i64::pow`, is Rust's own item and not a crate named `f64`:
/// a product calling one restores in a unit that links no crate for it, and a unit's runtime inventory keeps it.
#[test]
fn a_primitive_method_is_rusts_own_item() -> Result<(), String> {
    let span = source_span("numbers.incn", 3, 5);
    let call = TerminatorKind::Call(
        Callee {
            kind: CalleeKind::External("f64::powf".to_owned()),
            span: span.clone(),
        },
        Vec::new(),
        place_at(0, Projection::Whole, &span),
        1,
        Unwind::Continue,
    );
    let caller = Function::new(
        "power".to_owned(),
        Vec::new(),
        PlanType::Unit,
        vec![local(PlanType::Unit, &span)],
        vec![
            block(
                Vec::new(),
                Terminator {
                    kind: call,
                    span: span.clone(),
                },
                &span,
            ),
            block(Vec::new(), returning(&span), &span),
        ],
        span.clone(),
    );
    let powf = ExternalFunction {
        path: "f64::powf".to_owned(),
        type_arguments: Vec::new(),
        parameters: vec![PlanType::Float, PlanType::Float],
        return_type: PlanType::Float,
        span: span.clone(),
    };
    let runtime = vec![
        powf.clone(),
        ExternalFunction {
            path: "i64::pow".to_owned(),
            parameters: vec![PlanType::Int, PlanType::U32],
            return_type: PlanType::Int,
            ..powf
        },
    ];
    let references = product_nominals(&caller, &runtime, &[])?;
    let product = encode_product(&caller, &runtime, &[], &[])?;
    let placed = place_product_record(&product, &WirePositions::preserve())?;
    let (restored, restored_runtime, _) = placed.restore(&references, &BTreeSet::new())?;
    assert!(
        restored == caller && restored_runtime == runtime,
        "a product calling a primitive method did not restore in a unit that links no crate"
    );
    assert!(
        decode_runtime(&encode_runtime(&runtime)?)? == runtime,
        "a runtime inventory did not keep primitive methods"
    );
    Ok(())
}

/// A caller of a Rust-facet function and the Rust-facet functions it requires.
fn facet_caller() -> (Function, Vec<ExternalFunction>) {
    let span = source_span("records.incn", 12, 5);
    let call = TerminatorKind::Call(
        Callee {
            kind: CalleeKind::External("incan_stdlib_primitives::encode_utf8".to_owned()),
            span: span.clone(),
        },
        Vec::new(),
        place_at(0, Projection::Whole, &span),
        1,
        Unwind::Continue,
    );
    let caller = Function::new(
        "encode_key".to_owned(),
        Vec::new(),
        PlanType::Unit,
        vec![local(PlanType::Unit, &span)],
        vec![
            block(
                Vec::new(),
                Terminator {
                    kind: call,
                    span: span.clone(),
                },
                &span,
            ),
            block(Vec::new(), returning(&span), &span),
        ],
        span.clone(),
    );
    let required = ExternalFunction {
        path: "internal_models_native::encode_record_key".to_owned(),
        type_arguments: Vec::new(),
        parameters: vec![PlanType::String],
        return_type: PlanType::Unit,
        span: span.clone(),
    };
    let runtime = vec![
        required.clone(),
        ExternalFunction {
            path: "internal_models_native::ModelField::__incan_default_canonical".to_owned(),
            ..required.clone()
        },
        ExternalFunction {
            path: "incan_loaf_names::artifact_stem".to_owned(),
            ..required
        },
    ];
    (caller, runtime)
}

/// Emit the native compiler identity used for an exported declaration in this fixture.
fn package_symbol(
    package: &str,
    module: &[&str],
    name: &str,
    namespace: SymbolNamespace,
    kind: SemanticSourceTargetKind,
) -> String {
    encode_incan_symbol_identity(&CanonicalSymbolId {
        namespace,
        kind,
        origin: SymbolOrigin::Package {
            library: package.to_owned(),
            module_path: module.iter().map(|part| (*part).to_owned()).collect(),
        },
        declaration_name: name.to_owned(),
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(3, 8),
    })
}
