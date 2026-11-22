//! A unit's product nominal table describes each declaration once per planning: when its tables grow, only the new
//! rows are described, and the table equals one described afresh from the grown tables.

use std::sync::Arc;

use incan_mir_plan_native::UnitNominals;
use incan_semantics_core::body_ir::{BodyIrModule, NominalDeclaration};
use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, HirSourceSpan, SemanticSourceTargetKind};

use crate::native_type_identity::constructor_index;
use crate::nominal_records::{described_rows, product_nominal_table};
use crate::program_context::{retain_nominal_tables, synthetic_program_context};

fn model(name: &str, offset: usize) -> NominalDeclaration {
    NominalDeclaration {
        record_package_identity: None,
        trait_adoptions: Vec::new(),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("app::tables", offset, offset + 10),
        canonical: CanonicalSymbolId::module_declaration(
            vec!["app".into(), "tables".into()],
            name,
            SemanticSourceTargetKind::Model,
            HirSourceSpan::new(offset, offset + 10),
        ),
        name: name.into(),
        fields: Vec::new(),
        field_identities: Vec::new(),
        field_aliases: Vec::new(),
        field_types: Vec::new(),
        owned_callable_fields: Vec::new(),
        field_public: Vec::new(),
        reflection_fields: None,
        display_method: None,
        newtype_deserialize: Default::default(),
        newtype_string_conversion: Default::default(),
        reflection_metadata: None,
        comparison_equality: None,
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        validated_constructor: None,
        method_partial_defaults: Vec::new(),
        derives: Vec::new(),
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    }
}

/// The parts of a table a product resolves its declarations against.
fn same_table(left: &UnitNominals, right: &UnitNominals) -> bool {
    left.models == right.models
        && left.enums == right.enums
        && left.model_names == right.model_names
        && left.enum_names == right.enum_names
        && left.model_positions == right.model_positions
        && left.enum_positions == right.enum_positions
        && left.carriers == right.carriers
        && left.carrier_names == right.carrier_names
        && left.statics == right.statics
        && left.static_positions == right.static_positions
}

#[test]
fn a_grown_table_describes_only_its_new_rows_and_equals_a_fresh_one() -> Result<(), String> {
    let mut module = BodyIrModule::declarations_only("app::tables".into());
    module.nominal_declarations = vec![model("First", 0), model("Second", 20)];
    let mut program = synthetic_program_context(&module, Arc::new(constructor_index(&module.nominal_declarations)));
    let first = product_nominal_table(&mut program, &[], Some("products"));
    assert_eq!(described_rows(&program), (2, 0, 0));
    module.nominal_declarations.push(model("Third", 40));
    retain_nominal_tables(&mut program, &module)?;
    let grown = product_nominal_table(&mut program, &[], Some("products"));
    assert_eq!(described_rows(&program), (3, 0, 0));
    let mut fresh_program =
        synthetic_program_context(&module, Arc::new(constructor_index(&module.nominal_declarations)));
    let fresh = product_nominal_table(&mut fresh_program, &[], Some("products"));
    assert!(
        same_table(&grown, &fresh),
        "a grown table differs from one described afresh"
    );
    assert!(first.models.len() == 2 && grown.models.len() == 3);
    assert!(first.models[..] == grown.models[..2]);
    // A request without plan products describes nothing.
    let mut unproduced = synthetic_program_context(&module, Arc::new(constructor_index(&module.nominal_declarations)));
    assert!(product_nominal_table(&mut unproduced, &[], None).models.is_empty());
    assert_eq!(described_rows(&unproduced), (0, 0, 0));
    Ok(())
}
