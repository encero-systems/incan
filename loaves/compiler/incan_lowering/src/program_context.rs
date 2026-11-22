//! Immutable program mappings shared by ordinary, instantiated, and lifted function frames.

use std::collections::BTreeSet;
use std::sync::Arc;

use incan_mir_plan_native::CalleeKind;
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{
    BodyIrModule, DecoratedBinding, EnumDeclaration, NominalDeclaration, StaticDeclaration, ValueEnumDeclaration,
};

use crate::body_index::{BodyIndex, empty_body_index, extend_body_index};
use crate::native_type_identity::{ConstructorIndex, extend_constructor_index};
use crate::nominal_records::DescribedNominals;

/// One validated source body or SDK item rendering a retained nominal through a shared receiver.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayFunction {
    pub owner: i64,
    pub callee: CalleeKind,
}

/// Checked tables, body positions, arithmetic policy and diagnostic policy shared across frames.
#[derive(Clone)]
pub struct ProgramContext {
    pub body_indices: BodyIndex,
    /// This immutable copy keeps the specialized module's order, so the checked index selects the same facts.
    pub display_functions: Arc<Vec<DisplayFunction>>,
    pub statics: Arc<Vec<StaticDeclaration>>,
    pub comparison_functions: Arc<Vec<Vec<String>>>,
    pub union_types: Arc<BTreeSet<IncanType>>,
    /// Layouts are immutable after specialization; frames retain shared owners rather than copying checked tables.
    pub nominals: Arc<Vec<NominalDeclaration>>,
    /// Positions borrow exactly the nominal inventory published beside them.
    pub constructors: Arc<ConstructorIndex>,
    pub enums: Arc<Vec<EnumDeclaration>>,
    pub value_enums: Arc<Vec<ValueEnumDeclaration>>,
    pub decorated_bindings: Arc<Vec<DecoratedBinding>>,
    /// Lifted functions share their unit's ordinary integer policy along with its checked declaration tables.
    pub overflow_checks: bool,
    /// The diagnostic policy, observed once per context rather than for every frame.
    pub observe_work: bool,
    /// The rows of these tables a product nominal table has described, shared by the contexts that copy this one.
    pub described: Arc<DescribedNominals>,
}

/// Module storage but no dispatch mappings for external-only synthetic frames.
pub fn synthetic_program_context(module: &BodyIrModule, constructors: Arc<ConstructorIndex>) -> ProgramContext {
    ProgramContext {
        body_indices: empty_body_index(),
        display_functions: Arc::new(Vec::new()),
        statics: Arc::new(module.static_declarations.clone()),
        comparison_functions: Arc::new(Vec::new()),
        union_types: Arc::new(BTreeSet::new()),
        nominals: Arc::new(module.nominal_declarations.clone()),
        constructors,
        enums: Arc::new(module.enum_declarations.clone()),
        value_enums: Arc::new(module.value_enum_declarations.clone()),
        decorated_bindings: Arc::new(Vec::new()),
        overflow_checks: true,
        observe_work: std::env::var_os("INCAN_NATIVE_TIMING").is_some(),
        described: Arc::default(),
    }
}

/// Publish each appended nominal-table generation before its closed frames borrow the new layouts.
///
/// Instance retention only appends to the module's table, so the shared table and its constructor index grow in
/// place by the new rows. A table an earlier context still shares is copied once, on its first write.
pub fn retain_nominal_tables(program: &mut ProgramContext, module: &BodyIrModule) -> Result<(), String> {
    let retained = program.nominals.len();
    let current = module.nominal_declarations.len();
    if retained == current {
        return Ok(());
    }
    if retained > current || program.constructors.count != retained {
        return Err("invalid Body IR nominal table generation".into());
    }
    Arc::make_mut(&mut program.nominals).extend(module.nominal_declarations[retained..].iter().cloned());
    extend_constructor_index(Arc::make_mut(&mut program.constructors), &module.nominal_declarations);
    Ok(())
}

/// Extend retained program lookup and storage generations after runtime resolution without rebuilding dispatch roots.
pub fn extend_program_context(
    program: &mut ProgramContext,
    module: &BodyIrModule,
    constructors: Arc<ConstructorIndex>,
    unions: &[IncanType],
) -> Result<(), String> {
    if program.statics.len() > module.static_declarations.len()
        || program.nominals.len() > module.nominal_declarations.len()
        || program.enums.len() > module.enum_declarations.len()
        || program.value_enums.len() > module.value_enum_declarations.len()
        || program.decorated_bindings.len() > module.decorated_bindings.len()
    {
        return Err("invalid Body IR program context generation".into());
    }
    extend_body_index(&mut program.body_indices, module)?;
    let statics = program.statics.len();
    Arc::make_mut(&mut program.statics).extend(module.static_declarations[statics..].iter().cloned());
    let nominals = program.nominals.len();
    Arc::make_mut(&mut program.nominals).extend(module.nominal_declarations[nominals..].iter().cloned());
    let enums = program.enums.len();
    Arc::make_mut(&mut program.enums).extend(module.enum_declarations[enums..].iter().cloned());
    let value_enums = program.value_enums.len();
    Arc::make_mut(&mut program.value_enums).extend(module.value_enum_declarations[value_enums..].iter().cloned());
    let bindings = program.decorated_bindings.len();
    Arc::make_mut(&mut program.decorated_bindings).extend(module.decorated_bindings[bindings..].iter().cloned());
    for ty in unions {
        Arc::make_mut(&mut program.union_types).insert(ty.clone());
    }
    program.constructors = constructors;
    Ok(())
}
