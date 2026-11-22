//! Keyed plan functions are byte-identical to the functions the whole-program plan compiles.

use std::path::{Path, PathBuf};

use incan_mir_plan_native::{
    AdmittedFunction, AdmittedPlacement, EventPosition, ExternalFunction, Function, INTERFACE_REPORT, Local,
    LoweringRefusal, ModelDeclaration, NominalKind, Parameter, Plan, PlanType, ProductNominal, SourceSpan,
    TerminatorKind, block, decode_lowering_event, decode_lowering_refusal, decode_native_interface,
    decode_plan_function_record, decode_record_plan, encode_lowering_refusal, product_nominals, read_module_admission,
    source_span,
};

use crate::plan_modules::report_admitted;
use crate::record_plan_outputs::publish_plan_records;
use crate::scalar_store::{ProductEvent, record_scalar_inputs, report_scalar, retain_scalar};

/// One request's product store and report directory, with its producer generation, under a fresh root.
struct Request {
    root: PathBuf,
    products: String,
    report: String,
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
    publish_plan_records(&unit, &request.products, &request.report)?;
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
    publish_plan_records(&unit, &request.products, &request.report)?;
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
    match publish_plan_records(&unit, &request.products, &request.report) {
        Ok(()) => panic!("a reused keyed plan that differs from the whole-program plan was accepted"),
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
    publish_plan_records(&unit, &request.products, &request.report)?;
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
    publish_plan_records(&unit, &request.products, &request.report)?;
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
    publish_plan_records(&unit, &request.products, &request.report)?;
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
    publish_plan_records(&unit, &request.products, &request.report)?;
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

/// A unit that calls a function of a crate it links, as a Loaf's own Rust facet, still compiles: its runtime inventory
/// cannot travel in an interface another unit might restore, so it publishes none, and says why.
#[test]
fn keyed_plan_calling_a_linked_crate_refuses_the_interface() -> Result<(), String> {
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
    publish_plan_records(&unit, &request.products, &request.report)?;
    assert!(
        !request.report(INTERFACE_REPORT).is_file(),
        "a unit inventory naming a linked crate published an interface"
    );
    assert!(
        request
            .refusals()?
            .iter()
            .any(|refusal| refusal.refusal.contains("Incan binding identity")),
        "an omitted interface did not say why its runtime inventory was refused"
    );
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
                key: "same-normalized-product".into(),
                positions: vec![EventPosition { line, column: 3 }],
            }),
        };
        let products = attempt.join("products").to_string_lossy().into_owned();
        report_admitted(Some(&root_text), "m", &[owner], &[Vec::new()], 1, true, Some(&products))?;
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
