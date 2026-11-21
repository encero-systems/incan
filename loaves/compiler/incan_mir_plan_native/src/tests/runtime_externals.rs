//! Scalar plan products retain the exact linked callable in both the external table and call terminators.

use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
    encode_incan_symbol_identity,
};

use super::support::{block, local, place_at, returning};
use crate::body::{Callee, CalleeKind, Projection, Terminator, TerminatorKind, Unwind};
use crate::plan::{ExternalFunction, Function};
use crate::records::function::{decode_product, encode_product, product_nominals};
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
        "incan_std_data::unrelated_host_function".to_owned(),
        format!("incan_stdlib_system::{model}"),
    ] {
        let wrong_provider = ExternalFunction { path, ..push.clone() };
        assert!(encode_product(&instance, &[wrong_provider], &[], &[]).is_err());
    }
    Ok(())
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
