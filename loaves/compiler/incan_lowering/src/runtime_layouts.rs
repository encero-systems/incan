//! Retain the checked runtime zip result as the builtin iterator's consumer-owned storage.

use incan_mir_plan_native::{EnumDeclaration, PlanType, ZipCarrier};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::{CanonicalSymbolId, IncanType};

use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::runtime_calls::runtime_body;
use crate::symbols::model_symbol;
use crate::tuples::tuple_element;
use crate::unions::collect_unions;

/// Select zip storage from its published result identity and that declaration's closed carriers.
pub fn runtime_zip_carriers(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<ZipCarrier>, String> {
    let Some(body) = runtime_body(module, "incan_native_runtime::zip_lists")? else {
        return Ok(Vec::new());
    };
    let owner = zip_result_identity(&body.return_type, &body)?;
    closed_zip_carriers(constructors, module, &owner, intrinsics)
}

/// Require the zip declaration's checked nominal result and its exact published owner binding.
fn zip_result_identity(result: &IncanType, body: &Body) -> Result<CanonicalSymbolId, String> {
    match result {
        IncanType::Generic { base, args } => {
            if args.len() != 2 {
                return Err("invalid governed zip result arity".into());
            }
            body.named_type_identities
                .get(base)
                .cloned()
                .ok_or_else(|| "unretained governed zip result identity".to_owned())
        }
        _ => Err("unsupported governed zip result declaration".into()),
    }
}

/// Associate builtin element layouts only with closed instances of the source result's canonical declaration.
fn closed_zip_carriers(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    owner: &CanonicalSymbolId,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<ZipCarrier>, String> {
    let mut carriers = Vec::new();
    let unions = collect_unions(module);
    let nominals = &module.nominal_declarations;
    let enums = &module.enum_declarations;
    for (index, declaration) in nominals.iter().enumerate() {
        if declaration.canonical != *owner || declaration.native_type_arguments.len() != 2 {
            continue;
        }
        let left = &declaration.native_type_arguments[0];
        let right = &declaration.native_type_arguments[1];
        let first = native_source_type(constructors, nominals, enums, &unions, left, intrinsics, "")?;
        let second = native_source_type(constructors, nominals, enums, &unions, right, intrinsics, "")?;
        carriers.push(ZipCarrier {
            left: tuple_element(&first, left)?,
            right: tuple_element(&second, right)?,
            target: PlanType::Model(index as i64, model_symbol(&declaration.name)),
        });
    }
    Ok(carriers)
}
