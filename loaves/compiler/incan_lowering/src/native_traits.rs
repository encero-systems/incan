//! Rust trait implementations a source class fills with its own methods; rustc proves each implementation.

use incan_mir_plan_native::{NativeTraitAdoption, NativeTraitMethod};
use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use crate::classes::function_name;

/// The Rust traits one class adopts, each with the planned method filling each trait method the class declares.
pub fn native_trait_adoptions(module: &BodyIrModule, owner: usize) -> Result<Vec<NativeTraitAdoption>, String> {
    let declaration = &module.nominal_declarations[owner];
    let mut adoptions = Vec::new();
    for adoption in &declaration.trait_adoptions {
        let SymbolOrigin::RustCrate(parts) = &adoption.trait_identity.origin else {
            continue;
        };
        let path = parts.join("::");
        let Some(slots) = &adoption.methods else {
            return Err("unsupported Body IR Rust trait adoption without its method slots".into());
        };
        let mut methods = Vec::new();
        for slot in slots {
            if let Some(function) = filling_function(module, &declaration.canonical, slot)? {
                methods.push(NativeTraitMethod {
                    name: slot.declaration_name.clone(),
                    function,
                });
            }
        }
        adoptions.push(NativeTraitAdoption { path, methods });
    }
    Ok(adoptions)
}

/// The planned function of the one concrete body filling this owner's slot; none keeps the trait's default.
fn filling_function(
    module: &BodyIrModule,
    owner: &CanonicalSymbolId,
    slot: &CanonicalSymbolId,
) -> Result<Option<String>, String> {
    for implementation in &module.trait_implementations {
        if implementation.owner != *owner || implementation.method != *slot {
            continue;
        }
        let mut selected = String::new();
        for body in &module.bodies {
            if body.canonical.as_ref() != Some(&implementation.implementation) {
                continue;
            }
            if !selected.is_empty() || !body.type_parameters.is_empty() {
                return Err("unsupported Body IR ambiguous or generic Rust trait method".into());
            }
            selected = function_name(module, body);
        }
        if selected.is_empty() {
            return Err("unsupported Body IR Rust trait method without its body".into());
        }
        return Ok(Some(selected));
    }
    Ok(None)
}
