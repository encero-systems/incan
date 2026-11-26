//! The plan records publish the Incan declarations' field defaults, which agree with the Rust constructors.

use crate::body::{FieldProjection, ForeignFunction};
use crate::nominal::{NominalReference, capture_remap, unit_nominals};
use crate::plan::{Carrier, EnumDeclaration, ExternalFunction, Function, ModelDeclaration, Plan, StaticCell};
use crate::positions::{PositionChange, WirePositions};
use crate::types::{PlanType, source_span};

/// Every published default equals the value a Rust constructor fills in for that field.
#[test]
fn published_defaults_match_the_constructors() {
    let span = source_span("defaults.incn", 1, 1);
    let function = Function::new(
        "f".to_owned(),
        Vec::new(),
        PlanType::Unit,
        Vec::new(),
        Vec::new(),
        span.clone(),
    );
    assert!(function.is_async == Function::__incan_default_is_async());
    assert!(function.future_callee == Function::__incan_default_future_callee());
    assert!(function.attributes == Function::__incan_default_attributes());
    assert!(function.signature_lifetimes == Function::__incan_default_signature_lifetimes());
    assert!(function.generic_loans == Function::__incan_default_generic_loans());
    assert!(function.retained_result_source == Function::__incan_default_retained_result_source());
    let plan = Plan::new(Vec::new(), Vec::new(), Vec::new(), Vec::new(), span.clone());
    assert!(plan.statics == Plan::__incan_default_statics() && plan.tests == Plan::__incan_default_tests());
    assert!(plan.programs == Plan::__incan_default_programs() && plan.exports == Plan::__incan_default_exports());
    let model = ModelDeclaration::new("M".to_owned(), true, Vec::new(), Vec::new(), Vec::new(), span.clone());
    assert!(model.view_lifetime_count == ModelDeclaration::__incan_default_view_lifetime_count());
    assert!(model.reflection_metadata == ModelDeclaration::__incan_default_reflection_metadata());
    assert!(model.unavailable_native_fields == ModelDeclaration::__incan_default_unavailable_native_fields());
    let declaration = EnumDeclaration::new("E".to_owned(), false, Vec::new(), Vec::new(), span.clone());
    assert!(declaration.carrier == EnumDeclaration::__incan_default_carrier());
    assert!(EnumDeclaration::__incan_default_carrier() == Carrier::Declared);
    assert!(declaration.semantic_key == EnumDeclaration::__incan_default_semantic_key());
    assert!(declaration.closure_items == EnumDeclaration::__incan_default_closure_items());
    let step = FieldProjection::new(0, PlanType::Int);
    assert!(step.variant == FieldProjection::__incan_default_variant());
    assert!(step.dereferences == FieldProjection::__incan_default_dereferences());
    assert!(NominalReference::__incan_default_name().is_empty());
    let remap = capture_remap();
    assert!(remap.fold == crate::nominal::NominalRemap::__incan_default_fold());
    assert!(!crate::nominal::NominalRemap::__incan_default_preserve());
    let unit = unit_nominals(Vec::new(), Vec::new(), Vec::new(), Vec::new());
    assert!(unit.carriers == crate::nominal::UnitNominals::__incan_default_carriers());
    assert!(WirePositions::__incan_default_change() == PositionChange::Preserve);
    assert!(WirePositions::preserve().change == WirePositions::__incan_default_change());
    assert!(ExternalFunction::__incan_default_type_arguments().is_empty());
    assert!(!StaticCell::__incan_default_module_completion());
    assert!(ForeignFunction::__incan_default_clove().is_empty() && !ForeignFunction::__incan_default_framework());
}

/// The lowering events and admitted modules publish the reference defaults Oven's constructions leave out, print in
/// the reference models' `Name(field=value, ...)` text, and key sets by value.
#[test]
fn lowering_event_records_keep_the_reference_surface() {
    use crate::records::lowering_events::{
        AdmittedDefinition, AdmittedFunction, AdmittedModule, EventPosition, EventReference, LoweringEvent,
        admitted_definitions_filename, native_interface_filename,
    };
    use crate::records::plan_module::{PlanModuleFunction, PlanModuleValue};
    use std::collections::HashSet;

    assert!(!LoweringEvent::__incan_default_admitted());
    assert!(LoweringEvent::__incan_default_module().is_empty());
    assert!(LoweringEvent::__incan_default_role().is_empty());
    assert!(LoweringEvent::__incan_default_demand().is_empty());
    assert_eq!(LoweringEvent::__incan_default_runtime_requirements(), 0);
    assert!(LoweringEvent::__incan_default_references().is_empty());
    assert!(LoweringEvent::__incan_default_positions().is_empty());
    assert_eq!(AdmittedFunction::__incan_default_placement(), None);
    assert!(!AdmittedModule::__incan_default_inspected());
    assert_eq!(native_interface_filename(), "native-interface.crumb");
    assert_eq!(admitted_definitions_filename(), "admitted-definitions.crumb");

    let position = EventPosition { line: 1, column: 2 };
    let copied = position;
    assert_eq!(format!("{position:?}"), "EventPosition(line=1, column=2)");
    assert_eq!(copied, position);
    let reference = EventReference {
        kind: "model".into(),
        index: 0,
        name: "M".into(),
    };
    assert_eq!(
        format!("{reference:?}"),
        "EventReference(kind=\"model\", index=0, name=\"M\")"
    );
    let definition = AdmittedDefinition {
        function: "f".into(),
        identity: "i".into(),
        key: "k".into(),
        digest: "d".into(),
    };
    let keyed: HashSet<_> = [definition.clone(), definition].into_iter().collect();
    assert_eq!(keyed.len(), 1);
    let plan = PlanModuleValue {
        module: "m".into(),
        functions: vec![PlanModuleFunction {
            identity: "i".into(),
            symbol: "s".into(),
            function: "f".into(),
            key: "k".into(),
            event: vec![],
        }],
    };
    assert_eq!(
        format!("{plan:?}"),
        "PlanModuleValue(module=\"m\", functions=[PlanModuleFunction(identity=\"i\", symbol=\"s\", function=\"f\", key=\"k\", event=[])])"
    );
}
