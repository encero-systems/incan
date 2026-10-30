//! Exact representation-operation comparisons over the same checked seed. These are migration oracles, not
//! producers: only this harness assembles a whole module to compare the independent native operations.

use super::{
    BodyIrContext, ProviderOperationCatalog, bir, legacy_opaque_returns, legacy_union_layouts, opaque_returns,
    union_layouts,
};
use crate::typechecker::{TypeCheckInfo, TypeChecker};

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
    assert_shared_loans()?;
    assert_equality_contract()?;
    let mut changed = seed.clone();
    changed.bodies[0].native_attributes.push(bir::NativeAttribute {
        path: "selected::attribute".to_owned(),
        arguments: Vec::new(),
    });
    assert_eq!(postcard::to_stdvec(&seed)?, postcard::to_stdvec(&changed)?);
    let difference = assert_same_module(&seed, &changed)
        .err()
        .ok_or("oracle omitted an invocation-only field")?;
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
    };
    let seed = super::build_module_seed(&program, &[], &facts, &context);
    Ok((program, facts, seed))
}

/// Compare the ordering-sensitive private-loan fixed point, including a multi-step forwarding edge and an escaped
/// callable. Synthetic native identity is adapter input; the checker still supplies all executable operations.
fn assert_shared_loans() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Native:\n    value: int\ndef leaf(items: Native) -> int:\n    return 1\ndef forward(items: Native) -> int:\n    return leaf(items)\ndef escaped(items: Native) -> int:\n    return 2\ndef use() -> int:\n    callback = escaped\n    return callback(Native(value=1))\n";
    let (program, _, mut candidate) = checked_seed(source)?;
    for body in &mut candidate.bodies {
        for identity in std::sync::Arc::make_mut(&mut body.named_type_identities)
            .values_mut()
            .filter(|identity| identity.declaration_name == "Native")
        {
            identity.kind = super::SemanticSourceTargetKind::RustItem;
            identity.origin = incan_semantics_core::SymbolOrigin::RustCrate(vec!["native".to_owned()]);
        }
    }
    let mut expected = candidate.clone();
    super::legacy_shared_reads::retain(&mut expected, &program);
    super::shared_reads::retain(&mut candidate, &program);
    assert_same_module(&expected, &candidate)?;
    for name in ["leaf", "forward"] {
        let body = candidate
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing loan body")?;
        assert!(
            matches!(body.params[0].ty, super::IncanType::Ref(_)),
            "{name}: {body:#?}"
        );
    }
    let escaped = candidate
        .bodies
        .iter()
        .find(|body| body.name == "escaped")
        .ok_or("missing escaped body")?;
    assert!(matches!(escaped.params[0].ty, super::IncanType::Named(_)));
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
