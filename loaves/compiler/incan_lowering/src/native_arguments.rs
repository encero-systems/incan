//! Checked native storage uses that require source nominal ABI implementations.

use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::{IncanType, SymbolOrigin};

use crate::decls::library_item;
use crate::nominal_instances::instance_arguments;

/// Collect, once per module, the source carrier names found in foreign Rust storage arguments.
pub fn native_argument_names(module: &BodyIrModule) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for declaration in &module.nominal_declarations {
        if library_item(declaration).is_empty() || !matches!(declaration.canonical.origin, SymbolOrigin::RustCrate(_)) {
            continue;
        }
        let mut arguments = declaration.native_type_arguments.clone();
        if arguments.is_empty() {
            arguments = instance_arguments(declaration)?;
        }
        for argument in &arguments {
            collect_names(argument, &mut names);
        }
    }
    Ok(names)
}

/// Record each named argument in a checked argument tree once.
fn collect_names(ty: &IncanType, names: &mut Vec<String>) {
    match ty {
        IncanType::Named(value) => {
            if !names.contains(value) {
                names.push(value.clone());
            }
        }
        IncanType::Generic { args, .. } => {
            for argument in args {
                collect_names(argument, names);
            }
        }
        _ => {}
    }
}
