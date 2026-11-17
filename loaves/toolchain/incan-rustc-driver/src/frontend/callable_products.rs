//! Native source-item products selected by Oven's source-independent semantic context.
//!
//! A module product contains independent checked bodies, not a replacement checking route. Its frontend owner
//! admits only scalar expressions with complete local-call witnesses and validates exact current locators. Imported
//! calls and other bodies fall back to ordinary source checking; a missing, corrupt or concurrently replaced product
//! is an ordinary miss.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use incan_driver::session::{CallableBodySource, CompilationSession};
use incan_driver::unit::UnitDescription;
use incan_frontend::declaration_interface::{CallableBodyInputs, CallableBodyProducts, ValidatedCallableBodies};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::typechecker::TypeChecker;
use incan_semantics_core::body_ir::BodyIrModule;
use sha2::{Digest, Sha256};

/// One invocation's authenticated store placement and accepted declaration inputs, without retaining checkers.
pub(super) struct CallableProducts {
    directory: Option<PathBuf>,
    package: String,
    inputs: BTreeMap<PathBuf, CallableBodyInputs>,
}

#[derive(Debug)]
struct StoredCallableBodies {
    directory: PathBuf,
    package: String,
}

impl CallableBodySource for StoredCallableBodies {
    fn select(&self, module: &ParsedModule) -> Option<ValidatedCallableBodies> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::select_callable_products",
            file = %module.file_path.display())
        .entered();
        let path = self.directory.join(module_key(module));
        let bytes = std::fs::read(path).ok()?;
        let products = CallableBodyProducts::from_bytes(&bytes).ok()?;
        let selection = products
            .select(&module.ast, &module.source, &self.package, &module.path_segments)
            .ok()?;
        if selection.is_empty() {
            return None;
        }
        tracing::debug!(target: "incan::invocation", file = %module.file_path.display(),
            retained_bodies = selection.len(), "selected callable body products");
        Some(selection)
    }
}

impl CallableProducts {
    /// Install the private scalar source when Oven supplied its authenticated compilation context.
    pub(super) fn prepare(unit: &UnitDescription, session: &mut CompilationSession) -> Self {
        let directory = product_directory(unit);
        if let Some(directory) = &directory {
            session.set_callable_body_source(std::sync::Arc::new(StoredCallableBodies {
                directory: directory.clone(),
                package: unit.package.clone(),
            }));
        }
        Self {
            directory,
            package: unit.package.clone(),
            inputs: BTreeMap::new(),
        }
    }

    /// Retain accepted inputs from the same checking pass used for publication. Unsupported source shapes simply
    /// decline capture, preserving their ordinary diagnostics and full compilation behavior.
    pub(super) fn observe(&mut self, module: &ParsedModule, checker: &TypeChecker) {
        if self.directory.is_none() {
            return;
        }
        match CallableBodyInputs::from_checked_for_unit(&module.ast, &module.source, checker, &self.package) {
            Ok(inputs) => {
                self.inputs.insert(module.file_path.clone(), inputs);
            }
            Err(reason) => tracing::debug!(target: "incan::invocation", file = %module.file_path.display(),
                %reason, "callable product capture refused"),
        }
    }

    /// Publish independently validated products from the actual completed Body IR pass. A cache write failure is
    /// observational only and cannot change source acceptance or substitute another native implementation.
    pub(super) fn publish(
        mut self,
        modules: &[ParsedModule],
        bodies: &[BodyIrModule],
    ) -> Vec<(incan_semantics_core::CompilerNodeId, String)> {
        let Some(directory) = self.directory else {
            return Vec::new();
        };
        let mut admitted = Vec::new();
        for (module, body) in modules.iter().zip(bodies) {
            let Some(inputs) = self.inputs.remove(&module.file_path) else {
                continue;
            };
            let result = inputs
                .finish(body)
                .map_err(|error| error.to_string())
                .and_then(|products| {
                    admitted.extend(products.native_inputs().map_err(|error| error.to_string())?);
                    let path = directory.join(module_key(module));
                    publish(&path, products)
                        .map_err(|error| format!("cannot publish callable products {}: {error}", path.display()))
                });
            if let Err(error) = result {
                tracing::debug!(target: "incan::invocation", %error, "callable products were not retained");
            }
        }
        admitted
    }
}

/// Use only Oven's explicit item directory and well-formed context digest. Library, program and test units share the
/// same source-independent semantic key; current module inventory and exact declaration source/provenance are still
/// independently checked by the frontend before any body can skip work.
fn product_directory(unit: &UnitDescription) -> Option<PathBuf> {
    let context = unit.checked_context.as_deref()?;
    if context.len() != 64 || !context.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    unit.checked_products
        .as_ref()
        .map(|directory| directory.join(incan_frontend::checker_records::GRAPH_BODY_RECORD_PROFILE))
}

/// Scope native function products to Oven's complete semantic generation and current native profile. Lowering
/// additionally keys the exact checked body, overflow policy and current diagnostic/frame locations.
pub(super) fn native_directory(unit: &UnitDescription) -> Option<String> {
    product_directory(unit).map(|directory| directory.join(&unit.profile).to_string_lossy().into_owned())
}

/// Distinguish source modules through their complete producer path, without parsing any generated carrier name.
fn module_key(module: &ParsedModule) -> String {
    let mut digest = Sha256::new();
    for segment in &module.path_segments {
        digest.update(segment.len().to_le_bytes());
        digest.update(segment.as_bytes());
    }
    format!("{}.incnitems", hex::encode(digest.finalize()))
}

/// Verify the complete private transport before atomically replacing one module's independent item inventory.
/// A concurrent writer publishes another complete generation; every reader still validates its own source input.
fn publish(path: &std::path::Path, products: CallableBodyProducts) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = products.to_bytes()?;
    if CallableBodyProducts::from_bytes(&bytes)?.to_bytes()? != bytes {
        return Err("callable products lost facts in their private round trip".into());
    }
    std::fs::create_dir_all(path.parent().ok_or("callable products have no owning directory")?)?;
    let temporary = path.with_extension(format!("{}.pending", std::process::id()));
    let mut output = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    output.write_all(&bytes)?;
    output.flush()?;
    drop(output);
    std::fs::rename(temporary, path)?;
    Ok(())
}
