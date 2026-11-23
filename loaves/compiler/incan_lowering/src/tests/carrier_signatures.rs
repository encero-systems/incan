//! Carrier discovery reads only the ordered, checked overloads of the selected callable.

use incan_semantics_core::body_ir::{ArgumentBinding, CallableParam, CallableParamDefault, LoafCallable, LocalId};
use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType,
};
use sha2::{Digest, Sha256};

use super::{BodyIrModule, CarrierPass, CarrierSignatures, ConstructorIndex, IncanType, NamedCallableTarget, generic};
use crate::body_index::body_index;
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
        canonical: selected,
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

    let constructors = ConstructorIndex::default();
    let body_indices = body_index(&module);
    let signatures = CarrierSignatures::new(&body_indices, &module);
    let mut layouts = Vec::new();
    let mut pass = CarrierPass::new(&constructors, &module, &mut layouts, "src/main.incn", Some(&signatures));
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

    let Some(linked) = module.loaf_callables.last_mut() else {
        return Err("fixture has no linked callable".into());
    };
    linked.type_parameters.clear();
    let signatures = CarrierSignatures::new(&body_indices, &module);
    let mut layouts = Vec::new();
    let mut pass = CarrierPass::new(&constructors, &module, &mut layouts, "src/main.incn", Some(&signatures));
    assert_eq!(
        pass.retain_call(&target),
        Err("the standard library carrier type argument count is unsupported".into())
    );
    assert_eq!(
        work,
        (3, 1, 3 * std::mem::size_of::<usize>()),
        "the carrier pass did not borrow only matching rows"
    );
    Ok(())
}
