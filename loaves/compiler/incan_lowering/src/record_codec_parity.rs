//! Every lowering event, native interface, admitted definition, refusal, note and plan-module page the codecs in
//! `incan_mir_plan_native` write reads back unchanged. The plan Loaf is the one owner of these records; byte identity
//! with reference-written records is proven on the captured records corpus.

use incan_mir_plan_native as native;

/// A product event with every field set, including references and positions.
fn product_event() -> native::LoweringEvent {
    native::LoweringEvent {
        identity: "pub::demo::main".into(),
        key: "4f".repeat(32),
        function: "main".into(),
        product: vec![0, 1, 2, 3, 250, 251],
        reused: true,
        admitted: false,
        module: "demo::main".into(),
        role: String::new(),
        demand: String::new(),
        runtime_requirements: 3,
        references: vec![
            native::EventReference {
                kind: "model".into(),
                index: 2,
                name: "Point".into(),
            },
            native::EventReference {
                kind: "enum".into(),
                index: -9,
                name: "Option_i64".into(),
            },
        ],
        positions: vec![
            native::EventPosition { line: 0, column: 5 },
            native::EventPosition { line: 7, column: 1 },
        ],
    }
}

#[test]
fn lowering_events_read_back_unchanged() {
    for event in [
        product_event(),
        native::LoweringEvent {
            role: "entry".into(),
            demand: "demand".into(),
            ..product_event()
        },
    ] {
        let encoded = native::encode_lowering_event(&event).expect("event");
        let read = native::decode_lowering_event(&encoded).expect("the event reads back");
        assert_eq!(
            (
                read.identity,
                read.key,
                read.function,
                read.product,
                read.reused,
                read.admitted
            ),
            (
                event.identity.clone(),
                event.key.clone(),
                event.function.clone(),
                event.product.clone(),
                event.reused,
                event.admitted
            )
        );
        assert_eq!(
            (read.module, read.role, read.demand, read.runtime_requirements),
            (
                event.module.clone(),
                event.role.clone(),
                event.demand.clone(),
                event.runtime_requirements
            )
        );
        let references: Vec<(String, i64, String)> = read
            .references
            .iter()
            .map(|reference| (reference.kind.clone(), reference.index, reference.name.clone()))
            .collect();
        let expected: Vec<(String, i64, String)> = event
            .references
            .iter()
            .map(|reference| (reference.kind.clone(), reference.index, reference.name.clone()))
            .collect();
        assert_eq!(references, expected);
        let positions: Vec<(i64, i64)> = read.positions.iter().map(|at| (at.line, at.column)).collect();
        let expected: Vec<(i64, i64)> = event.positions.iter().map(|at| (at.line, at.column)).collect();
        assert_eq!(positions, expected);
        assert_eq!(native::decode_lowering_event(&encoded).expect("round trip"), event);
    }
}

#[test]
fn interfaces_and_definitions_read_back_unchanged() {
    let interface = native::NativeInterface {
        entry: "main".into(),
        plan: vec![9, 8, 7],
    };
    let encoded = native::encode_native_interface(&interface).expect("interface");
    let read = native::decode_native_interface(&encoded).expect("the interface reads back");
    assert_eq!(
        (read.entry, read.plan),
        (interface.entry.clone(), interface.plan.clone())
    );
    assert_eq!(
        native::decode_native_interface(&encoded).expect("round trip"),
        interface
    );
    let definitions: Vec<native::AdmittedDefinition> = ["a", "b"]
        .iter()
        .map(|function| native::AdmittedDefinition {
            function: (*function).into(),
            identity: format!("pub::demo::{function}"),
            key: "ab".repeat(32),
            digest: format!("sha256:{}", "cd".repeat(32)),
        })
        .collect();
    let encoded = native::encode_admitted_definitions(&definitions).expect("definitions");
    let read = native::decode_admitted_definitions(&encoded).expect("the definitions read back");
    let read: Vec<(String, String, String, String)> = read
        .into_iter()
        .map(|value| (value.function, value.identity, value.key, value.digest))
        .collect();
    let expected: Vec<(String, String, String, String)> = definitions
        .iter()
        .map(|value| {
            (
                value.function.clone(),
                value.identity.clone(),
                value.key.clone(),
                value.digest.clone(),
            )
        })
        .collect();
    assert_eq!(read, expected);
}

#[test]
fn refusals_notes_and_pages_read_back_unchanged() {
    let refusal = native::LoweringRefusal {
        identity: "pub::demo::main".into(),
        function: "main".into(),
        refusal: "checker supplied no complete scalar body witness".into(),
    };
    let encoded = native::encode_lowering_refusal(&refusal).expect("refusal");
    assert_eq!(native::decode_lowering_refusal(&encoded).expect("round trip"), refusal);
    let note = native::AdmittedModule {
        module: "demo::main".into(),
        complete: true,
        functions: vec![native::AdmittedFunction {
            function: "main".into(),
            identity: "pub::demo::main".into(),
            references: vec![native::EventReference {
                kind: "model".into(),
                index: 4,
                name: "Point".into(),
            }],
            placement: None,
        }],
        inspected: false,
    };
    let encoded = native::encode_module_note(&note).expect("note");
    assert_eq!(native::decode_module_note(&encoded).expect("round trip"), note);
    let function = |name: &str| native::PlanModuleFunction {
        identity: format!("pub::demo::{name}"),
        symbol: format!("pub::demo::{name}"),
        function: name.into(),
        key: "ef".repeat(32),
        event: vec![1, 2, 3],
    };
    let page = native::PlanModuleValue {
        module: "demo::main".into(),
        functions: vec![function("a"), function("b")],
    };
    let encoded = native::encode_plan_module(&page).expect("page");
    assert_eq!(native::decode_plan_module(&encoded).expect("round trip"), page);
    let unordered = native::PlanModuleValue {
        functions: vec![function("b"), function("a")],
        ..page
    };
    assert!(native::encode_plan_module(&unordered).is_err());
}
