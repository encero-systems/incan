//! One compilation's record-backed native definitions and stable rustc definition index.

use super::*;
use incan_mir_plan_native::AdmittedDefinition;
use rustc_hir::def::DefKind;
use rustc_hir::def_id::DefId;
use std::collections::HashMap;
use std::sync::OnceLock;

const NATIVE_CGU_PARTITIONS: usize = 16;

/// The exact record input attached to one compiler definition.
#[derive(Clone)]
pub(crate) struct DefinitionInput {
    /// The plan's name for the definition; it chooses the codegen unit and carries no source position.
    name: String,
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

    /// The codegen unit of the definition, from its plan name: a comment that shifts byte offsets, and with them the
    /// record identity, leaves every definition in the unit it was in, so equal plans compile to equal bytes.
    pub(crate) fn partition(&self) -> usize {
        stable_partition(&self.name)
    }
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
    pub(super) fn new(
        plan: Arc<Plan>,
        report: Option<&Path>,
        compact: Vec<AdmittedDefinition>,
    ) -> Result<Self, String> {
        let mut reader = Self::with_inputs(plan, HashMap::new(), HashMap::new());
        let compact = reader.index_compact_definitions(compact)?;
        let strict = !compact.is_empty();
        let Some(report) = report else {
            if strict {
                return Err("compact native definitions have no native report directory".into());
            }
            return Ok(reader);
        };
        match std::fs::read(report.join(incan_mir_plan_native::native_interface_filename())) {
            Ok(bytes) => {
                let interface = incan_mir_plan_native::decode_native_interface(&bytes)
                    .map_err(|error| format!("cannot decode native-interface record: {error}"))?;
                if interface.plan.is_empty() {
                    return Err("native-interface record has no encoded plan".into());
                }
                reader.function_inputs = reader.read_function_inputs(report, compact)?;
            }
            // Without the whole-unit interface, the report's events and definitions must name every active function;
            // otherwise the unit stays an ordinary compilation, ineligible for native incremental reuse.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match reader.read_function_inputs(report, compact) {
                    Ok(inputs) => reader.function_inputs = inputs,
                    Err(reason) if strict => return Err(reason),
                    Err(reason) => {
                        tracing::debug!(target: "incan::invocation", %reason, "native incremental reuse unavailable");
                        return Ok(reader);
                    }
                }
            }
            Err(error) => return Err(format!("cannot read native-interface record: {error}")),
        }
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

    /// Validate each authenticated compact definition against the existing indexed active plan exactly once.
    fn index_compact_definitions(
        &self,
        definitions: Vec<AdmittedDefinition>,
    ) -> Result<HashMap<String, DefinitionInput>, String> {
        let mut indexed = HashMap::with_capacity(definitions.len());
        for definition in definitions {
            let function = definition.function.clone();
            let input = self.admitted_definition_input(definition)?;
            if indexed.insert(function.clone(), input).is_some() {
                return Err(format!("compact admission repeats native function {function}"));
            }
        }
        Ok(indexed)
    }

    /// Read authenticated function events and admitted definitions, requiring coverage of the active plan.
    fn read_function_inputs(
        &self,
        report: &Path,
        compact: HashMap<String, DefinitionInput>,
    ) -> Result<HashMap<String, DefinitionInput>, String> {
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
            let event = incan_mir_plan_native::decode_lowering_event(
                &std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?,
            )
            .map_err(|error| format!("cannot decode {}: {error}", path.display()))?;
            if event.role == "entry" {
                continue;
            }
            let product = incan_mir_plan_native::decode_plan_function_record(&event.product)
                .map_err(|error| format!("cannot decode native function product for {}: {error}", event.function))?;
            let key = incan_semantics_core::semantic_digest::semantic_digest(&product.inputs)
                .map_err(|error| format!("cannot fingerprint native function inputs: {error}"))?;
            if key.strip_prefix("sha256:") != Some(event.key.as_str()) {
                return Err(format!("native function event key differs for {}", event.function));
            }
            let digest = incan_mir_plan_native::plan_function_digest(&product.function, &product.requests)
                .map_err(|error| format!("cannot fingerprint native function product: {error}"))?;
            if digest != product.digest {
                return Err(format!("native function product digest differs for {}", event.function));
            }
            let fingerprint = incan_semantics_core::semantic_digest::semantic_digest(&vec![
                event.key.clone(),
                product.digest.clone(),
            ])
            .map_err(|error| format!("cannot fingerprint native function record: {error}"))?;
            if !self.contains_active_function(&event.function) {
                return Err(format!(
                    "native function product is outside the active plan for {}",
                    event.function
                ));
            }
            if inputs
                .insert(
                    event.function.clone(),
                    DefinitionInput {
                        name: event.function.clone(),
                        identity: event.identity,
                        fingerprint: fingerprint.trim_start_matches("sha256:").to_owned(),
                    },
                )
                .is_some()
            {
                return Err(format!("native plan report repeats function {}", event.function));
            }
        }
        self.read_admitted_definitions(
            report,
            incan_mir_plan_native::admitted_definitions_filename(),
            &compact,
            &mut inputs,
        )?;
        self.read_admitted_definitions(
            report,
            incan_mir_plan_native::NATIVE_DEFINITIONS.to_owned(),
            &compact,
            &mut inputs,
        )?;
        for (function, input) in compact {
            inputs.entry(function).or_insert(input);
        }
        if inputs.len() != self.plan.functions.len()
            || self
                .plan
                .functions
                .iter()
                .any(|value| !inputs.contains_key(&value.name))
        {
            return Err("native plan report does not cover every active function".into());
        }
        Ok(inputs)
    }

    /// Add the definitions a report lists in `filename` for functions that reported no event: those restored from
    /// admitted module plan records, and the native definitions of helper frames and generated functions. A function
    /// that also reported an event keeps the input its event authenticated.
    fn read_admitted_definitions(
        &self,
        report: &Path,
        filename: String,
        compact: &HashMap<String, DefinitionInput>,
        inputs: &mut HashMap<String, DefinitionInput>,
    ) -> Result<(), String> {
        let path = report.join(filename);
        if !path.is_file() {
            return Ok(());
        }
        let definitions = incan_mir_plan_native::decode_admitted_definitions(
            &std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?,
        )
        .map_err(|error| format!("cannot decode {}: {error}", path.display()))?;
        for definition in definitions {
            if compact.contains_key(&definition.function) {
                return Err(format!(
                    "native plan report repeats compact function {} as a scalar definition",
                    definition.function
                ));
            }
            if inputs.contains_key(&definition.function) {
                continue;
            }
            let function = definition.function.clone();
            inputs.insert(function, self.admitted_definition_input(definition)?);
        }
        Ok(())
    }

    /// Bind a scalar or compact product to the same exact active function and native fingerprint.
    fn admitted_definition_input(&self, definition: AdmittedDefinition) -> Result<DefinitionInput, String> {
        if !self.contains_active_function(&definition.function) {
            return Err(format!(
                "admitted function product is outside the active plan for {}",
                definition.function
            ));
        }
        let fingerprint =
            incan_semantics_core::semantic_digest::semantic_digest(&vec![definition.key, definition.digest])
                .map_err(|error| format!("cannot fingerprint native function record: {error}"))?;
        Ok(DefinitionInput {
            name: definition.function,
            identity: definition.identity,
            fingerprint: fingerprint.trim_start_matches("sha256:").to_owned(),
        })
    }

    /// Check one definition against the existing active function index.
    fn contains_active_function(&self, name: &str) -> bool {
        #[cfg(test)]
        tests::record_active_function_probe();
        self.by_name.contains_key(name)
    }

    fn layout_inputs(plan: &Plan) -> Result<HashMap<String, DefinitionInput>, String> {
        let mut inputs = HashMap::new();
        for layout in &plan.models {
            let fingerprint = incan_semantics_core::semantic_digest::semantic_digest(&format!("{layout:?}"))
                .map_err(|error| format!("cannot fingerprint native layout input: {error}"))?;
            inputs.insert(
                layout.name.clone(),
                DefinitionInput {
                    name: layout.name.clone(),
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
                    name: layout.name.clone(),
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

#[cfg(test)]
mod tests {
    use super::PlanReader;
    use crate::plan::{Function, Plan, PlanType, Terminator, TerminatorKind};
    use incan_mir_plan_native::{
        AdmittedDefinition, BasicBlock, Local, NATIVE_DEFINITIONS, NativeInterface, admitted_definitions_filename,
        decode_lowering_event, decode_plan_function_record, decode_plan_module, decode_product,
        encode_admitted_definitions, encode_native_interface, encode_record_plan, native_interface_filename,
        source_span,
    };
    use std::sync::Arc;

    thread_local! {
        static ACTIVE_FUNCTION_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// Count each membership comparison on the native definition admission route.
    pub(super) fn record_active_function_probe() {
        ACTIVE_FUNCTION_PROBES.with(|probes| probes.set(probes.get() + 1));
    }

    fn plan(names: &[&str]) -> Plan {
        let span = source_span("reader.incn", 1, 1);
        let functions = names
            .iter()
            .map(|name| {
                Function::new(
                    (*name).into(),
                    Vec::new(),
                    PlanType::Unit,
                    Vec::new(),
                    Vec::new(),
                    span.clone(),
                )
            })
            .collect();
        Plan::new(Vec::new(), Vec::new(), functions, Vec::new(), span)
    }

    fn definitions(names: &[&str]) -> Result<Vec<u8>, String> {
        let listed: Vec<AdmittedDefinition> = names
            .iter()
            .map(|name| AdmittedDefinition {
                function: (*name).into(),
                identity: (*name).into(),
                key: "content".into(),
                digest: format!("sha256:{name}"),
            })
            .collect();
        encode_admitted_definitions(&listed)
    }

    /// A unit without a native-interface record still gives every function its record input when the report's native
    /// definitions name them all, so native incremental reuse can track each definition; a report naming only some
    /// keeps the unit an ordinary compilation.
    #[test]
    fn native_definitions_give_every_function_an_input_without_an_interface() -> Result<(), Box<dyn std::error::Error>>
    {
        let report = tempfile::tempdir()?;
        let names = ["main", "__incan_closure_main_0"];
        std::fs::write(report.path().join(NATIVE_DEFINITIONS), definitions(&names)?)?;
        ACTIVE_FUNCTION_PROBES.set(0);
        let reader = PlanReader::new(Arc::new(plan(&names)), Some(report.path()), Vec::new())?;
        assert_eq!(
            ACTIVE_FUNCTION_PROBES.replace(0),
            names.len(),
            "native admission must make one indexed membership probe per definition"
        );
        assert!(reader.has_record_inputs());
        assert!(names.iter().all(|name| reader.function_input(name).is_some()));
        std::fs::write(report.path().join(NATIVE_DEFINITIONS), definitions(&names[..1])?)?;
        let partial = PlanReader::new(Arc::new(plan(&names)), Some(report.path()), Vec::new())?;
        assert!(!partial.has_record_inputs());
        Ok(())
    }

    /// Admit selected ordinary and synthetic native functions through one indexed reader with either report form.
    #[test]
    fn compact_members_and_helpers_cover_the_plan_with_both_report_forms() -> Result<(), Box<dyn std::error::Error>> {
        let bytes = include_bytes!("../../tests/fixtures/plan_module_selection/70726f6265.incnmodule.positioned");
        let page = decode_plan_module(bytes)?;
        let [member] = page.functions.as_slice() else {
            return Err("the selected probe fixture must hold one function".into());
        };
        let event = decode_lowering_event(&member.event)?;
        let product = decode_plan_function_record(&event.product)?;
        assert_eq!(event.identity, member.symbol);
        assert_ne!(member.identity, member.symbol);
        let (function, externals) = decode_product(&product.function, &[])?;
        let mut plan = plan(&["__incan_probe_helper"]);
        let helper = &mut plan.functions[0];
        helper.locals.push(Local {
            ty: PlanType::Unit,
            span: helper.span.clone(),
        });
        helper.blocks.push(BasicBlock {
            statements: Vec::new(),
            terminator: Terminator {
                kind: TerminatorKind::Return,
                span: helper.span.clone(),
            },
            cleanup: false,
            span: helper.span.clone(),
        });
        plan.functions.push(function);
        plan.externals = externals;
        let compact = AdmittedDefinition {
            function: member.function.clone(),
            identity: member.symbol.clone(),
            key: event.key.clone(),
            digest: product.digest,
        };
        let fingerprint =
            incan_semantics_core::semantic_digest::semantic_digest(&vec![compact.key.clone(), compact.digest.clone()])?;
        let interface = encode_native_interface(&NativeInterface {
            entry: event.key,
            plan: encode_record_plan(&plan)?,
        })?;
        for with_interface in [false, true] {
            let report = tempfile::tempdir()?;
            std::fs::write(
                report.path().join(NATIVE_DEFINITIONS),
                definitions(&["__incan_probe_helper"])?,
            )?;
            if with_interface {
                std::fs::write(report.path().join(native_interface_filename()), &interface)?;
            }
            ACTIVE_FUNCTION_PROBES.set(0);
            let read = |compact| PlanReader::new(Arc::new(plan.clone()), Some(report.path()), compact);
            let reader = read(vec![compact.clone()])?;
            let names = ["__incan_probe_helper", "probe_run"];
            let covered = names
                .iter()
                .filter(|name| reader.function_input(name).is_some())
                .count();
            assert_eq!(
                covered, 2,
                "the native reader covered {covered} definitions, expected 2"
            );
            assert_eq!(ACTIVE_FUNCTION_PROBES.replace(0), 2);
            let input = reader
                .function_input("probe_run")
                .ok_or("the selected function has no input")?;
            assert_eq!(input.identity(), "pub::probe::run");
            assert_eq!(input.fingerprint(), fingerprint.trim_start_matches("sha256:"));
            assert_eq!(
                reader
                    .functions
                    .iter()
                    .map(|function| function.name.as_str())
                    .collect::<Vec<_>>(),
                names
            );

            let mut outside = compact.clone();
            outside.function = "outside_active_plan".into();
            assert!(matches!(read(vec![outside]), Err(reason) if reason.contains("outside the active plan")));
            assert!(matches!(
                read(vec![compact.clone(), compact.clone()]),
                Err(reason) if reason.contains("repeats native function")
            ));
            let scalar_path = report.path().join(admitted_definitions_filename());
            let mut scalar = compact.clone();
            scalar.identity = "legacy::probe::run".into();
            std::fs::write(&scalar_path, encode_admitted_definitions(&[scalar])?)?;
            assert!(matches!(
                read(vec![compact.clone()]),
                Err(reason) if reason.contains("repeats compact function")
            ));
            std::fs::write(report.path().join(format!("{}.event", compact.key)), &member.event)?;
            let legacy = read(Vec::new())?;
            let input = legacy
                .function_input("probe_run")
                .ok_or("legacy event input was lost")?;
            assert_eq!(input.identity(), "pub::probe::run");
            assert_eq!(input.fingerprint(), fingerprint.trim_start_matches("sha256:"));
            std::fs::remove_file(&scalar_path)?;
            assert!(
                matches!(read(vec![compact.clone()]), Err(reason) if reason.contains("as a changed event")),
                "a changed event superseded an authenticated compact definition"
            );
        }
        Ok(())
    }
}
