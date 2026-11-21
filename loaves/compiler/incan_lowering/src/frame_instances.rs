//! Closed generic instances: their frames, native layouts and the plan inventories they extend.

use std::collections::BTreeMap;

use incan_mir_plan_native::{
    Carrier, EnumDeclaration, ExternalFunction, Function, PlanType, StaticCell, carrier_position,
};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::BodyIrModule;

use crate::decls::{library_enum_item, library_item};
use crate::instance_products::InstanceProducts;
use crate::lowering::Inventories;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::NominalInstance;
use crate::pending::untranslated;
use crate::program_context::ProgramContext;
use crate::runtime_requirements::RuntimeRetention;
use crate::type_parameters::Instance;

/// Drain checked instances through an explicit mutable loan and append native layouts in place.
#[allow(clippy::too_many_arguments)]
pub fn lower_instances(
    template_constructors: &ConstructorIndex,
    context: &mut BodyIrModule,
    templates: &BodyIrModule,
    program: &mut ProgramContext,
    instances: &mut Vec<Instance>,
    unions: &[IncanType],
    sources: &[String],
    files: &[String],
    owners: &[String],
    inventories: &mut Inventories,
    intrinsics: &mut Vec<EnumDeclaration>,
    nominal_instances: &mut Vec<NominalInstance>,
    cells: &mut Vec<StaticCell>,
    products: Option<&InstanceProducts>,
) -> Result<RuntimeRetention, String> {
    let _ = (
        template_constructors,
        templates,
        program,
        unions,
        sources,
        files,
        intrinsics,
        nominal_instances,
        cells,
    );
    let _ = products;
    let governed_runtime = !context.runtime_callable_bindings.is_empty()
        || !context.runtime_binding_scopes.is_empty()
        || governed_runtime_module(owners);
    if governed_runtime
        && (!inventories.externals.is_empty() || !inventories.functions.is_empty() || !instances.is_empty())
    {
        return Err(untranslated("runtime_calls::retain_runtime_instances"));
    }
    if !instances.is_empty() {
        return Err(untranslated("frame_instances::lower_instance_frame"));
    }
    Ok(RuntimeRetention::Complete)
}

/// Whether the unit lowers the runtime itself, which governs its own instances.
fn governed_runtime_module(owners: &[String]) -> bool {
    owners
        .iter()
        .any(|owner| owner.starts_with("pub::incan_native_runtime::"))
}

/// Positions of one plan function list by symbol; each lookup first indexes the rows appended since the last.
#[derive(Default)]
pub struct FunctionNames {
    pub positions: BTreeMap<String, Vec<usize>>,
    pub indexed: usize,
}

pub fn function_names() -> FunctionNames {
    FunctionNames::default()
}

/// Index the rows appended since the last lookup, then return this symbol's positions in list order.
pub fn named_functions(names: &mut FunctionNames, functions: &[Function], name: &str) -> Vec<usize> {
    while names.indexed < functions.len() {
        names
            .positions
            .entry(functions[names.indexed].name.clone())
            .or_default()
            .push(names.indexed);
        names.indexed += 1;
    }
    names.positions.get(name).cloned().unwrap_or_default()
}

/// Add closure bodies and constructors once; two functions sharing one item's forwarding environment agree on it.
pub fn append_synthesized(
    functions: &mut Vec<Function>,
    names: &mut FunctionNames,
    additions: Vec<Function>,
) -> Result<(), String> {
    for addition in additions {
        let mut present = false;
        for position in named_functions(names, functions, &addition.name) {
            if functions[position] != addition {
                return Err(format!(
                    "invalid Body IR synthesized function collision {}",
                    addition.name
                ));
            }
            present = true;
        }
        if !present {
            functions.push(addition);
        }
    }
    Ok(())
}

/// Retain one exact signature per native path and instantiation; report both checked signatures on conflict.
pub fn merge_externals(externals: &mut Vec<ExternalFunction>, additions: Vec<ExternalFunction>) -> Result<(), String> {
    for external in additions {
        let mut found = false;
        for retained in externals.iter() {
            if retained.path == external.path && retained.type_arguments == external.type_arguments {
                if retained.parameters != external.parameters || retained.return_type != external.return_type {
                    return Err(conflicting_external_signatures(retained, &external));
                }
                found = true;
            }
        }
        if !found {
            externals.push(external);
        }
    }
    Ok(())
}

/// Positions of each native path's signatures in an externals list that only grows while the index is in use.
#[derive(Default)]
pub struct ExternalIndex {
    pub positions: BTreeMap<String, Vec<usize>>,
    pub indexed: usize,
}

pub fn external_index() -> ExternalIndex {
    ExternalIndex::default()
}

/// Merge as `merge_externals` does, finding a path's retained signatures by key instead of scanning every entry.
pub fn merge_indexed(
    externals: &mut Vec<ExternalFunction>,
    index: &mut ExternalIndex,
    additions: Vec<ExternalFunction>,
) -> Result<(), String> {
    for external in additions {
        while index.indexed < externals.len() {
            index
                .positions
                .entry(externals[index.indexed].path.clone())
                .or_default()
                .push(index.indexed);
            index.indexed += 1;
        }
        let mut found = false;
        if let Some(positions) = index.positions.get(&external.path) {
            for position in positions {
                let retained = &externals[*position];
                if retained.type_arguments == external.type_arguments {
                    if retained.parameters != external.parameters || retained.return_type != external.return_type {
                        return Err(conflicting_external_signatures(retained, &external));
                    }
                    found = true;
                }
            }
        }
        if !found {
            externals.push(external);
        }
    }
    Ok(())
}

/// Keep each conflicting checked signature and source location in the native merge refusal.
fn conflicting_external_signatures(retained: &ExternalFunction, external: &ExternalFunction) -> String {
    format!(
        "unsupported Body IR conflicting native call signatures for {} with {:?}: ({:?}) -> {:?} at {}:{}:{}; ({:?}) -> {:?} at {}:{}:{}",
        external.path,
        external.type_arguments,
        retained.parameters,
        retained.return_type,
        retained.span.file,
        retained.span.line,
        retained.span.column,
        external.parameters,
        external.return_type,
        external.span.file,
        external.span.line,
        external.span.column
    )
}

/// Report whether a native main's result is a `Result` with a unit success, which Rust's entry point reports itself.
///
/// The error side must be text or a type an SDK crate owns: legacy's entry point reports an error through its Debug,
/// which both have.
pub fn entry_result(module: &BodyIrModule, ty: &PlanType, intrinsics: &[EnumDeclaration]) -> bool {
    let PlanType::Enum(owner, _) = ty else {
        return false;
    };
    let index = carrier_position(*owner);
    if index < 0 || index as usize >= intrinsics.len() {
        return false;
    }
    let declaration = &intrinsics[index as usize];
    if declaration.carrier != Carrier::Result || declaration.variants.len() != 2 {
        return false;
    }
    let success = &declaration.variants[0].fields;
    let failure = &declaration.variants[1].fields;
    if failure.len() != 1 || (!success.is_empty() && success.as_slice() != [PlanType::Unit]) {
        return false;
    }
    entry_error(module, &failure[0])
}

/// Report whether an entry result's error is text or an SDK crate's model or enum.
fn entry_error(module: &BodyIrModule, ty: &PlanType) -> bool {
    match ty {
        PlanType::String => true,
        PlanType::Model(index, _) => module
            .nominal_declarations
            .get(*index as usize)
            .is_some_and(|declaration| !library_item(declaration).is_empty()),
        PlanType::Enum(index, _) => module
            .enum_declarations
            .get(*index as usize)
            .is_some_and(|declaration| !library_enum_item(declaration).is_empty()),
        _ => false,
    }
}
