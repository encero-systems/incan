//! One compilation's record-backed native definitions and stable rustc definition index.

use super::*;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::DefId;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

const NATIVE_CGU_PARTITIONS: usize = 16;

/// The exact record input attached to one compiler definition.
#[derive(Clone)]
pub(crate) struct DefinitionInput {
    identity: String,
    fingerprint: String,
}

impl DefinitionInput {
    pub(crate) fn identity(&self) -> &str {
        &self.identity
    }

    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn partition(&self) -> usize {
        stable_partition(&self.identity)
    }
}

#[derive(Deserialize)]
struct FunctionEvent {
    schema: u64,
    identity: String,
    key: String,
    function: String,
    product: String,
    role: Option<String>,
}

#[derive(Deserialize)]
struct FunctionProduct {
    schema: u64,
    inputs: Vec<String>,
    function: String,
    digest: String,
}

#[derive(Deserialize)]
struct NativeInterface {
    schema: String,
    plan: String,
}

/// A record-backed plan reader. Missing record input keeps ordinary compilation available but makes the unit
/// ineligible for native incremental reuse.
pub(crate) struct PlanReader {
    plan: Arc<Plan>,
    by_name: HashMap<String, usize>,
    function_inputs: HashMap<String, DefinitionInput>,
    alias_inputs: HashMap<String, DefinitionInput>,
    named_items: OnceLock<HashMap<String, DefId>>,
}

impl PlanReader {
    /// Authenticate the native-interface and every per-function product before exposing query answers.
    pub(super) fn new(plan: Arc<Plan>, report: Option<&Path>) -> Result<Self, String> {
        let mut reader = Self::with_inputs(plan, HashMap::new(), HashMap::new());
        let Some(report) = report else {
            return Ok(reader);
        };
        let bytes = match std::fs::read(report.join("native-interface.json")) {
            Ok(bytes) => bytes,
            // A build without record input stays an ordinary compilation, ineligible for native incremental reuse.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(reader),
            Err(error) => return Err(format!("cannot read native-interface record: {error}")),
        };
        let interface: NativeInterface = serde_json::from_slice(&bytes)
            .map_err(|error| format!("cannot decode native-interface record: {error}"))?;
        if interface.schema != "incan.native-interface/1" {
            return Err("native-interface record has an unsupported schema".into());
        }
        if interface.plan.is_empty() {
            return Err("native-interface record has no encoded plan".into());
        }
        reader.function_inputs = Self::read_function_inputs(report, &reader.plan)?;
        reader.alias_inputs = Self::layout_inputs(&reader.plan)?;
        Ok(reader)
    }

    fn with_inputs(
        plan: Arc<Plan>,
        function_inputs: HashMap<String, DefinitionInput>,
        alias_inputs: HashMap<String, DefinitionInput>,
    ) -> Self {
        let mut by_name = HashMap::new();
        for (position, function) in plan.functions.iter().enumerate() {
            by_name.entry(function.name.to_string()).or_insert(position);
        }
        Self {
            plan,
            by_name,
            function_inputs,
            alias_inputs,
            named_items: OnceLock::new(),
        }
    }

    /// Whether every active function's authenticated record input was read, so native incremental reuse is sound.
    pub(super) fn has_record_inputs(&self) -> bool {
        !self.function_inputs.is_empty()
    }

    /// Keep authenticated definition inputs when native metadata refines only invocation-local plan facts.
    pub(super) fn with_plan(&self, plan: Arc<Plan>) -> Self {
        Self::with_inputs(plan, self.function_inputs.clone(), self.alias_inputs.clone())
    }

    fn read_function_inputs(report: &Path, plan: &Plan) -> Result<HashMap<String, DefinitionInput>, String> {
        let mut paths = std::fs::read_dir(report)
            .map_err(|error| format!("cannot read native plan report: {error}"))?
            .map(|entry| entry.map(|value| value.path()).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        paths.sort();
        let mut inputs = HashMap::new();
        for path in paths {
            if path.extension().and_then(|value| value.to_str()) != Some("event") {
                continue;
            }
            let event: FunctionEvent = serde_json::from_slice(
                &std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?,
            )
            .map_err(|error| format!("cannot decode {}: {error}", path.display()))?;
            if event.role.is_some() {
                continue;
            }
            if event.schema != 2 {
                return Err(format!(
                    "native function event for {} has an unsupported schema",
                    event.function
                ));
            }
            let product: FunctionProduct = serde_json::from_str(&event.product)
                .map_err(|error| format!("cannot decode native function product for {}: {error}", event.function))?;
            if product.schema != 3 {
                return Err(format!(
                    "native function product for {} has an unsupported schema",
                    event.function
                ));
            }
            let key = incan_semantics_core::semantic_digest::semantic_digest(&product.inputs)
                .map_err(|error| format!("cannot fingerprint native function inputs: {error}"))?;
            if key.strip_prefix("sha256:") != Some(event.key.as_str()) {
                return Err(format!("native function event key differs for {}", event.function));
            }
            let digest = incan_semantics_core::semantic_digest::semantic_digest(&vec![product.function.clone()])
                .map_err(|error| format!("cannot fingerprint native function product: {error}"))?;
            if digest != product.digest {
                return Err(format!("native function product digest differs for {}", event.function));
            }
            let fingerprint = incan_semantics_core::semantic_digest::semantic_digest(&vec![
                event.key.clone(),
                product.digest.clone(),
            ])
            .map_err(|error| format!("cannot fingerprint native function record: {error}"))?;
            if !plan.functions.iter().any(|value| value.name == event.function) {
                return Err(format!(
                    "native function product is outside the active plan for {}",
                    event.function
                ));
            }
            if inputs
                .insert(
                    event.function.clone(),
                    DefinitionInput {
                        identity: event.identity,
                        fingerprint: fingerprint.trim_start_matches("sha256:").to_owned(),
                    },
                )
                .is_some()
            {
                return Err(format!("native plan report repeats function {}", event.function));
            }
        }
        if inputs.len() != plan.functions.len() || plan.functions.iter().any(|value| !inputs.contains_key(&value.name))
        {
            return Err("native plan report does not cover every active function".into());
        }
        Ok(inputs)
    }

    fn layout_inputs(plan: &Plan) -> Result<HashMap<String, DefinitionInput>, String> {
        let mut inputs = HashMap::new();
        for layout in &plan.models {
            let fingerprint = incan_semantics_core::semantic_digest::semantic_digest(&format!("{layout:?}"))
                .map_err(|error| format!("cannot fingerprint native layout input: {error}"))?;
            inputs.insert(
                layout.name.clone(),
                DefinitionInput {
                    identity: layout.name.clone(),
                    fingerprint: fingerprint.trim_start_matches("sha256:").to_owned(),
                },
            );
        }
        for layout in &plan.enums {
            let fingerprint = incan_semantics_core::semantic_digest::semantic_digest(&format!("{layout:?}"))
                .map_err(|error| format!("cannot fingerprint native layout input: {error}"))?;
            inputs.insert(
                layout.name.clone(),
                DefinitionInput {
                    identity: layout.name.clone(),
                    fingerprint: fingerprint.trim_start_matches("sha256:").to_owned(),
                },
            );
        }
        Ok(inputs)
    }

    pub(super) fn function(&self, name: &str) -> Option<&crate::plan::Function> {
        self.by_name.get(name).map(|position| &self.plan.functions[*position])
    }

    pub(super) fn function_input(&self, name: &str) -> Option<&DefinitionInput> {
        self.function_inputs.get(name)
    }

    pub(super) fn alias_input(&self, name: &str) -> Option<&DefinitionInput> {
        self.alias_inputs.get(name)
    }

    pub(super) fn function_partition(&self, name: &str) -> usize {
        self.function_inputs
            .get(name)
            .map(DefinitionInput::partition)
            .unwrap_or_else(|| stable_partition(name))
    }

    /// The first source-local function item named `name` among the session's crate items.
    pub(crate) fn item(&self, tcx: TyCtxt<'_>, name: &str) -> Option<DefId> {
        self.named_items
            .get_or_init(|| {
                let mut items = HashMap::new();
                for item in tcx.hir_crate_items(()).free_items() {
                    let definition = item.owner_id.to_def_id();
                    if tcx.def_kind(definition) == DefKind::Fn
                        && let Some(symbol) = tcx.opt_item_name(definition)
                    {
                        items.entry(symbol.as_str().to_owned()).or_insert(definition);
                    }
                }
                items
            })
            .get(name)
            .copied()
    }

    /// Emit deterministic test evidence only when explicitly requested by the caller.
    pub(super) fn write_fingerprint_report(&self) -> Result<(), DriverError> {
        let Some(path) = std::env::var_os("INCAN_NATIVE_FINGERPRINT_REPORT") else {
            return Ok(());
        };
        let mut rows = self
            .function_inputs
            .iter()
            .map(|(name, input)| {
                format!(
                    "{}\t{}\t{}\t{}",
                    input.identity(),
                    name,
                    input.fingerprint(),
                    input.partition()
                )
            })
            .collect::<Vec<_>>();
        rows.sort();
        std::fs::write(path, format!("{}\n", rows.join("\n")))
            .map_err(|error| DriverError::Invocation(format!("cannot write native fingerprint report: {error}")))
    }
}

/// A fixed, deterministic partition independent of declaration count and source-file ordering.
fn stable_partition(value: &str) -> usize {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash as usize % NATIVE_CGU_PARTITIONS
}

impl std::ops::Deref for PlanReader {
    type Target = Plan;

    fn deref(&self) -> &Plan {
        &self.plan
    }
}
