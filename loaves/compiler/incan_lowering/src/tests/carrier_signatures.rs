//! Carrier discovery reads only the ordered, checked overloads of the selected callable.

use incan_semantics_core::body_ir::{ArgumentBinding, CallableParam, CallableParamDefault, LoafCallable, LocalId};
use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType,
};
use sha2::{Digest, Sha256};

use super::{
    BodyIrModule, CarrierPass, CarrierSignatures, ClosedCarrierSignatures, ConstructorIndex, EnumDeclaration,
    IncanType, NamedCallableTarget, generic,
};
use crate::body_index::{BodyIndex, body_index, extend_body_index};
use crate::tests::bodies::empty_body;

/// The complete source owner used by the carrier lookup, independent of the caller's spelling.
fn identity(name: &str) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(
        vec!["app".into()],
        name,
        super::SemanticSourceTargetKind::Function,
        HirSourceSpan::new(0, 10),
    )
}

/// One checked standard carrier suitable for comparing the selected layout order.
fn option_type(ty: IncanType) -> IncanType {
    generic("Option".into(), vec![ty])
}

/// A module whose 32 unrelated bodies and 8 linked callables surround three source `select` overloads, one of a
/// different arity, and the linked `select` the carrier demand names.
fn carrier_fixture_module(selected: &CanonicalSymbolId, text: &IncanType, boolean: &IncanType) -> BodyIrModule {
    let mut module = BodyIrModule::declarations_only("app".into());
    for index in 0..32 {
        let name = format!("unrelated_{index}");
        let mut body = empty_body(&name);
        body.canonical = Some(identity(&name));
        module.bodies.push(body);
    }
    let mut first = empty_body("select");
    first.canonical = Some(selected.clone());
    first.type_parameters = vec!["T".into()];
    first.params.push(CallableParam {
        local: LocalId(0),
        name: "value".into(),
        ty: option_type(IncanType::TypeVar("T".into())),
        kind: IncanCallableParamKind::Normal,
        span: first.span,
        default: CallableParamDefault::Required,
        mutable: false,
        native_owned: false,
        input_access: IncanInputAccess::Inferred,
    });
    let mut second = empty_body("select");
    second.canonical = Some(selected.clone());
    second.type_parameters = vec!["T".into()];
    second.return_type = option_type(text.clone());
    let mut different_arity = empty_body("select");
    different_arity.canonical = Some(selected.clone());
    module.bodies.extend([first, different_arity, second]);
    let linked = LoafCallable {
        canonical: selected.clone(),
        item: "app::select".into(),
        parameters: Vec::new(),
        return_type: option_type(boolean.clone()),
        type_parameters: vec!["T".into()],
        native_type_arguments: Vec::new(),
        native_callable_parameters: Vec::new(),
        parameter_defaults: Vec::new(),
        declaration_frame: None,
    };
    module.loaf_callables = (0..8)
        .map(|index| LoafCallable {
            canonical: identity(&format!("linked_{index}")),
            ..linked.clone()
        })
        .collect();
    module.loaf_callables.push(linked);
    module
}

/// Close the ordered source and linked signatures, refusing a changed linked arity without visiting other owners.
#[test]
fn a_named_carrier_demand_visits_only_its_ordered_signatures() -> Result<(), String> {
    let selected = identity("select");
    let int = IncanType::Primitive(IncanPrimitiveType::Int);
    let text = IncanType::Primitive(IncanPrimitiveType::Str);
    let boolean = IncanType::Primitive(IncanPrimitiveType::Bool);
    let target = NamedCallableTarget {
        name: "renamed_select".into(),
        direct_call_id: None,
        builtin: None,
        type_args: vec![int.clone()],
        binding: ArgumentBinding::resolved_positional(1),
        canonical: Some(selected.clone()),
        receiver_type: None,
    };
    let mut module = carrier_fixture_module(&selected, &text, &boolean);

    let constructors = ConstructorIndex::default();
    let body_indices = body_index(&module);
    let signatures = CarrierSignatures::new(&body_indices, &module);
    let mut layouts = Vec::new();
    let mut pass = CarrierPass::new(
        &constructors,
        &module,
        &mut layouts,
        "src/main.incn",
        Some(&signatures),
        None,
    );
    let started = std::time::Instant::now();
    pass.retain_call(&target)?;
    let mut same_target = target.clone();
    same_target.name = "another_local_alias".into();
    pass.retain_call(&same_target)?;
    let work = (pass.body_candidates, pass.loaf_candidates, pass.borrowed_position_bytes);
    println!(
        "source candidates={}, linked candidates={}, borrowed position bytes={}, borrowed parameter slot bytes={}, ns={}",
        work.0,
        work.1,
        work.2,
        pass.borrowed_parameter_bytes,
        started.elapsed().as_nanos()
    );
    let retained: Vec<&str> = layouts.iter().map(|layout| layout.source_type.as_str()).collect();
    let ordered = [int, text, boolean].map(|ty| option_type(ty).to_string());
    assert!(
        retained.iter().copied().eq(ordered.iter().map(String::as_str)),
        "{retained:?}"
    );
    let layout_bytes = serde_json::to_vec(&layouts).map_err(|error| error.to_string())?;
    println!(
        "ordered layout bytes={}, sha256={}",
        layout_bytes.len(),
        hex::encode(Sha256::digest(&layout_bytes))
    );

    let continued_module = module.clone();
    let Some(linked) = module.loaf_callables.last_mut() else {
        return Err("fixture has no linked callable".into());
    };
    linked.type_parameters.clear();
    let signatures = CarrierSignatures::new(&body_indices, &module);
    let mut layouts = Vec::new();
    let mut pass = CarrierPass::new(
        &constructors,
        &module,
        &mut layouts,
        "src/main.incn",
        Some(&signatures),
        None,
    );
    assert_eq!(
        pass.retain_call(&target),
        Err("the standard library carrier type argument count is unsupported".into())
    );
    assert_eq!(
        work,
        (3, 1, 3 * std::mem::size_of::<usize>()),
        "the carrier pass did not borrow only matching rows"
    );
    continuation_selections(continued_module, &target)
}

/// Reuse the same append-only checked body index across ordinary carrier signature passes.
#[derive(Default)]
struct SignatureContinuation {
    constructors: ConstructorIndex,
    body_indices: BodyIndex,
    closed_signatures: ClosedCarrierSignatures,
}

impl SignatureContinuation {
    fn observe(
        &mut self,
        module: &BodyIrModule,
        layouts: &mut Vec<EnumDeclaration>,
        targets: &[&NamedCallableTarget],
    ) -> Result<(usize, usize, usize), String> {
        extend_body_index(&mut self.body_indices, module)?;
        let signatures = CarrierSignatures::new(&self.body_indices, module);
        let mut pass = CarrierPass::new(
            &self.constructors,
            module,
            layouts,
            "src/main.incn",
            Some(&signatures),
            Some(&mut self.closed_signatures),
        );
        for target in targets {
            pass.retain_call(target)?;
        }
        Ok((pass.body_candidates, pass.loaf_candidates, pass.borrowed_position_bytes))
    }
}

/// Preserve the exact layout sequence and refusals when a continuation changes only some callable owners.
fn continuation_selections(mut module: BodyIrModule, target: &NamedCallableTarget) -> Result<(), String> {
    let mut continuation = SignatureContinuation::default();
    let mut missing = target.clone();
    missing.name = "still_waiting_for_its_declaration".into();
    let late = identity("late");
    missing.canonical = Some(late.clone());
    let mut layouts = Vec::new();
    let position_bytes = std::mem::size_of::<usize>();
    assert_eq!(
        continuation.observe(&module, &mut layouts, &[target, &missing])?,
        (3, 1, 3 * position_bytes)
    );
    let initial_layouts = serde_json::to_vec(&layouts).map_err(|error| error.to_string())?;

    let mut unrelated = empty_body("new_unrelated_body");
    unrelated.canonical = Some(identity("new_unrelated_body"));
    module.bodies.push(unrelated);
    let mut renamed = target.clone();
    renamed.name = "an_unrelated_local_alias".into();
    let repeated = continuation.observe(&module, &mut layouts, &[&renamed, &missing])?;
    assert_eq!(
        serde_json::to_vec(&layouts).map_err(|error| error.to_string())?,
        initial_layouts
    );

    let float = option_type(IncanType::Primitive(IncanPrimitiveType::Float));
    let mut discovered = empty_body("late");
    discovered.canonical = Some(late);
    discovered.type_parameters = vec!["T".into()];
    discovered.return_type = float.clone();
    module.bodies.push(discovered);
    assert_eq!(
        continuation.observe(&module, &mut layouts, &[&missing])?,
        (1, 0, position_bytes)
    );
    assert_eq!(
        layouts.last().map(|layout| &layout.source_type),
        Some(&float.to_string())
    );

    let bytes = option_type(IncanType::Primitive(IncanPrimitiveType::Bytes));
    let linked = module
        .loaf_callables
        .last_mut()
        .ok_or("fixture has no linked callable")?;
    linked.return_type = bytes.clone();
    assert_eq!(
        continuation.observe(&module, &mut layouts, &[target])?,
        (3, 1, 3 * position_bytes)
    );
    let order: Vec<_> = layouts.iter().map(|layout| layout.source_type.as_str()).collect();
    let expected = [
        IncanPrimitiveType::Int,
        IncanPrimitiveType::Str,
        IncanPrimitiveType::Bool,
    ]
    .map(|chosen| option_type(IncanType::Primitive(chosen)).to_string());
    let expected: Vec<_> = expected
        .into_iter()
        .chain([float.to_string(), bytes.to_string()])
        .collect();
    assert_eq!(order, expected);

    let linked = module
        .loaf_callables
        .last_mut()
        .ok_or("fixture has no linked callable")?;
    linked.type_parameters.clear();
    assert_eq!(
        continuation.observe(&module, &mut layouts, &[target]),
        Err("the standard library carrier type argument count is unsupported".into())
    );
    assert_eq!(repeated, (0, 0, 0), "unchanged continuation signature work");
    Ok(())
}
