//! Exact representation-operation comparisons over the same checked seed. These are migration baselines, not
//! producers: only this harness assembles a whole module to compare the independent native operations.

use super::{
    BodyIrContext, ProviderOperationCatalog, bir, legacy_opaque_returns, legacy_union_layouts, opaque_returns,
    union_layouts,
};
use crate::typechecker::{CheckedBodyInputError, TypeCheckInfo, TypeChecker};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Compare complete serialized bodies and ordered union layouts after independent local operations, including an
/// empty sibling, repeated shapes and nested callable unions. Ordered values and coordinates are never removed.
#[test]
fn local_representation_operations_match_module_passes() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Readable:\n    def value(self) -> int: ...\n    def again(self) -> Self:\n        return self\nmodel Value with Readable:\n    number: int\n    def value(self) -> int:\n        return self.number\ndef make() -> Readable:\n    return Value(number=1)\ndef forward() -> Readable:\n    return make()\ndef through(value: Readable) -> int:\n    return value.value()\ndef first() -> int:\n    value = forward()\n    return value.again().value()\ndef sibling() -> int:\n    return 2\n";
    let (program, facts, mut seed) = checked_seed(source)?;
    let callable = super::IncanType::Function {
        params: Vec::new(),
        return_type: Box::new(super::IncanType::Primitive(super::IncanPrimitiveType::Int)),
    };
    let shape = super::IncanType::Generic {
        base: "Union".to_owned(),
        args: vec![
            callable.clone(),
            super::IncanType::Primitive(super::IncanPrimitiveType::Str),
        ],
    };
    let carriers = opaque_returns::collect(&seed, &facts);
    let legacy_carriers = legacy_opaque_returns::collect(&seed, &facts);
    assert_eq!(carriers.len(), 1);
    let mut expected = seed.clone();
    super::legacy_trait_values::retain_inputs(&mut expected, &program, &facts, &[]);
    super::trait_values::retain_inputs(&mut seed, &program, &facts, &[]);
    assert_same_module(&expected, &seed)?;
    let through = seed
        .bodies
        .iter()
        .find(|body| body.name == "through")
        .ok_or("missing trait input body")?;
    assert_eq!(through.type_parameters.len(), 1);
    assert!(matches!(through.params[0].ty, super::IncanType::TypeVar(_)));
    super::legacy_trait_returns::retain_returns(&mut expected, &program, &facts, &[]);
    super::trait_returns::retain_returns(&mut seed, &program, &facts, &[]);
    assert_same_module(&expected, &seed)?;
    let forward = seed
        .bodies
        .iter()
        .find(|body| body.name == "forward")
        .ok_or("missing trait return body")?;
    assert!(matches!(&forward.return_type, super::IncanType::Named(name)
        if forward.named_type_identities.get(name).is_some_and(|identity| identity.declaration_name == "Value")));
    legacy_opaque_returns::retain(&mut expected, &legacy_carriers);
    for body in &mut seed.bodies {
        opaque_returns::retain_body(body, &carriers);
    }
    assert_same_module(&expected, &seed)?;

    seed.bodies
        .iter_mut()
        .find(|body| body.name == "first")
        .ok_or("missing first body")?
        .return_type = super::IncanType::Tuple(vec![shape.clone(), shape.clone()]);
    seed.nominal_declarations[0].field_types = vec![shape.clone()];
    seed.bodies
        .iter_mut()
        .find(|body| body.name == "sibling")
        .ok_or("missing sibling body")?
        .return_type = super::IncanType::Generic {
        base: "Union".to_owned(),
        args: vec![shape, callable],
    };

    let mut legacy = seed.clone();
    legacy_union_layouts::retain(&mut legacy);
    let mut independent = Vec::new();
    for body in &seed.bodies {
        union_layouts::append_types(&mut independent, union_layouts::body_types(body));
    }
    for owner in &seed.nominal_declarations {
        union_layouts::append_types(&mut independent, union_layouts::storage_types(owner.field_types.iter()));
    }
    let independent = independent
        .into_iter()
        .filter_map(union_layouts::layout)
        .collect::<Vec<_>>();
    assert_eq!(
        postcard::to_stdvec(&legacy.native_union_declarations)?,
        postcard::to_stdvec(&independent)?
    );
    assert_eq!(independent.len(), 2);

    assert!(union_layouts::storage_types(std::iter::empty()).is_empty());
    assert_settled_function_seeds()?;
    assert_free_function_input_baseline()?;
    assert_shared_loans()?;
    assert_projected_shared_loans()?;
    assert_complete_shared_read_shapes()?;
    assert_lent_and_moved_read()?;
    assert_equality_contract()?;
    let mut changed = seed.clone();
    changed.bodies[0].native_attributes.push(bir::NativeAttribute {
        path: "selected::attribute".to_owned(),
        arguments: Vec::new(),
    });
    assert_eq!(postcard::to_stdvec(&seed)?, postcard::to_stdvec(&changed)?);
    let difference = assert_same_module(&seed, &changed)
        .err()
        .ok_or("baseline omitted an invocation-only field")?;
    assert!(difference.to_string().contains("native_attributes"));
    Ok(())
}

/// Prepare identical checked inputs for the old module pass and the independent native operations. This whole
/// seed construction exists only in the verification harness and is never a record producer implementation.
fn checked_seed(
    source: &str,
) -> Result<(crate::ast::Program, TypeCheckInfo, bir::BodyIrModule), Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let seed = super::build_module_seed(&program, &[], &facts, &context);
    Ok((program, facts, seed))
}

/// Prove the decreasing shared-access fixed point across forwarding, recursion and a function-item adapter while a
/// value-returning body and explicit source contracts keep their established ownership.
fn assert_shared_loans() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Native:\n    value: int\nmodel Wrapper:\n    item: Native\nclass Reader:\n    pub def read(self, items: Native) -> int:\n        return items.value\ntrait Fetcher:\n    def fetch(self, scope: Native, request: Native) -> int: ...\nclass Fetch with Fetcher:\n    def fetch(self, scope: Native, request: Native) -> int:\n        return scope.value + request.value\ndef leaf(items: Native) -> int:\n    return items.value\ndef explicit(items: &Native) -> int:\n    return items.value\ndef forward(items: Native) -> int:\n    return leaf(items)\ndef explicit_forward(items: Native) -> int:\n    return explicit(items)\ndef explicit_projected(wrapper: &Wrapper) -> int:\n    return explicit(wrapper.item)\ndef method_forward(reader: Reader, items: Native) -> int:\n    return reader.read(items)\ndef trait_forward[F with Fetcher](source: F, scope: Native, request: Native) -> int:\n    return source.fetch(scope, request)\ndef left(items: Native, remaining: int) -> int:\n    if remaining == 0:\n        return items.value\n    return right(items, remaining - 1)\ndef right(items: Native, remaining: int) -> int:\n    if remaining == 0:\n        return items.value\n    return left(items, remaining - 1)\ndef escaped(items: Native) -> int:\n    return items.value\ndef take(items: Native) -> Native:\n    return items\ndef use() -> int:\n    callback = escaped\n    return callback(Native(value=1))\n";
    let (_, _, mut candidate) = checked_seed(source)?;
    let leaf = candidate
        .bodies
        .iter_mut()
        .find(|body| body.name == "leaf")
        .ok_or("missing profiled loan body")?;
    leaf.checked_input_profile = Some(bir::CheckedInputProfile {
        version: 1,
        owner: leaf.canonical.clone().ok_or("missing profiled loan identity")?,
        parameters: leaf
            .params
            .iter()
            .enumerate()
            .map(|(slot, parameter)| bir::CheckedInputParameter {
                slot,
                access: parameter.input_access,
            })
            .collect(),
        owned_initializers: Vec::new(),
    });
    for access in [
        incan_semantics_core::IncanInputAccess::Shared,
        incan_semantics_core::IncanInputAccess::Exclusive,
        incan_semantics_core::IncanInputAccess::Owned,
    ] {
        let mut explicit = candidate.clone();
        let parameter = explicit
            .bodies
            .iter_mut()
            .find(|body| body.name == "leaf")
            .ok_or("missing explicit loan control")?;
        parameter.params[0].input_access = access;
        let original_type = parameter.params[0].ty.clone();
        super::shared_reads::retain_modules(std::slice::from_mut(&mut explicit), &[]);
        let parameter = explicit
            .bodies
            .iter()
            .find(|body| body.name == "leaf")
            .ok_or("missing retained explicit loan control")?;
        assert_eq!(parameter.params[0].input_access, access);
        assert_eq!(parameter.params[0].ty, original_type);
        assert_eq!(parameter.locals[parameter.params[0].local.index()].ty, original_type);
    }
    super::shared_reads::retain_modules(std::slice::from_mut(&mut candidate), &[]);
    for name in ["leaf", "forward", "explicit_forward", "left", "right", "escaped"] {
        let body = candidate
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing loan body")?;
        assert_eq!(
            body.params[0].input_access,
            incan_semantics_core::IncanInputAccess::InferredShared,
            "{name}: {body:#?}"
        );
        if name == "leaf" {
            let retained = body
                .checked_input_profile
                .as_ref()
                .and_then(|profile| profile.parameters.first())
                .ok_or("missing retained loan profile")?;
            assert_eq!(retained.access, body.params[0].input_access);
        }
        assert!(matches!(body.params[0].ty, super::IncanType::Named(_)));
    }
    let method = candidate
        .bodies
        .iter()
        .find(|body| body.name == "read")
        .ok_or("missing loan method")?;
    assert_eq!(
        method.params[1].input_access,
        incan_semantics_core::IncanInputAccess::InferredShared
    );
    let method_forward = candidate
        .bodies
        .iter()
        .find(|body| body.name == "method_forward")
        .ok_or("missing loan method caller")?;
    assert_eq!(
        method_forward.params[1].input_access,
        incan_semantics_core::IncanInputAccess::InferredShared
    );
    for name in ["explicit_forward", "explicit_projected"] {
        let body = candidate
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing explicit forwarding body")?;
        assert!(body.block.stmts.iter().any(|statement| {
            let bir::StatementKind::Call { args, .. } = &statement.kind else {
                return false;
            };
            args.iter().any(|argument| {
                matches!(
                    argument,
                    bir::ArgumentElement::One(bir::Operand::Place(read))
                        if read.fact == bir::OwnershipFact::Borrow && !read.last_use
                )
            })
        }));
    }
    let trait_forward = candidate
        .bodies
        .iter()
        .find(|body| body.name == "trait_forward")
        .ok_or("missing loan trait caller")?;
    assert_eq!(
        trait_forward.params[1].input_access,
        incan_semantics_core::IncanInputAccess::Inferred,
        "{trait_forward:#?}"
    );
    assert_eq!(
        trait_forward.params[2].input_access,
        incan_semantics_core::IncanInputAccess::Inferred,
        "{trait_forward:#?}"
    );
    let implementation = candidate
        .bodies
        .iter()
        .find(|body| body.name == "fetch")
        .ok_or("missing retained trait implementation")?;
    assert!(
        implementation.params[1..]
            .iter()
            .all(|parameter| parameter.input_access == incan_semantics_core::IncanInputAccess::Inferred)
    );
    let take = candidate
        .bodies
        .iter()
        .find(|body| body.name == "take")
        .ok_or("missing owned result body")?;
    assert_eq!(
        take.params[0].input_access,
        incan_semantics_core::IncanInputAccess::Inferred
    );
    let use_body = candidate
        .bodies
        .iter()
        .find(|body| body.name == "use")
        .ok_or("missing function item body")?;
    let adapter = use_body.block.stmts.iter().find_map(|statement| {
        let bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Closure { params, body, .. },
            ..
        } = &statement.kind
        else {
            return None;
        };
        matches!(
            body.stmts.as_slice(),
            [bir::Statement {
                kind: bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Named(_)),
                    ..
                },
                ..
            }]
        )
        .then_some((params, body))
    });
    let (parameters, body) = adapter.ok_or("missing function item adapter")?;
    assert!(!body.function_item);
    assert_eq!(
        parameters[0].input_access,
        incan_semantics_core::IncanInputAccess::Inferred
    );
    let Some(bir::Statement {
        kind: bir::StatementKind::Call { args, .. },
        ..
    }) = body.stmts.first()
    else {
        return Err("function item adapter has no forwarding call".into());
    };
    assert!(matches!(
        args.first(),
        Some(bir::ArgumentElement::One(bir::Operand::Place(read)))
            if read.fact == bir::OwnershipFact::Borrow && !read.last_use
    ));
    Ok(())
}

/// Prove an exact checked field projection is lent to a read-only callee while a projected transfer keeps both the
/// field and its containing parameter owned.
fn assert_projected_shared_loans() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Native:\n    value: int\nmodel Wrapper:\n    item: Native\ndef observe(item: Native) -> int:\n    return item.value\ndef projected(wrapper: Wrapper) -> int:\n    return observe(wrapper.item)\ndef take(item: Native) -> Native:\n    return item\ndef projected_take(wrapper: Wrapper) -> Native:\n    return take(wrapper.item)\n";
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(std::slice::from_mut(&mut module), &[]);

    for name in ["observe", "projected"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing projected shared body")?;
        assert_eq!(
            body.params[0].input_access,
            incan_semantics_core::IncanInputAccess::InferredShared,
            "{name}: {body:#?}"
        );
    }
    let projected = module
        .bodies
        .iter()
        .find(|body| body.name == "projected")
        .ok_or("missing projected caller")?;
    assert!(projected.block.stmts.iter().any(|statement| {
        matches!(
            &statement.kind,
            bir::StatementKind::Call { args, .. }
                if matches!(args.first(), Some(bir::ArgumentElement::One(bir::Operand::Place(read)))
                    if !read.place.projection.is_empty()
                        && read.fact == bir::OwnershipFact::Borrow
                        && !read.last_use)
        )
    }));
    for name in ["take", "projected_take"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing projected transfer body")?;
        assert_eq!(
            body.params[0].input_access,
            incan_semantics_core::IncanInputAccess::Inferred,
            "{name}: {body:#?}"
        );
    }
    let conversion_source =
        "def count(values: list[int]) -> int:\n    return 42\ndef ranged() -> int:\n    return count(range(1, 3))\n";
    let (_, _, mut conversions) = checked_seed(conversion_source)?;
    super::shared_reads::retain_modules(std::slice::from_mut(&mut conversions), &[]);
    let ranged = conversions
        .bodies
        .iter()
        .find(|body| body.name == "ranged")
        .ok_or("missing range conversion caller")?;
    assert!(ranged.block.stmts.iter().any(|statement| {
        matches!(
            &statement.kind,
            bir::StatementKind::Call { args, .. }
                if matches!(args.first(), Some(bir::ArgumentElement::One(bir::Operand::Place(read)))
                    if read.fact == bir::OwnershipFact::Move || read.last_use)
        )
    }));
    Ok(())
}

/// Prove every admitted observation shape borrows its plain parameter while the corresponding store, escape, capture
/// or item-taking shape retains owned access. Unrelated aggregates, results, closures and matches do not weaken an
/// otherwise read-only body.
fn assert_complete_shared_read_shapes() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Native:\n    value: int\ndef narrowed(value: str | None) -> str:\n    if value is not None:\n        return value.upper()\n    return \"missing\"\ndef discarded(item: Native) -> int:\n    _ = item\n    return 1\ndef formatted(item: Native) -> str:\n    return f\"{item:?}\"\ndef stored(item: Native) -> list[Native]:\n    return [item]\ndef compared(value: str) -> bool:\n    return value == \"chosen\"\ndef returned(value: str) -> str:\n    return value\ndef matched(item: Result[Native, str]) -> int:\n    match item:\n        Ok(_) => return 1\n        Err(_) => return 0\ndef match_take(item: Result[Native, str]) -> Native:\n    match item:\n        Ok(value) => return value\n        Err(_) => return Native(value=0)\ndef indexed(items: list[Native]) -> int:\n    return items[0].value\ndef index_take(items: list[Native]) -> Native:\n    return items[0]\ndef iterated(items: list[Native]) -> int:\n    mut total = 0\n    for item in items:\n        total += item.value\n    return total\ndef loop_take(items: list[Native]) -> Native:\n    for item in items:\n        return item\n    return Native(value=0)\ndef unrelated(item: Native) -> int:\n    values = [1, 2]\n    callback: Callable[int, int] = (value) => value + 1\n    result: Result[int, str] = Ok(callback(values[0]))\n    match result:\n        Ok(_) => pass\n        Err(_) => pass\n    return item.value\ndef captured(item: Native) -> int:\n    callback: () -> int = () => item.value\n    return callback()\n";
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(std::slice::from_mut(&mut module), &[]);

    for name in ["formatted", "compared", "matched", "indexed", "iterated", "unrelated"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing admitted shared-read shape")?;
        assert_eq!(
            body.params[0].input_access,
            incan_semantics_core::IncanInputAccess::InferredShared,
            "{name}: {body:#?}"
        );
    }
    for name in [
        "stored",
        "returned",
        "match_take",
        "index_take",
        "discarded",
        "narrowed",
        "loop_take",
        "captured",
    ] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing refused shared-read shape")?;
        assert_eq!(
            body.params[0].input_access,
            incan_semantics_core::IncanInputAccess::Inferred,
            "{name}: {body:#?}"
        );
    }
    Ok(())
}

/// Prove a read lent to one input and moved to another of the same call clones for the move, so the callee's loan
/// never reads moved storage.
fn assert_lent_and_moved_read() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Item:\n    count: int\ndef both(kept: Item, taken: Item) -> Item:\n    total = kept.count\n    return taken\ndef caller(item: Item) -> Item:\n    return both(item, item)\n";
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(std::slice::from_mut(&mut module), &[]);
    let both = module
        .bodies
        .iter()
        .find(|body| body.name == "both")
        .ok_or("missing lending callee")?;
    assert_eq!(
        both.params[0].input_access,
        incan_semantics_core::IncanInputAccess::InferredShared,
        "{both:#?}"
    );
    assert_eq!(
        both.params[1].input_access,
        incan_semantics_core::IncanInputAccess::Inferred,
        "{both:#?}"
    );
    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "caller")
        .ok_or("missing lending caller")?;
    let facts = caller.block.stmts.iter().find_map(|statement| match &statement.kind {
        bir::StatementKind::Call { args, .. } => Some(
            args.iter()
                .filter_map(|argument| match argument {
                    bir::ArgumentElement::One(bir::Operand::Place(read)) => Some(read.fact),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        ),
        _ => None,
    });
    assert_eq!(
        facts,
        Some(vec![bir::OwnershipFact::Borrow, bir::OwnershipFact::Clone]),
        "{caller:#?}"
    );
    Ok(())
}

/// Verify own-body equality adaptation and its separate owner facet against the frozen module mutation, including
/// fields omitted by the publication codec. The standard slot is a selected native contract in this adapter test.
fn assert_equality_contract() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Compare:\n    def equal(self, other: Self) -> bool: ...\nmodel Shape with Compare:\n    value: int\n    def equal(self, other: Self) -> bool:\n        return self.value == other.value\n";
    let (_, _, mut candidate) = checked_seed(source)?;
    let implementation = candidate
        .trait_implementations
        .first_mut()
        .ok_or("missing comparison implementation")?;
    implementation.method.declaration_name = "__eq__".to_owned();
    implementation.method.origin = incan_semantics_core::SymbolOrigin::Module(vec![
        "std".to_owned(),
        "derives".to_owned(),
        "comparison".to_owned(),
    ]);
    let mut expected = candidate.clone();
    super::legacy_source_comparison::retain_equality_borrows(&mut expected);
    super::source_comparison::retain_equality_borrows(&mut candidate);
    assert_same_module(&expected, &candidate)?;
    assert!(candidate.nominal_declarations[0].comparison_equality.is_some());
    Ok(())
}

/// Compare the complete native representation as well as publication bytes. Derived Debug includes invocation-only
/// fields that serde intentionally skips, and its ordered containers retain every index, name and source coordinate.
/// The first differing encoded structural line is reported instead of hiding differences with normalization.
fn assert_same_module(
    expected: &bir::BodyIrModule,
    actual: &bir::BodyIrModule,
) -> Result<(), Box<dyn std::error::Error>> {
    let before = format!("{expected:#?}");
    let after = format!("{actual:#?}");
    capture_module_baseline(expected, actual, &before, &after)?;
    if expected != actual || before.as_bytes() != after.as_bytes() {
        let mut left = before.lines();
        let mut right = after.lines();
        let mut line = 0;
        loop {
            let pair = (left.next(), right.next());
            if pair == (None, None) {
                return Err("native values differ despite equal Debug encodings".into());
            }
            line += 1;
            if pair.0 != pair.1 {
                return Err(format!("first structural difference at line {line}: {pair:?}").into());
            }
        }
    }
    assert_eq!(postcard::to_stdvec(expected)?, postcard::to_stdvec(actual)?);
    Ok(())
}

/// Prove the settled read boundary feeds scalar function seeds without another body's facts. The expected module
/// is constructed independently; empty declaration/layout inputs are justified by this fixture, not a general producer.
fn assert_settled_function_seeds() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def chosen(value: int) -> int:\n    if value > 0:\n        return value + 1\n    return value - 1\ndef sibling(value: str) -> str:\n    return value\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let expected = super::build_module_seed(&program, &[], checker.type_info(), &context);
    let mut actual = expected.clone();
    actual.bodies.clear();
    let identity = super::body_ir_module_identity(&[]);
    let mut declaration_reads = 0;
    for declaration in &program.declarations {
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return Err("the seed fixture must contain only scalar functions".into());
        };
        let checked = checker
            .checked_body(declaration.span)
            .ok_or("missing settled function")?;
        assert!(!checked.is_refused());
        let binding = checker
            .declaration_view()
            .function(declaration.span)
            .ok_or("missing signature")?;
        declaration_reads += 1;
        let declaration_owner = checker
            .checked_declaration(declaration.span)
            .ok_or("missing checked declaration owner")?;
        let references = checked.reference_inputs(declaration_owner)?;
        let mut info = TypeCheckInfo {
            expressions: checked.expressions().clone(),
            references,
            calls: checked.calls().clone(),
            protocols: checked.protocols().clone(),
            rust: checked.rust().clone(),
            traits: checked.traits().clone(),
            c_abi: checked.c_abi().clone(),
            ..TypeCheckInfo::default()
        };
        info.declarations
            .function_bindings_by_span
            .insert(declaration.span, binding.clone());
        if let Some(canonical) = &binding.identity {
            info.declarations
                .declaration_identities
                .insert(declaration.span, canonical.clone());
        }
        let facts = super::BodyIrLoweringFacts {
            published_methods: &[],
            model_constants: &Default::default(),
            local_enum_declarations: &Default::default(),
            published_callable_returns: &Default::default(),
            published_callable_defaults: &Default::default(),
            published_newtype_returns: &Default::default(),
            published_storage_types: &Default::default(),
            published_constants: &Default::default(),
            type_info: &info,
            runtime_type_conflicts: &Default::default(),
            retain_nominal_carriers: true,
            function_default_sources: &Default::default(),
            local_function_declarations: &Default::default(),
            local_nongeneric_functions: &Default::default(),
            local_receiver_methods: &[],
            local_trait_partials: &[],
            local_nominal_declarations: &Default::default(),
            local_fieldless_enum_declarations: &Default::default(),
            local_value_enum_declarations: &Default::default(),
            module_identity: &identity,
            registry_package: None,
            provider_operations: &providers,
            rust_module: None,
        };
        let mut body = super::lower_function_body(function, declaration.span, &facts);
        super::representation_facts::retain(&mut body);
        actual.bodies.push(body);
    }
    let first = program.declarations.first().ok_or("missing first owner")?;
    let second = program.declarations.get(1).ok_or("missing second owner")?;
    let body = checker.checked_body(first.span).ok_or("missing first body")?;
    let other = checker
        .checked_declaration(second.span)
        .ok_or("missing second declaration")?;
    assert!(matches!(
        body.reference_inputs(other),
        Err(CheckedBodyInputError::DifferentOwner)
    ));
    assert!(matches!(
        body.reference_inputs(body),
        Err(CheckedBodyInputError::WrongOwnerRole)
    ));
    assert_refused_reference_inputs()?;
    assert_eq!(actual.bodies.len(), 2);
    assert_same_module(&expected, &actual)?;
    eprintln!(
        "settled-input-fixture\tbodies={}\tselected_signatures={declaration_reads}",
        actual.bodies.len()
    );
    Ok(())
}

/// Retain both complete native and publication streams when the qualification harness requests evidence. The
/// sequence identifies comparisons only; it is not an evaluation counter or a persistent compiler authority.
fn capture_module_baseline(
    expected: &bir::BodyIrModule,
    actual: &bir::BodyIrModule,
    before: &str,
    after: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COMPARISON: AtomicUsize = AtomicUsize::new(0);
    let Some(directory) = std::env::var_os("INCAN_BODY_IR_BASELINE_DIR") else {
        return Ok(());
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory)?;
    let sequence = COMPARISON.fetch_add(1, Ordering::Relaxed);
    std::fs::write(directory.join(format!("{sequence:04}-expected.native.txt")), before)?;
    std::fs::write(directory.join(format!("{sequence:04}-actual.native.txt")), after)?;
    std::fs::write(
        directory.join(format!("{sequence:04}-expected.publication.bin")),
        postcard::to_stdvec(expected)?,
    )?;
    std::fs::write(
        directory.join(format!("{sequence:04}-actual.publication.bin")),
        postcard::to_stdvec(actual)?,
    )?;
    Ok(())
}

/// A refused body remains a semantic predecessor refusal; the native read adapter cannot turn it into executable
/// inputs even when its own declaration's signature and default checks succeeded.
fn assert_refused_reference_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def invalid(value: int) -> int:\n    return \"wrong\"\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_err());
    let declaration = program.declarations.first().ok_or("missing refused owner")?;
    let body = checker.checked_body(declaration.span).ok_or("missing refused body")?;
    let owner = checker
        .checked_declaration(declaration.span)
        .ok_or("missing refused declaration")?;
    assert!(matches!(
        body.reference_inputs(owner),
        Err(CheckedBodyInputError::RefusedOwner)
    ));
    Ok(())
}

/// Extend the existing representation baseline to nonempty declaration defaults, nominal construction, calls and
/// nested lexical closures. The missing-owner control proves a comparison cannot silently skip an eligible body.
fn assert_free_function_input_baseline() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Value:\n    number: int = 1\n    offset: int = 2\ndef increment(value: int) -> int:\n    return value + 1\ndef chosen(value: int = increment(2)) -> int:\n    item = Value(number=value)\n    callback: Callable[int, int] = (number) => number + item.number\n    return callback(value)\ndef sibling() -> int:\n    values = [1, 2]\n    mut total = 0\n    for value in values:\n        total += increment(value)\n    return total\ndef refined() -> int:\n    mut values = []\n    values.append(1)\n    return len(values)\ndef defaults(prefix: int, items: list[float] = []) -> int:\n    return prefix\ndef residual() -> int:\n    callback = partial defaults(prefix=1)\n    return callback()\npreset = partial defaults(prefix=2)\ndef preset_use() -> int:\n    return preset()\ndef constructor_partial() -> int:\n    make = partial Value(number=3)\n    return make().number\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let owners = checker.body_ir_baseline_inputs();
    let mut info = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
        include_inline_tests: false,
    };
    let reports = tempfile::tempdir()?;
    std::fs::write(reports.path().join("interrupted.tmp"), b"{\"equal\":")?;
    assert!(super::verify_free_function_inputs(&program, &[], &info, &context, reports.path()).is_err());
    info.body_ir_baseline_inputs = Some(owners);
    super::verify_free_function_inputs(&program, &[], &info, &context, reports.path())?;
    let mut comparisons = 0;
    for entry in std::fs::read_dir(reports.path())? {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "jsonl") {
            for line in std::fs::read_to_string(path)?.lines() {
                let row: serde_json::Value = serde_json::from_str(line)?;
                assert_eq!(row["equal"], true, "{row}");
                comparisons += 1;
            }
        }
    }
    assert_eq!(comparisons, 16);
    let module = super::build_body_ir_module(&program, &[], &info, &context);
    let expected = module.bodies.first().ok_or("missing comparator control body")?;
    let mut changed = expected.clone();
    changed.native_attributes.push(bir::NativeAttribute {
        path: "selected::attribute".to_owned(),
        arguments: Vec::new(),
    });
    assert_eq!(postcard::to_stdvec(expected)?, postcard::to_stdvec(&changed)?);
    let failure = super::free_function_baseline::record_comparison(expected, &changed, "negative", reports.path())
        .err()
        .ok_or("free-function baseline omitted a native-only field")?;
    assert!(failure.to_string().contains("native_attributes"));
    Ok(())
}
/// The actual checked generic metadata body must survive publication, not merely a fabricated method target.
#[test]
fn generic_field_metadata_source_publishes_a_covered_body() -> Result<(), Box<dyn std::error::Error>> {
    let metadata_members = "pub def field_names[T](value: T) -> list[str]:\n    return [str(field.name) for field in value.__fields__()]\n";
    let source =
        format!("pub def count_fields[T](value: T) -> int:\n    return len(value.__fields__())\n{metadata_members}");
    let (facts, module) = checked_metadata_seed(&source)?;
    assert_metadata_publication(&facts, &module)?;
    let source = format!(
        "model FieldInfo:\n    pub own_value: int\npub def count_fields[T](value: T) -> int:\n    return len(value.__fields__())\npub def local_value(value: FieldInfo) -> int:\n    return value.own_value\n{metadata_members}"
    );
    let (facts, module) = checked_metadata_seed(&source)?;
    assert_metadata_publication(&facts, &module)?;
    let local = module
        .bodies
        .iter()
        .find(|body| body.name == "local_value")
        .ok_or("missing local FieldInfo consumer")?;
    let super::IncanType::Named(key) = &local.params[0].ty else {
        return Err("local FieldInfo lost its nominal carrier".into());
    };
    let owner = local.named_type_identities.get(key).ok_or("missing local owner")?;
    assert!(
        matches!(&owner.origin, incan_semantics_core::SymbolOrigin::Package { library, .. } if library == "metadata")
    );
    Ok(())
}

/// Inspect the source-produced body and its published coverage together so omitted carrier facts cannot pass.
fn assert_metadata_publication(
    facts: &TypeCheckInfo,
    module: &bir::BodyIrModule,
) -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::executable_representation::{DeclarationCoverage, SurfaceReader, build_surface};
    use std::collections::BTreeSet;

    assert!(
        facts
            .declarations
            .body_ir_native_type_identities
            .values()
            .any(|identity| identity.declaration_name == "FieldInfo")
    );
    let mut helpers = 0;
    for body in module
        .bodies
        .iter()
        .filter(|body| matches!(body.name.as_str(), "count_fields" | "field_names"))
    {
        helpers += 1;
        let identity = body.canonical.clone().ok_or("missing canonical helper identity")?;
        let public = BTreeSet::from([identity]);
        let bytes = build_surface(
            std::slice::from_ref(module),
            "metadata",
            "0.1.0",
            &public,
            &BTreeSet::new(),
        )?;
        let reader = SurfaceReader::open(&bytes)?;
        let (_, coverage) = reader
            .index()
            .declarations
            .iter()
            .find(|(identity, _)| identity.declaration_name == body.name)
            .ok_or("missing public helper coverage")?;
        assert!(
            matches!(coverage, DeclarationCoverage::Covered { .. }),
            "actual coverage: {coverage:?}; native identities: {:#?}; checked body: {body:#?}",
            facts.declarations.body_ir_native_type_identities
        );
    }
    assert_eq!(helpers, 2, "missing checked metadata helpers");
    Ok(())
}

/// Admit the real SDK metadata declaration before checking a source helper that obtains it without an import.
fn checked_metadata_seed(source: &str) -> Result<(TypeCheckInfo, bir::BodyIrModule), Box<dyn std::error::Error>> {
    use crate::api_metadata::{
        CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, collect_checked_api_metadata,
        materialize_checked_api_public_namespaces,
    };
    use crate::library_exports::collect_checked_public_exports;
    use crate::library_manifest::LibraryManifest;
    use crate::loaf_index::LoafIndex;
    use std::sync::Arc;

    let sdk = include_str!("../../../../stdlib/core/src/reflection.incn");
    let tokens = crate::lexer::lex(sdk).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut declaring = TypeChecker::new();
    declaring.set_current_package_identity(Some("incan_stdlib_core".into()));
    declaring.set_current_module_path(Some(vec!["reflection".into()]));
    declaring
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let module_path = vec!["reflection".to_owned()];
    let exports = collect_checked_public_exports(&program, &declaring);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![collect_checked_api_metadata(&program, &declaring, module_path.clone())],
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api)?;
    let mut manifest = LibraryManifest::new("incan_stdlib_core", "0.6.0");
    manifest.contract_metadata.identity_graph.extend_checked_api_exports(
        "incan_stdlib_core",
        &api,
        &[(module_path, exports)],
    )?;
    manifest.contract_metadata.api = Some(api);
    let directory = tempfile::tempdir()?;
    std::fs::write(
        directory.path().join("incan_stdlib_core.incnlib"),
        manifest.to_json_string()?,
    )?;
    let entry = LoafIndex::standard("incan_stdlib_core", directory.path())?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("metadata".into()));
    checker.set_current_module_path(Some(Vec::new()));
    checker.set_loaves(Arc::new(LoafIndex::new([entry])?));
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.into_type_info();
    let providers = ProviderOperationCatalog::new();
    let context = BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: Some("metadata"),
        include_inline_tests: false,
    };
    let module = super::build_module_seed(&program, &[], &facts, &context);
    Ok((facts, module))
}
