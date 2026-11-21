//! A module's admitted plan record: the page a worker restores its functions from, and the note of what it restored.
//!
//! Restoring a module from its page, and writing the note of what a records request restored, refuse by name until
//! they are in Rust; a request without a producer profile or without a page admits nothing, as in the reference.

use std::path::{Path, PathBuf};

use internal_models_native::plan_module_records::{POSITIONED_MODULE_PAGE, REPOSITION_MODULE_PAGE};

use crate::pending::untranslated;

/// Where Bakery places the plan record of an admitted module: named by the module, never by a product key.
pub fn module_page(products: &str, module: &str) -> PathBuf {
    let basename = hex::encode(module.as_bytes());
    let placed = Path::new(products).join(format!("{basename}{POSITIONED_MODULE_PAGE}"));
    if placed.is_file() {
        return placed;
    }
    Path::new(products).join(format!("{basename}{REPOSITION_MODULE_PAGE}"))
}

/// Require that the module admits nothing: no product directory, no producer profile or no page.
///
/// Bakery writes the page only for a module whose every witness still validates; restoring its bodies from it is the
/// step `plan_modules::admitted_page` names.
pub fn require_unadmitted(products: Option<&str>, module: &str, generation: Option<&str>) -> Result<(), String> {
    let (Some(directory), Some(_)) = (products, generation) else {
        return Ok(());
    };
    if module_page(directory, module).is_file() {
        return Err(untranslated("plan_modules::admitted_page"));
    }
    Ok(())
}

/// Report what a records request restored for one module; an ordinary keyed request records no note.
pub fn report_admitted(report: Option<&str>, products: Option<&str>) -> Result<(), String> {
    match (report, products) {
        (Some(_), Some(_)) => Err(untranslated("plan_modules::report_admitted")),
        _ => Ok(()),
    }
}
