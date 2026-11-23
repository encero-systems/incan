//! Keyed plan functions are byte-identical to the functions the whole-program plan compiles.

use std::path::{Path, PathBuf};

use incan_mir_plan_native::{
    ADMITTED_DEFINITIONS, AdmittedFunction, AdmittedPlacement, EventPosition, ExternalFunction, Function,
    INTERFACE_REPORT, Local, LoweringRefusal, ModelDeclaration, NATIVE_DEFINITIONS, NominalKind, Parameter, Plan,
    PlanType, ProductNominal, SourceSpan, TerminatorKind, block, decode_admitted_definitions, decode_lowering_event,
    decode_lowering_refusal, decode_native_interface, decode_plan_function_record, decode_record_plan,
    encode_lowering_refusal, product_nominals, read_module_admission, source_span,
};

use crate::plan_modules::{
    AdmittedProduct, PlanAdmissions, PlanModuleSelection, admitted_page, report_admitted, staged_page_reads,
};
use crate::record_plan_outputs::publish_plan_records;
use crate::scalar_store::{ProductEvent, record_scalar_inputs, report_scalar, retain_scalar};

/// One request's product store and report directory, with its producer generation, under a fresh root.
struct Request {
    root: PathBuf,
    products: String,
    report: String,
    admissions: PlanAdmissions,
}

impl Request {
    fn new(test: &str) -> Result<Self, String> {
        let root = std::env::temp_dir().join(format!("incan-record-outputs-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (products, report) = (root.join("products"), root.join("report"));
        std::fs::create_dir_all(&products).map_err(|error| error.to_string())?;
        std::fs::create_dir_all(&report).map_err(|error| error.to_string())?;
        std::fs::write(products.join("producer-profile"), "keyed-plan-test").map_err(|error| error.to_string())?;
        Ok(Request {
            products: products.to_string_lossy().into_owned(),
            report: report.to_string_lossy().into_owned(),
            admissions: PlanAdmissions::default(),
            root,
        })
    }

    fn report(&self, name: &str) -> PathBuf {
        Path::new(&self.report).join(name)
    }

    /// Every refusal the report holds.
    fn refusals(&self) -> Result<Vec<LoweringRefusal>, String> {
        let mut refusals = Vec::new();
        for entry in std::fs::read_dir(&self.report).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.to_string_lossy().ends_with(".refusal") {
                refusals.push(decode_lowering_refusal(&read(&path)?)?);
            }
        }
        Ok(refusals)
    }

    /// The encoded plan of the published native-interface record.
    fn interface_plan(&self) -> Result<Plan, String> {
        decode_record_plan(&decode_native_interface(&read(&self.report(INTERFACE_REPORT))?)?.plan)
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| error.to_string())
}

fn keyed_span() -> SourceSpan {
    source_span("keyed_plan.incn", 4, 1)
}

/// One synchronous unit function whose local count distinguishes two native functions.
fn unit_function(name: &str, locals: usize) -> Function {
    let span = keyed_span();
    let locals = (0..locals)
        .map(|_| Local {
            ty: PlanType::Unit,
            span: span.clone(),
        })
        .collect();
    let blocks = vec![block(Vec::new(), TerminatorKind::Return, &span)];
    Function::new(name.into(), Vec::new(), PlanType::Unit, locals, blocks, span)
}

fn inputs(function: &Function) -> Vec<String> {
    record_scalar_inputs("checked-meaning", &function.name, &function.span, false, "reads", 0)
}

fn event<'a>(
    request: &'a Request,
    inputs: &'a [String],
    function: &'a str,
    reused: bool,
    references: &'a [incan_mir_plan_native::NominalReference],
) -> ProductEvent<'a> {
    ProductEvent {
        products: &request.products,
        inputs,
        identity: function,
        function,
        reused,
        runtime: 0,
        references,
        positions: &[],
        module: "",
    }
}

/// Retain and report one product exactly as ordinary-frame lowering does, returning its key.
fn retain_product(request: &Request, function: &Function, reused: bool) -> Result<String, String> {
    let inputs = inputs(function);
    retain_scalar(&request.products, &inputs, function, &[], &[], &[], &[], &[])?;
    report_scalar(
        Some(&request.report),
        &event(request, &inputs, &function.name, reused, &[]),
    )?;
    for entry in std::fs::read_dir(&request.report).map_err(|error| error.to_string())? {
        let name = entry
            .map_err(|error| error.to_string())?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if let Some(key) = name.strip_suffix(".event") {
            return Ok(key.to_owned());
        }
    }
    Err("lowering reported no plan-function event".into())
}

fn plan(functions: Vec<Function>, programs: &[&str]) -> Plan {
    let mut plan = Plan::new(Vec::new(), Vec::new(), functions, Vec::new(), keyed_span());
    plan.programs = programs.iter().map(|program| (*program).to_owned()).collect();
    plan
}

/// A product captured before unit-level runtime resolution is replaced by the function the unit compiles.
#[test]
fn fresh_keyed_plan_retains_the_whole_program_function() -> Result<(), String> {
    let request = Request::new("fresh")?;
    let whole = unit_function("main", 1);
    let unit = plan(vec![whole.clone()], &[]);
    let key = retain_product(&request, &unit_function("main", 2), false)?;
    publish_plan_records(&unit, &request.products, &request.report, &request.admissions)?;
    assert_eq!(request.interface_plan()?, unit);
    let reported = decode_lowering_event(&read(&request.report(&format!("{key}.event")))?)?;
    let retained = read(&Path::new(&request.products).join(format!("{key}.incnfunction")))?;
    assert_eq!(reported.product, retained);
    assert_eq!(decode_plan_function_record(&reported.product)?.inputs, inputs(&whole));
    Ok(())
}

/// The first ordinary pass can capture a provisional native function before runtime selection closes.
#[test]
fn runtime_retry_keeps_current_requests_product_fresh() -> Result<(), String> {
    let request = Request::new("retry")?;
    let captured = unit_function("main", 2);
    let key = retain_product(&request, &captured, false)?;
    let path = request.report(&format!("{key}.event"));
    let record = decode_plan_function_record(&decode_lowering_event(&read(&path)?)?.product)?;
    report_scalar(
        Some(&request.report),
        &event(&request, &record.inputs, &captured.name, true, &[]),
    )?;
    assert!(!decode_lowering_event(&read(&path)?)?.reused);
    let unit = plan(vec![unit_function("main", 1)], &["main"]);
    publish_plan_records(&unit, &request.products, &request.report, &request.admissions)?;
    assert_eq!(request.interface_plan()?, unit);
    Ok(())
}

/// A reused product that differs from its whole-plan function refuses, even when the unit publishes no interface.
#[test]
fn reused_keyed_plan_must_equal_whole_program_plan() -> Result<(), String> {
    let request = Request::new("reused")?;
    let unit = plan(vec![unit_function("main", 1), unit_function("helper", 1)], &[]);
    let refusal = LoweringRefusal {
        identity: "helper".into(),
        function: "helper".into(),
        refusal: "checker supplied no complete scalar body witness".into(),
    };
    std::fs::write(request.report("helper.refusal"), encode_lowering_refusal(&refusal)?)
        .map_err(|error| error.to_string())?;
    retain_product(&request, &unit_function("main", 2), true)?;
    match publish_plan_records(&unit, &request.products, &request.report, &request.admissions) {
        Ok(_) => panic!("a reused keyed plan that differs from the whole-program plan was accepted"),
        Err(message) => assert!(
            message.contains("plan-function main differs from the whole-program plan"),
            "{message}"
        ),
    }
    Ok(())
}

/// A fresh product that names a model by declaration identity finalizes against the whole-program plan.
#[test]
fn keyed_plan_naming_a_model_publishes_its_interface() -> Result<(), String> {
    let request = Request::new("model")?;
    let span = keyed_span();
    let field = Parameter {
        name: "x".into(),
        ty: PlanType::Int,
        span: span.clone(),
    };
    let point = ModelDeclaration::new(
        "Point_0_37".into(),
        true,
        vec![field],
        vec![true],
        Vec::new(),
        span.clone(),
    );
    let mut function = unit_function("main", 1);
    function.locals.push(Local {
        ty: PlanType::Model(0, "Point_0_37".into()),
        span,
    });
    let mut unit = plan(vec![function.clone()], &[]);
    unit.models = vec![point];
    let inputs = inputs(&function);
    let references = product_nominals(&function, &[], &[])?;
    let identity = ProductNominal {
        kind: NominalKind::Model,
        identity: "model:keyed_plan::Point".into(),
        digest: "point-v1".into(),
    };
    retain_scalar(&request.products, &inputs, &function, &[], &[identity], &[], &[], &[])?;
    report_scalar(
        Some(&request.report),
        &event(&request, &inputs, &function.name, false, &references),
    )?;
    publish_plan_records(&unit, &request.products, &request.report, &request.admissions)?;
    assert_eq!(request.interface_plan()?, unit);
    Ok(())
}

/// A unit whose keyed product carries a helper frame publishes no interface, and reports why.
#[test]
fn keyed_plan_carrying_a_frame_refuses_the_interface() -> Result<(), String> {
    let request = Request::new("frame")?;
    let caller = unit_function("main", 1);
    let frame = unit_function("__incan_closure_main_0", 2);
    let unit = plan(vec![caller.clone(), frame.clone()], &[]);
    let inputs = inputs(&caller);
    retain_scalar(&request.products, &inputs, &caller, &[], &[], &[], &[frame], &[])?;
    report_scalar(
        Some(&request.report),
        &event(&request, &inputs, &caller.name, false, &[]),
    )?;
    publish_plan_records(&unit, &request.products, &request.report, &request.admissions)?;
    assert!(
        !request.report(INTERFACE_REPORT).is_file(),
        "a unit with a carried frame published an interface"
    );
    assert!(
        request
            .refusals()?
            .iter()
            .any(|refusal| refusal.refusal.contains("helper frame carried")),
        "an omitted interface carried no lowering refusal"
    );
    Ok(())
}

/// A unit whose plan gains a runtime frame no product witnesses publishes no interface, and names the frame.
#[test]
fn keyed_plan_with_a_generated_runtime_frame_refuses_the_interface() -> Result<(), String> {
    let request = Request::new("runtime-frame")?;
    let caller = unit_function("main", 1);
    let adapter = unit_function("__incan_module_runtime_assert__runtime_frame", 2);
    let unit = plan(vec![caller.clone(), adapter.clone()], &[]);
    retain_product(&request, &caller, false)?;
    publish_plan_records(&unit, &request.products, &request.report, &request.admissions)?;
    assert!(
        !request.report(INTERFACE_REPORT).is_file(),
        "an unwitnessed runtime frame published an interface"
    );
    assert!(
        request
            .refusals()?
            .iter()
            .any(|refusal| refusal.function == adapter.name && refusal.refusal.contains("generated function")),
        "an omitted interface did not name its unwitnessed generated function"
    );
    Ok(())
}

/// A unit whose main returns a value, as a `Result` main does, still compiles: it publishes no interface, and says why.
///
/// An async main never gets this far: its product refuses coroutine state, so its unit publishes no interface anyway.
#[test]
fn keyed_plan_with_a_returning_main_refuses_the_interface() -> Result<(), String> {
    let request = Request::new("returning-main")?;
    let mut main = unit_function("main", 1);
    main.return_type = PlanType::Int;
    let unit = plan(vec![main.clone()], &[]);
    retain_product(&request, &main, false)?;
    publish_plan_records(&unit, &request.products, &request.report, &request.admissions)?;
    assert!(
        !request.report(INTERFACE_REPORT).is_file(),
        "a main the entry record cannot bind published an interface"
    );
    assert!(
        request
            .refusals()?
            .iter()
            .any(|refusal| refusal.function == "main" && refusal.refusal.contains("synchronous unit main")),
        "an omitted interface did not say why the entry was refused"
    );
    Ok(())
}

/// A unit that calls a function of a crate it links, as a Loaf's own Rust facet, publishes its interface: the
/// interface keeps the linked-crate external, and only a unit that links the crate restores it.
#[test]
fn keyed_plan_calling_a_linked_crate_publishes_the_interface() -> Result<(), String> {
    let request = Request::new("linked-crate")?;
    let main = unit_function("main", 1);
    let mut unit = plan(vec![main.clone()], &[]);
    unit.externals = vec![ExternalFunction {
        path: "rustself_native::native_len".into(),
        type_arguments: Vec::new(),
        parameters: vec![PlanType::Int],
        return_type: PlanType::Int,
        span: keyed_span(),
    }];
    retain_product(&request, &main, false)?;
    publish_plan_records(&unit, &request.products, &request.report, &request.admissions)?;
    assert!(
        request.report(INTERFACE_REPORT).is_file(),
        "a unit inventory naming a linked crate published no interface"
    );
    assert!(
        !request
            .refusals()?
            .iter()
            .any(|refusal| refusal.refusal.contains("Incan binding identity")),
        "a linked-crate external was refused instead of kept"
    );
    Ok(())
}

/// Supply the exact key and digest fields written after a selected plan product has been admitted.
fn plan_evidence() -> AdmittedProduct {
    AdmittedProduct {
        key: "ab".repeat(32),
        digest: format!("sha256:{}", "cd".repeat(32)),
        helpers: Vec::new(),
    }
}

/// An incomplete interface can publish native fingerprints from admitted evidence without reopening its plan page.
#[test]
fn admitted_definition_publication_uses_settled_products_without_reading_pages() -> Result<(), String> {
    let mut request = Request::new("retain-admitted-native-definition")?;
    let owner = AdmittedFunction {
        function: "main".into(),
        identity: "stable.main".into(),
        references: Vec::new(),
        placement: None,
    };
    report_admitted(
        &mut request.admissions,
        Some(&request.report),
        "m",
        vec![owner],
        vec![plan_evidence()],
        1,
        true,
        Some(&request.products),
        None,
    )?;
    assert!(
        !Path::new(&request.products)
            .join("admissions/6d.admitted.frames")
            .is_file(),
        "one redundant per-module frame sidecar was written"
    );
    let unit = plan(vec![unit_function("main", 1), unit_function("helper", 2)], &[]);
    publish_plan_records(&unit, &request.products, &request.report, &request.admissions)?;
    let definitions = decode_admitted_definitions(&read(&request.report(ADMITTED_DEFINITIONS))?)?;
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].function, "main");
    assert_eq!(definitions[0].identity, "stable.main");
    assert_eq!(definitions[0].key, plan_evidence().key);
    assert_eq!(definitions[0].digest, plan_evidence().digest);
    assert!(!request.report(INTERFACE_REPORT).is_file());
    // A complete interface must still read actual product bytes and refuse a missing page.
    let complete = plan(vec![unit_function("main", 1)], &[]);
    assert!(publish_plan_records(&complete, &request.products, &request.report, &request.admissions).is_err());
    Ok(())
}

/// Two overlapping requests have the same visible description, scalar, symbol and number of nodes.
#[test]
fn admitted_positions_are_private_to_the_compiler_request() -> Result<(), String> {
    let request = Request::new("admitted")?;
    let root = request.root.clone();
    let attempts = [root.join("plan-admission-a"), root.join("plan-admission-b")];
    let root_text = root.to_string_lossy().into_owned();
    for (line, attempt) in (8..).zip(&attempts) {
        let owner = AdmittedFunction {
            function: "f".into(),
            identity: "stable.symbol".into(),
            references: Vec::new(),
            placement: Some(AdmittedPlacement {
                key: "ab".repeat(32),
                positions: vec![EventPosition { line, column: 3 }],
            }),
        };
        let products = attempt.join("products").to_string_lossy().into_owned();
        report_admitted(
            &mut PlanAdmissions::default(),
            Some(&root_text),
            "m",
            vec![owner],
            vec![plan_evidence()],
            1,
            true,
            Some(&products),
            None,
        )?;
    }
    for (line, attempt) in (8..).zip(&attempts) {
        let note = read_module_admission(
            &attempt
                .join("products")
                .join("admissions")
                .join("6d.admitted")
                .to_string_lossy(),
        )?;
        assert!(note.inspected && note.functions.len() == 1);
        let Some(placed) = &note.functions[0].placement else {
            return Err("a later request replaced the earlier admission".into());
        };
        assert_eq!(placed.positions, [EventPosition { line, column: 3 }]);
    }
    // An old reader sees the usual note. Its shared path carries no authoritative placement.
    assert!(!root.join("6d.admitted.placement").is_file());
    Ok(())
}

/// The native inputs a plan report names, by function.
fn native_inputs(request: &Request) -> Result<std::collections::BTreeMap<String, (String, String)>, String> {
    let definitions = decode_admitted_definitions(&read(&request.report(NATIVE_DEFINITIONS))?)?;
    Ok(definitions
        .into_iter()
        .map(|definition| (definition.function, (definition.key, definition.digest)))
        .collect())
}

/// A unit that publishes no native interface still names a native input for every function no event names: a helper
/// frame by the product that carries it, an unwitnessed generated function by its own content. Native incremental
/// reuse can then track every definition, and an input changes exactly when what it names changes.
#[test]
fn every_function_without_an_event_has_a_native_input() -> Result<(), String> {
    let caller = unit_function("main", 1);
    let frame = unit_function("__incan_closure_main_0", 2);
    let runtime = "__incan_module_runtime_assert__runtime_frame";
    let publish = |test: &str, caller: &Function, adapter: &Function| -> Result<_, String> {
        let request = Request::new(test)?;
        let inputs = inputs(caller);
        retain_scalar(
            &request.products,
            &inputs,
            caller,
            &[],
            &[],
            &[],
            std::slice::from_ref(&frame),
            &[],
        )?;
        report_scalar(
            Some(&request.report),
            &event(&request, &inputs, &caller.name, false, &[]),
        )?;
        let unit = plan(vec![caller.clone(), frame.clone(), adapter.clone()], &[]);
        publish_plan_records(&unit, &request.products, &request.report, &request.admissions)?;
        assert!(!request.report(INTERFACE_REPORT).is_file());
        let named = native_inputs(&request)?;
        let key = std::fs::read_dir(&request.report)
            .map_err(|error| error.to_string())?
            .filter_map(|entry| entry.ok())
            .find_map(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .strip_suffix(".event")
                    .map(str::to_owned)
            })
            .ok_or("no event")?;
        Ok((named, key))
    };
    let (first, key) = publish("inputs", &caller, &unit_function(runtime, 2))?;
    assert_eq!(
        first.keys().cloned().collect::<Vec<_>>(),
        vec![frame.name.clone(), runtime.to_owned()]
    );
    assert_eq!(
        first[&frame.name].0, key,
        "a helper frame's input names the product that carries it"
    );
    // The same content names the same inputs; a changed generated function changes only its own.
    let (again, _) = publish("inputs-again", &caller, &unit_function(runtime, 2))?;
    assert_eq!(again, first);
    let (changed, _) = publish("inputs-changed", &caller, &unit_function(runtime, 3))?;
    assert_eq!(changed[&frame.name], first[&frame.name]);
    assert_ne!(changed[runtime], first[runtime]);
    // A changed carrier changes its frame's input.
    let (carried, _) = publish("inputs-carrier", &unit_function("main", 3), &unit_function(runtime, 2))?;
    assert_ne!(carried[&frame.name], first[&frame.name]);
    Ok(())
}

/// An admitted plan with complete coverage reconstructs the whole interface through the original product bytes.
#[test]
fn complete_admitted_interface_matches_the_whole_plan_bytes() -> Result<(), String> {
    use incan_mir_plan_native::{
        POSITIONED_MODULE_PAGE, PlanModuleFunction, PlanModuleValue, encode_lowering_event, encode_plan_module,
    };

    let mut request = Request::new("complete-admitted-interface")?;
    let function = unit_function("main", 1);
    let key = retain_product(&request, &function, false)?;
    let event_path = request.report(&format!("{key}.event"));
    let mut event = decode_lowering_event(&read(&event_path)?)?;
    event.module = "m".into();
    let record = decode_plan_function_record(&event.product)?;
    let page = PlanModuleValue {
        module: "m".into(),
        functions: vec![PlanModuleFunction {
            identity: event.identity.clone(),
            symbol: event.identity.clone(),
            function: event.function.clone(),
            key: key.clone(),
            event: encode_lowering_event(&event)?,
        }],
    };
    let path = Path::new(&request.products).join(format!("6d{POSITIONED_MODULE_PAGE}"));
    std::fs::write(path, encode_plan_module(&page)?).map_err(|error| error.to_string())?;
    std::fs::remove_file(event_path).map_err(|error| error.to_string())?;
    let owner = AdmittedFunction {
        function: event.function,
        identity: event.identity,
        references: Vec::new(),
        placement: None,
    };
    let mut evidence = AdmittedProduct {
        key,
        digest: record.digest,
        helpers: Vec::new(),
    };
    report_admitted(
        &mut request.admissions,
        Some(&request.report),
        "m",
        vec![owner.clone()],
        vec![evidence.clone()],
        1,
        false,
        Some(&request.products),
        None,
    )?;
    let whole = plan(vec![function], &[]);
    publish_plan_records(&whole, &request.products, &request.report, &request.admissions)?;
    let produced = postcard::to_allocvec(&request.interface_plan()?).map_err(|error| error.to_string())?;
    let expected = postcard::to_allocvec(&whole).map_err(|error| error.to_string())?;
    assert_eq!(produced, expected);
    let interface = decode_native_interface(&read(&request.report(INTERFACE_REPORT))?)?;
    std::fs::remove_file(request.report(&format!("{}.event", interface.entry))).map_err(|error| error.to_string())?;

    evidence.digest = format!("sha256:{}", "ef".repeat(32));
    report_admitted(
        &mut request.admissions,
        Some(&request.report),
        "m",
        vec![owner],
        vec![evidence],
        1,
        false,
        Some(&request.products),
        None,
    )?;
    match publish_plan_records(&whole, &request.products, &request.report, &request.admissions) {
        Err(error) if error.contains("digest differs from its settled product") => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err("a changed admission digest was accepted".into()),
    }
}

/// A complete selected positioned module uses one compact note beside the separate changed scalar event.
#[test]
fn settled_positioned_module_emits_one_compact_note() -> Result<(), String> {
    use incan_mir_plan_native::{
        ADMITTED_MODULE_NOTE, POSITIONED_MODULE_PAGE, PlanModuleFunction, PlanModuleValue, compact_admission_pin,
        decode_whole_module_admission, encode_lowering_event, encode_plan_module,
    };
    use sha2::{Digest, Sha256};

    let mut request = Request::new("settled-positioned-compact")?;
    let unchanged = unit_function("unchanged", 1);
    let keyed = retain_product(&request, &unchanged, false)?;
    let initial = request.report(&format!("{keyed}.event"));
    let mut event = decode_lowering_event(&read(&initial)?)?;
    std::fs::remove_file(initial).map_err(|error| error.to_string())?;
    event.module = "m".into();
    let product = decode_plan_function_record(&event.product)?;
    let page = encode_plan_module(&PlanModuleValue {
        module: event.module.clone(),
        functions: vec![PlanModuleFunction {
            function: event.function.clone(),
            identity: event.identity.clone(),
            symbol: event.identity.clone(),
            key: keyed.clone(),
            event: encode_lowering_event(&event)?,
        }],
    })?;
    let products = PathBuf::from(&request.products);
    let basename = format!("6d{POSITIONED_MODULE_PAGE}");
    let digest = hex::encode(Sha256::digest(&page));
    std::fs::write(products.join(&basename), page).map_err(|error| error.to_string())?;
    std::fs::write(products.join(format!("{basename}.digest")), &digest).map_err(|error| error.to_string())?;
    let pin = [0x39; 32];
    let output = format!("plan-module\tincan.stable-declaration/1\tprobe\tm.incn\tchecked\t{digest}");
    let selection = positioned_selection(&digest, &output, pin);
    let owner = AdmittedFunction {
        function: event.function,
        identity: event.identity,
        references: event.references,
        placement: None,
    };
    let evidence = AdmittedProduct {
        key: keyed,
        digest: product.digest,
        helpers: Vec::new(),
    };
    let name = format!("6d{ADMITTED_MODULE_NOTE}");
    let note_path = products.join("admissions").join(&name);
    let publish = |request: &mut Request, selection, owner, evidence, unit: &Plan| -> Result<_, String> {
        let empty = !note_path.is_file();
        request.admissions = PlanAdmissions::with_selection(selection);
        let staged = admitted_page(Some(&request.products), "m", Some("keyed-plan-test"));
        report_admitted(
            &mut request.admissions,
            Some(&request.report),
            "m",
            vec![owner],
            vec![evidence],
            1,
            false,
            Some(&request.products),
            staged.positioned_content(),
        )?;
        if empty && request.admissions.selection().is_some() {
            assert!(
                !note_path.is_file(),
                "a compact candidate wrote a provisional scalar note"
            );
        }
        publish_plan_records(unit, &request.products, &request.report, &request.admissions)
    };
    let changed = unit_function("main", 2);
    let changed_key = retain_product(&request, &changed, false)?;
    let unit = plan(vec![unchanged, changed], &[]);
    let page = products.join(&basename);
    let before = staged_page_reads(&page);
    let settled = publish(&mut request, Some(selection), owner.clone(), evidence.clone(), &unit)?;
    assert_eq!(
        settled.map_or(0, |pin| pin.count),
        1,
        "a settled complete positioned module emitted no compact note"
    );
    assert_eq!(
        staged_page_reads(&page) - before,
        1,
        "the complete interface opens its selected page once"
    );
    let encoded = read(&note_path)?;
    let note_digest = Sha256::digest(&encoded);
    let verified = compact_admission_pin([("m", note_digest.into())])?;
    assert_eq!(settled, Some(verified));
    let note = decode_whole_module_admission(&encoded)?;
    assert_eq!(
        (note.module, note.output, note.digest, note.count, note.selection),
        ("m".into(), output, digest, 1, hex::encode(pin))
    );
    assert_eq!(read(&request.report(&name))?, encoded);
    assert!(!products.join("admissions").join(format!("{name}.placement")).exists());
    assert!(!request.report(ADMITTED_DEFINITIONS).is_file());
    let changed_event = decode_lowering_event(&read(&request.report(&format!("{changed_key}.event")))?)?;
    assert_eq!(changed_event.function, "main");
    assert_eq!(ordinary_events(&request)?, ["main"]);
    let interface = read(&request.report(INTERFACE_REPORT))?;
    let selected_entry = decode_native_interface(&interface)?.entry;
    std::fs::remove_file(request.report(&format!("{selected_entry}.event"))).map_err(|error| error.to_string())?;
    assert_eq!(publish(&mut request, None, owner, evidence, &unit)?, None);
    let scalar = read_module_admission(&note_path.to_string_lossy())?;
    assert!(scalar.complete && scalar.functions.len() == 1);
    assert_eq!(read(&request.report(INTERFACE_REPORT))?, interface);
    println!(
        "compact fixture notes={} set={} private_note={} interface={} full_page_reads=1",
        verified.count,
        hex::encode(verified.digest),
        hex::encode(&encoded),
        hex::encode(Sha256::digest(&interface)),
    );
    let native = postcard::to_allocvec(&request.interface_plan()?).map_err(|error| error.to_string())?;
    let expected = postcard::to_allocvec(&unit).map_err(|error| error.to_string())?;
    assert_eq!(native, expected);
    Ok(())
}

/// One independently selected exact positioned page.
fn positioned_selection(digest: &str, output: &str, pin: [u8; 32]) -> PlanModuleSelection {
    use incan_mir_plan_native::{PlanModuleSelectionRecord, PlanSelectedModule, PlanSelectionScope};

    PlanModuleSelection::test_selection(
        PlanModuleSelectionRecord {
            scope: PlanSelectionScope {
                store_root: String::new(),
                placement: "memory:1".into(),
                manifest: "ab".repeat(32),
                generation: 4,
                view: "home".into(),
                index: "index".into(),
                origin: "probe".into(),
                profile: "incan.lowering-records/12".into(),
            },
            modules: vec![PlanSelectedModule {
                module: "m".into(),
                output: output.into(),
                digest: digest.into(),
                count: 1,
                positioned: true,
                rank: 1,
                certificate: "probe-order".into(),
            }],
        },
        pin,
    )
}

/// Report actual emitted scalar definitions, omitting the ordinary pseudo-entry binding.
fn ordinary_events(request: &Request) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&request.report).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().is_some_and(|suffix| suffix == "event") {
            let event = decode_lowering_event(&read(&path)?)?;
            if event.role != "entry" {
                names.push(event.function);
            }
        }
    }
    names.sort();
    Ok(names)
}
