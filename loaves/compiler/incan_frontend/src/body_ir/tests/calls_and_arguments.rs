//! Call-site argument binding (#1158, #1159): named and out-of-order construction, omitted defaults as explicit slots,
//! source-local model and enum member identities, mixed positional / named arguments, explicit type arguments, overload
//! selection, rest parameters, inherited-field layout, spread arguments and spread aggregate elements, the refusals for
//! unbound constructions and unknown names, and a local callable shadowing the intrinsic `Ok` constructor.

use super::*;

#[test]
fn a_local_callable_named_ok_shadows_the_intrinsic_result_constructor() -> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Failure:\n  Shadowed\n\ndef main(Ok: (int) -> Result[int, Failure]) -> Result[int, Failure]:\n  return Ok(42)\n";
    let module = build(source, &["m", "result_constructor_shadow"])?;
    let main = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("the main body must be retained")?;
    let call = single_call(main)?;
    let bir::StatementKind::Call {
        callee: bir::Callee::Function(bir::CallableTarget::Local(target)),
        ..
    } = call
    else {
        return Err("a callable parameter named Ok must remain a local Body-IR call".into());
    };
    let parameter = main
        .param_locals
        .first()
        .ok_or("the callable parameter must retain a local id")?;
    assert_eq!(target.operand.place, bir::Place::from_local(*parameter));
    Ok(())
}

/// Return the single `Call` statement in `body`, failing when there is not exactly one.
///
/// The #1158 tests assert on one call's resolved binding, so a body that lowered to several calls would make a
/// positional "first call" assertion silently test the wrong statement.
fn single_call(body: &bir::Body) -> Result<&bir::StatementKind, Box<dyn std::error::Error>> {
    let calls: Vec<&bir::StatementKind> = body
        .block
        .stmts
        .iter()
        .map(|stmt| &stmt.kind)
        .filter(|kind| matches!(kind, bir::StatementKind::Call { .. }))
        .collect();
    match calls.as_slice() {
        [only] => Ok(only),
        other => Err(format!("expected exactly one call statement, found {}", other.len()).into()),
    }
}

/// Return the resolved argument binding carried by a call statement's callee.
fn call_binding(kind: &bir::StatementKind) -> Result<&bir::ArgumentBinding, Box<dyn std::error::Error>> {
    let bir::StatementKind::Call { callee, .. } = kind else {
        return Err("not a call statement".into());
    };
    match callee {
        bir::Callee::Function(bir::CallableTarget::Named(target)) => Ok(&target.binding),
        bir::Callee::Function(bir::CallableTarget::Local(target)) => Ok(&target.binding),
        bir::Callee::Method(target) => Ok(&target.binding),
        bir::Callee::Helper(_) => Err("a helper call carries no declared argument binding".into()),
        // A provider operation's declaration slots are described by its plan's inputs, not by an argument binding.
        bir::Callee::ProviderOperation(_) => Err("a provider operation carries its own input facts".into()),
    }
}

/// A resolved binding's two lists: the per-operand records, and the slots left to a default.
type ResolvedBindingParts<'a> = (&'a [bir::BoundArgument], &'a [usize]);

/// Return a call's resolved argument binding, failing when the call recorded no declared-slot binding.
///
/// Insisting on [`bir::ArgumentBinding::Resolved`] is the point: a test that accepted `UnresolvedPositional` would
/// silently pass against an implementation that stopped binding named arguments.
fn resolved_binding(kind: &bir::StatementKind) -> Result<ResolvedBindingParts<'_>, Box<dyn std::error::Error>> {
    match call_binding(kind)? {
        bir::ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        } => Ok((arguments, defaulted_slots)),
        bir::ArgumentBinding::UnresolvedPositional => {
            Err("expected a resolved declared-slot binding, found an unresolved positional call".into())
        }
    }
}

#[test]
fn named_construction_lowers_to_a_constructor_aggregate_with_a_resolved_field_binding()
-> Result<(), Box<dyn std::error::Error>> {
    // The canonical README spelling. Before #1158 this was the *only* accepted construction spelling and it
    // lowered to `unsupported`, so no nominal value was representable in Body IR at all.
    let source = "model P:\n  x: int\n  y: int\n\ndef make() -> P:\n  return P(x=1, y=2)\n";
    let module = build(source, &["m", "ctor"])?;
    let snapshot = module.render_snapshot();
    assert_eq!(
        snapshot,
        build(source, &["m", "ctor"])?.render_snapshot(),
        "lowering must be deterministic"
    );

    assert!(
        !snapshot.contains("unsupported("),
        "named construction must lower to real Body IR: {snapshot}"
    );
    assert!(
        snapshot.contains("constructor(P)[const(1), const(2)]"),
        "construction must lower to a constructor aggregate in declared field order: {snapshot}"
    );
    Ok(())
}

#[test]
fn out_of_order_named_construction_binds_by_field_and_records_written_order() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "model P:\n  x: int\n  y: int\n\ndef make() -> P:\n  return P(y=2, x=1)\n";
    let module = build(source, &["m", "ctor_order"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "out-of-order named construction must lower: {snapshot}"
    );
    // Operands follow declared field order (`x` then `y`) even though the source wrote `y` first, and the
    // written order is recorded rather than discarded.
    assert!(
        snapshot.contains("constructor(P) written=[1, 0][const(1), const(2)]"),
        "field binding must reorder operands while preserving the written order fact: {snapshot}"
    );
    Ok(())
}

#[test]
fn construction_records_an_omitted_field_default_as_an_explicit_slot() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model P:\n  x: int\n  y: int = 5\n\ndef make() -> P:\n  return P(x=1)\n";
    let module = build(source, &["m", "ctor_default"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "construction omitting a defaulted field must lower: {snapshot}"
    );
    // The default's *computation* stays owned by the declaration; the call site records only that slot 1 took it.
    assert!(
        snapshot.contains("constructor(P) defaults=[1][const(1)]"),
        "an omitted field must be recorded as a defaulted slot, not left implicit: {snapshot}"
    );
    Ok(())
}

/// Retain the exact local model layout a direct executor needs instead of treating a constructor spelling as an
/// identity.
#[test]
fn source_local_model_construction_retains_its_declaration_identity_and_canonical_field_layout()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "model Pair:\n  pub left: int\n  right: int\n\ndef main() -> int:\n  pair = Pair(right=2, left=40)\n  return pair.left + pair.right\n";
    let module = build(source, &["m", "nominal_identity"])?;
    let declaration = match module.nominal_declarations.as_slice() {
        [declaration] => declaration,
        declarations => {
            return Err(format!("expected one retained local model declaration, found {declarations:?}").into());
        }
    };
    assert_eq!(declaration.name, "Pair");
    assert_eq!(declaration.fields, vec!["left", "right"]);
    assert_eq!(declaration.type_parameter_count, 0);
    assert_eq!(declaration.field_public, vec![true, false]);
    assert!(!declaration.public);
    assert!(!declaration.has_field_defaults);
    assert_eq!(declaration.derives, incan_lang::lang::derives::plain_model_derives());
    assert!(module.is_well_formed_nominal_declaration(declaration));
    assert_eq!(
        declaration.canonical.kind,
        SemanticSourceTargetKind::Model,
        "the retained physical declaration must carry its compiler-owned model identity"
    );
    assert_eq!(declaration.canonical.declaration_name, "Pair");
    assert_eq!(declaration.field_identities.len(), declaration.fields.len());
    assert_eq!(
        declaration
            .field_identities
            .iter()
            .map(|identity| (identity.kind.clone(), identity.declaration_name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (SemanticSourceTargetKind::Field, "left"),
            (SemanticSourceTargetKind::Field, "right")
        ],
        "the physical field layout must retain the checked member identity for every slot"
    );

    let body = body_named(&module, "main")?;
    let target = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Aggregate(bir::AggregateKind::Constructor(target), _),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("the local model construction must lower as a constructor aggregate")?;
    assert_eq!(target.name, "Pair");
    assert_eq!(target.canonical.as_ref(), Some(&declaration.canonical));
    assert_eq!(
        target.direct_declaration_id.as_ref(),
        Some(&declaration.direct_declaration_id)
    );
    assert_eq!(
        target.canonical_field_layout.as_deref(),
        Some(declaration.fields.as_slice()),
        "the constructor must retain the checked layout independently from the mutable module declaration"
    );
    let bir::ArgumentBinding::Resolved {
        arguments,
        defaulted_slots,
    } = &target.binding
    else {
        return Err("local model construction must retain its resolved field binding".into());
    };
    assert!(defaulted_slots.is_empty());
    assert_eq!(
        arguments
            .iter()
            .map(|argument| (argument.slot, argument.written_position))
            .collect::<Vec<_>>(),
        vec![(0, 1), (1, 0)],
        "constructor operands retain declaration slots while written positions retain source evaluation order"
    );
    Ok(())
}

/// Retain the exact local value-enum member selected by source lowering rather than recovering it from a
/// qualified spelling in a direct runtime.
#[test]
fn source_local_value_enum_member_retains_exact_enum_and_variant_identities() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "enum HttpStatus(int):\n  Ok = 200\n  NotFound = 404\n\ndef main() -> int:\n  return HttpStatus.NotFound.value()\n";
    let module = build(source, &["m", "value_enum_identity"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("value_enum HttpStatus id=decl:m::value_enum_identity#decl."),
        "the module must retain the source-local enum declaration identity: {snapshot}"
    );
    assert!(
        snapshot.contains("variant NotFound id=decl:m::value_enum_identity#decl."),
        "the module must retain the source-local member declaration identity: {snapshot}"
    );
    assert!(
        snapshot.contains("value_enum_variant(HttpStatus::NotFound"),
        "the member expression must lower to an identity-bearing rvalue instead of an external field place: {snapshot}"
    );
    let declaration = module
        .value_enum_declarations
        .first()
        .ok_or("the value enum must retain its declaration record")?;
    let variant = declaration
        .variants
        .iter()
        .find(|variant| variant.name == "NotFound")
        .ok_or("the value enum must retain NotFound")?;
    assert_eq!(declaration.canonical.kind, SemanticSourceTargetKind::Enum);
    assert_eq!(variant.canonical.kind, SemanticSourceTargetKind::Variant);
    let selected = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::ValueEnumVariant(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("the selected member must retain its value-enum target")?;
    assert_eq!(selected.enum_canonical, declaration.canonical);
    assert_eq!(selected.variant_canonical, variant.canonical);
    let body = body_named(&module, "main")?;
    let value_method = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("the value-enum scalar projection must lower as a method call")?;
    let canonical = value_method
        .canonical
        .as_ref()
        .ok_or("the checked value-enum method must retain its canonical target")?;
    assert_eq!(canonical.namespace, incan_semantics_core::SymbolNamespace::Member);
    assert_eq!(canonical.kind, SemanticSourceTargetKind::Method);
    assert_eq!(canonical.declaration_name, "value");
    assert_eq!(
        canonical.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["m".to_string(), "value_enum_identity".to_string()])
    );
    Ok(())
}

/// Retain the exact local fieldless normal-enum member selected by source lowering rather than treating a
/// qualified spelling as a value any backend may recover.
#[test]
fn source_local_fieldless_enum_member_retains_exact_enum_and_variant_identities()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Signal:\n  Ready\n  Stop\n\ndef main() -> bool:\n  return Signal.Ready == Signal.Stop\n";
    let module = build(source, &["m", "fieldless_enum_identity"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("fieldless_enum Signal id=decl:m::fieldless_enum_identity#decl."),
        "the module must retain the source-local enum declaration identity: {snapshot}"
    );
    assert!(
        snapshot.contains("variant Ready id=decl:m::fieldless_enum_identity#decl."),
        "the module must retain the source-local member declaration identity: {snapshot}"
    );
    assert!(
        snapshot.contains("fieldless_enum_variant(Signal::Ready"),
        "the member expression must lower to an identity-bearing rvalue instead of an external field place: {snapshot}"
    );
    let declaration = module
        .fieldless_enum_declarations
        .first()
        .ok_or("the fieldless enum must retain its declaration record")?;
    let selected = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::FieldlessEnumVariant(target),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("the selected member must retain its fieldless-enum target")?;
    let variant = declaration
        .variants
        .iter()
        .find(|variant| variant.name == selected.variant_name)
        .ok_or("the selected variant must exist in the retained registry")?;
    assert_eq!(selected.enum_canonical, declaration.canonical);
    assert_eq!(selected.variant_canonical, variant.canonical);
    Ok(())
}

#[test]
fn mixed_positional_and_named_call_arguments_bind_to_declared_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(1, b=2)\n";
    let module = build(source, &["m", "mixed"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a mixed positional/named call must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:add(const(1), const(2))"),
        "a mixed call binding in declaration order needs no slot map: {snapshot}"
    );
    // The rendered string alone would also match an implementation that ignored named binding entirely and
    // lowered arguments in written order, so assert the resolved binding itself.
    let (arguments, defaulted_slots) = resolved_binding(single_call(body_named(&module, "use")?)?)?;
    assert!(defaulted_slots.is_empty(), "nothing was omitted: {defaulted_slots:?}");
    assert_eq!(
        arguments
            .iter()
            .map(|argument| (argument.slot, argument.written_position))
            .collect::<Vec<_>>(),
        vec![(0, 0), (1, 1)],
        "`b=2` must resolve to declared slot 1 rather than being taken positionally: {arguments:?}"
    );
    Ok(())
}

#[test]
fn out_of_order_named_call_arguments_evaluate_in_written_source_order() -> Result<(), Box<dyn std::error::Error>> {
    // The effect-ordering contract: `g()` is written first, so it must be *called* first, even though its value
    // binds to the later declared parameter. A consumer executing operands in slot order would swap the effects.
    let source = "def f() -> int:\n  return 1\n\ndef g() -> int:\n  return 2\n\ndef add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(b=g(), a=f())\n";
    let module = build(source, &["m", "written_order"])?;
    let snapshot = module.render_snapshot();
    let use_body = body_named(&module, "use")?;
    let rendered = use_body.render_snapshot();

    let g_at = rendered.find("call fn:g(").ok_or("missing call to g")?;
    let f_at = rendered.find("call fn:f(").ok_or("missing call to f")?;
    assert!(
        g_at < f_at,
        "argument sub-expressions must be evaluated in written source order: {rendered}"
    );
    assert!(
        rendered.contains("written=[1, 0]"),
        "the written order must be recorded on the call, not merely implied by statement order: {rendered}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "no part of this program should refuse: {snapshot}"
    );
    Ok(())
}

#[test]
fn an_omitted_defaulted_argument_is_recorded_as_a_defaulted_slot() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(a: int, b: int = 2) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(1)\n";
    let module = build(source, &["m", "call_default"])?;
    let use_body = body_named(&module, "use")?;
    let (arguments, defaulted_slots) = resolved_binding(single_call(use_body)?)?;

    assert_eq!(
        defaulted_slots,
        [1],
        "an omitted default must be an explicit call-site fact: {defaulted_slots:?}"
    );
    assert_eq!(
        arguments.len(),
        1,
        "only the supplied argument gets an operand: {arguments:?}"
    );
    Ok(())
}

#[test]
fn an_omitted_interior_default_binds_without_compacting_later_arguments() -> Result<(), Box<dyn std::error::Error>> {
    // #1124 had to refuse this: a flat operand vector could not say that `9` fills slot 2 rather than slot 1.
    // The recorded binding is exactly that sparse argument map, so the call is now representable.
    let source = "def at(a: int, b: int = 2, c: int = 3) -> int:\n  return a + b + c\n\ndef use() -> int:\n  return at(1, c=9)\n";
    let module = build(source, &["m", "interior_default"])?;
    let snapshot = module.render_snapshot();
    let use_body = body_named(&module, "use")?;
    let (arguments, defaulted_slots) = resolved_binding(single_call(use_body)?)?;

    assert!(
        !snapshot.contains("unsupported("),
        "an interior default hole must now lower: {snapshot}"
    );
    assert_eq!(
        defaulted_slots,
        [1],
        "slot 1 takes its declared default: {defaulted_slots:?}"
    );
    assert_eq!(
        arguments.iter().map(|argument| argument.slot).collect::<Vec<_>>(),
        vec![0, 2],
        "the supplied operands must keep their real declaration slots: {arguments:?}"
    );
    Ok(())
}

#[test]
fn method_call_named_arguments_bind_after_the_borrowed_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class C:\n  def add(self, a: int, b: int) -> int:\n    return a + b\n\ndef use(c: C) -> int:\n  return c.add(b=2, a=1)\n";
    let module = build(source, &["m", "method_named"])?;
    let snapshot = module.render_snapshot();
    let use_body = body_named(&module, "use")?;
    let (arguments, _) = resolved_binding(single_call(use_body)?)?;

    assert!(
        !snapshot.contains("unsupported("),
        "a named method call must lower: {snapshot}"
    );
    // The receiver stays `args[0]` and is deliberately outside the binding, whose slots index the method's own
    // declared parameters.
    assert_eq!(
        arguments.iter().map(|argument| argument.slot).collect::<Vec<_>>(),
        vec![0, 1],
        "method argument slots must index declared parameters, not the receiver: {arguments:?}"
    );
    assert_eq!(
        arguments
            .iter()
            .map(|argument| argument.written_position)
            .collect::<Vec<_>>(),
        vec![1, 0],
        "the written order of `b=2, a=1` must survive the reorder into declaration order: {arguments:?}"
    );
    assert!(
        use_body.render_snapshot().contains("borrow(_0)"),
        "the receiver must still lower as a borrowed first argument: {snapshot}"
    );
    Ok(())
}

#[test]
fn explicit_call_site_type_arguments_survive_lowering() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def pick[T](v: T) -> T:\n  return v\n\ndef use() -> int:\n  return pick[int](1)\n";
    let module = build(source, &["m", "generic_call"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "an explicitly generic call must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:pick[int](const(1))"),
        "resolved call-site type arguments belong to the callee's identity: {snapshot}"
    );
    Ok(())
}

#[test]
fn explicit_method_call_type_arguments_survive_lowering() -> Result<(), Box<dyn std::error::Error>> {
    // The other half of `CallSiteGenerics`' canonical surface: `session.read_csv[Order](path)`. The typechecker
    // substitutes the receiver's generics before recording the signature, so the method's slots are already
    // concrete here and the resolved type argument still has to reach the callee.
    let source =
        "class S:\n  def read[T](self, v: T) -> T:\n    return v\n\ndef use(s: S) -> int:\n  return s.read[int](1)\n";
    let module = build(source, &["m", "generic_method"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "an explicitly generic method call must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call method:read[int]("),
        "a method call's resolved type arguments belong to its callee identity: {snapshot}"
    );
    Ok(())
}

#[test]
fn direct_and_local_named_binding_go_through_one_mechanism() -> Result<(), Box<dyn std::error::Error>> {
    // #1158's "one mechanism" criterion: the direct `Callee::Function` path and the #1124 local-callable path
    // must produce the same binding facts for the same spelling, not merely both succeed.
    // Deliberately an out-of-order spelling: its binding is *not* the identity, so this cannot be satisfied by
    // two independent mechanisms that merely agree on the trivial case, nor by a path that never bound at all.
    let direct = "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(b=2, a=1)\n";
    let local =
        "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  g = add\n  return g(b=2, a=1)\n";

    let direct_module = build(direct, &["m", "one_direct"])?;
    let local_module = build(local, &["m", "one_local"])?;
    let direct_binding = call_binding(single_call(body_named(&direct_module, "use")?)?)?;
    let local_binding = call_binding(single_call(body_named(&local_module, "use")?)?)?;

    assert_eq!(
        direct_binding, local_binding,
        "a direct call and a local-callable call must resolve one spelling identically"
    );
    let bir::ArgumentBinding::Resolved { arguments, .. } = direct_binding else {
        return Err("the shared mechanism must produce a resolved binding, not a positional fallback".into());
    };
    assert_eq!(
        arguments
            .iter()
            .map(|argument| (argument.slot, argument.written_position))
            .collect::<Vec<_>>(),
        vec![(0, 1), (1, 0)],
        "the shared binding must be the non-trivial one this spelling implies: {arguments:?}"
    );
    Ok(())
}

#[test]
fn an_overloaded_call_binds_against_the_declaration_the_typechecker_selected() -> Result<(), Box<dyn std::error::Error>>
{
    // Regression: `function_bindings` is keyed by bare name, so it holds only one of two same-name
    // declarations. Binding against the wrong overload's parameter *names* silently swaps the arguments --
    // a wrong answer where the previous refusal was at least honest.
    let source = "def pick(a: int, b: int) -> int:\n  return a - b\n\ndef pick(b: str, a: str) -> str:\n  return a\n\ndef use() -> int:\n  return pick(a=10, b=1)\n";
    let module = build(source, &["m", "overload"])?;
    let use_body = body_named(&module, "use")?;
    let rendered = use_body.render_snapshot();
    let (arguments, _) = resolved_binding(single_call(use_body)?)?;

    // The selected overload is `pick(a: int, b: int)`, so `a=10` fills slot 0 and `b=1` fills slot 1. Binding
    // against the *second* declaration would map `a` to slot 1 and emit the operands as `const(1), const(10)`.
    assert_eq!(
        arguments
            .iter()
            .map(|argument| (argument.slot, argument.written_position))
            .collect::<Vec<_>>(),
        vec![(0, 0), (1, 1)],
        "the call must bind against the overload the typechecker selected: {arguments:?}"
    );
    assert!(
        rendered.contains("call fn:pick(const(10), const(1))"),
        "operands must follow the selected overload's declaration order: {rendered}"
    );
    Ok(())
}

#[test]
fn an_overloaded_call_retains_the_typechecker_selected_same_module_declaration_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "def pick(a: int, b: int) -> int:\n  return a - b\n\ndef pick(b: str, a: str) -> str:\n  return a\n\ndef use() -> int:\n  return pick(a=10, b=1)\n";
    let module = build(source, &["m", "overload_identity"])?;
    let use_body = body_named(&module, "use")?;
    let bir::StatementKind::Call {
        callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
        ..
    } = single_call(use_body)?
    else {
        return Err("expected an identity-selected named function call".into());
    };
    let target_id = target
        .direct_call_id
        .as_ref()
        .ok_or("same-module overloaded call must retain a direct declaration identity")?;
    let selected = module
        .bodies
        .iter()
        .find(|body| body.direct_call_id == *target_id)
        .ok_or("direct call identity must select a Body-IR declaration")?;

    assert_eq!(selected.name, "pick");
    assert!(
        selected.render_snapshot().contains("local 0 a : int [param]"),
        "the direct identity must select the integer overload: {}",
        selected.render_snapshot()
    );
    Ok(())
}

#[test]
fn an_overload_set_that_changes_arity_does_not_refuse_a_valid_call() -> Result<(), Box<dyn std::error::Error>> {
    // The other half of the same defect: with the two-parameter declaration written first, a name-keyed lookup
    // could resolve `pick(1, 2)` against the one-parameter overload and refuse a call the typechecker accepted.
    let source = "def pick(a: int, b: int) -> int:\n  return a + b\n\ndef pick(a: str) -> str:\n  return a\n\ndef use() -> int:\n  return pick(1, 2)\n";
    let module = build(source, &["m", "overload_arity"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a call the typechecker accepted must not be refused by overload confusion: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_rest_parameter_callee_still_lowers_its_positional_arguments() -> Result<(), Box<dyn std::error::Error>> {
    // Variadics are a delivered language capability. Routing the direct path through the shared planner must not
    // silently narrow what Body IR represents; the call keeps lowering, it simply makes no declared-slot claim.
    let source = "def total(a: int, *xs: int) -> int:\n  return a\n\ndef use() -> int:\n  return total(1, 2, 3)\n";
    let module = build(source, &["m", "rest"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a positional call into a rest-parameter signature must still lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:total unbound(const(1), const(2), const(3))"),
        "a rest signature has no one-to-one declared slots, so the binding must say so: {snapshot}"
    );
    Ok(())
}

#[test]
fn argument_ownership_facts_are_sequenced_by_written_order_not_operand_index() -> Result<(), Box<dyn std::error::Error>>
{
    // The invariant `ArgumentBinding` documents: operands are reordered into declaration order, but their
    // ownership facts were decided in written order. Read left to right this vector moves `_0` and then clones
    // it; `written=[1, 0]` is what tells a consumer the clone happened first.
    let source = "def two(p: str, q: str) -> str:\n  return p + q\n\ndef use(a: str) -> str:\n  return two(q=a, p=a)\n";
    let module = build(source, &["m", "own_order"])?;
    let rendered = body_named(&module, "use")?.render_snapshot();

    assert!(
        rendered.contains("call fn:two written=[1, 0](move(_0, last_use), clone(_0))"),
        "ownership facts must stay sequenced by written order: {rendered}"
    );
    Ok(())
}

#[test]
fn class_construction_binds_inherited_fields_in_declared_layout_order() -> Result<(), Box<dyn std::error::Error>> {
    // Constructor ABI order puts the parent's fields first. A subclass construction must bind against that
    // flattened order, not against the subclass's own declarations alone.
    let source = "class Base:\n  a: int\n\nclass Sub extends Base:\n  b: int = 7\n\ndef make() -> Sub:\n  return Sub(b=1, a=2)\n";
    let module = build(source, &["m", "subclass"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "subclass construction must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("constructor(Sub) written=[1, 0][const(2), const(1)]"),
        "inherited fields come first in constructor layout order: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_construction_the_checker_declined_to_bind_is_refused_as_a_construction() -> Result<(), Box<dyn std::error::Error>>
{
    // A duplicate field leaves no recorded binding. Falling through to the direct-call path would refuse this as
    // a call to an unknown function, naming the wrong construct entirely.
    let source = "model P:\n  x: int = 1\n  y: int = 2\n\ndef make() -> P:\n  return P(x=1, x=2)\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "dup_field"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the typechecker must reject a duplicated field first"
    );
    assert!(
        snapshot.contains("construction of `P` with an unresolved field layout"),
        "a refused construction must be named as a construction: {snapshot}"
    );
    Ok(())
}

#[test]
fn an_argument_spread_is_refused_by_name_rather_than_as_a_generic_call_failure()
-> Result<(), Box<dyn std::error::Error>> {
    // The typechecker rejects these first; lowering must stay fail-closed and name the specific spelling, since
    // #1159 owns spread representation while #1158 owns named binding.
    let source =
        "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use(xs: List[int]) -> int:\n  return add(*xs)\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the source checker must reject an unmatched positional spread first"
    );
    assert!(
        snapshot.contains("positional argument spread"),
        "a spread must be refused as a spread, not as a named-argument failure: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_named_argument_with_no_matching_parameter_is_refused_by_name() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(a: int, b: int) -> int:\n  return a + b\n\ndef use() -> int:\n  return add(a=1, zz=2)\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["m", "bad_named"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the typechecker must reject an unknown parameter name first"
    );
    assert!(
        snapshot.contains("has no parameter `zz`"),
        "lowering must name the unresolvable parameter rather than accepting it silently: {snapshot}"
    );
    Ok(())
}

/// Preserve written argument order while shared list parameter reads retain caller storage.
#[test]
fn a_leading_spread_splices_before_its_fixed_elements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(xs: list[int]) -> None:\n  out = [*xs, 1]\n  return\n";
    let module = build(source, &["m", "spread_trailing"])?;
    let snapshot = module.render_snapshot();
    assert_eq!(
        snapshot,
        build(source, &["m", "spread_trailing"])?.render_snapshot(),
        "lowering must be deterministic"
    );

    assert!(
        !snapshot.contains("unsupported("),
        "a list spread must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("list[*clone(_0), const(1)]"),
        "the spread must keep its written position and carry its own ownership fact: {snapshot}"
    );
    Ok(())
}

/// Preserve written argument order while shared list parameter reads retain caller storage.
#[test]
fn a_trailing_spread_splices_after_its_fixed_elements() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(xs: list[int]) -> None:\n  out = [1, *xs]\n  return\n";
    let module = build(source, &["m", "spread_after"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a trailing spread must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("list[const(1), *clone(_0)]"),
        "a spread written last must stay last: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_statically_shaped_spread_binds_as_an_ordinary_fixed_arity_call() -> Result<(), Box<dyn std::error::Error>> {
    // `add(*(1, 2))` really is `add(1, 2)`: the typechecker proves the arity before lowering, so this belongs
    // on the declaration-slot path, not on the runtime-arity path a genuine spread needs. Its operands must
    // land in declared slots with no spread element and no `unbound` marker.
    for (label, source) in [
        (
            "tuple",
            "def add(a: int, b: int) -> int:\n  return a + b\n\ndef m() -> int:\n  return add(*(1, 2))\n",
        ),
        (
            "list",
            "def add(a: int, b: int) -> int:\n  return a + b\n\ndef m() -> int:\n  return add(*[1, 2])\n",
        ),
        (
            "dict",
            "def add(a: int, b: int) -> int:\n  return a + b\n\ndef m() -> int:\n  return add(**{\"a\": 1, \"b\": 2})\n",
        ),
    ] {
        let module = build(source, &["m", "shaped"])?;
        let rendered = body_named(&module, "m")?.render_snapshot();

        assert!(
            !rendered.contains("unsupported("),
            "{label} spread must lower: {rendered}"
        );
        assert!(
            rendered.contains("call fn:add(const(1), const(2))"),
            "a {label} spread with a proven shape must bind to declared slots: {rendered}"
        );
        assert!(
            !rendered.contains("unbound") && !rendered.contains("*const"),
            "a proven-shape spread must not be represented as runtime-arity: {rendered}"
        );
    }
    Ok(())
}

/// Preserve written argument order while shared list parameter reads retain caller storage.
#[test]
fn a_spread_with_no_proven_shape_stays_on_the_runtime_arity_path() -> Result<(), Box<dyn std::error::Error>> {
    // The contrast case for the test above: a list *variable* has no statically visible arity, so it must keep
    // its spread element rather than being expanded into slots that cannot be counted.
    let source = "def log(*items: int) -> None:\n  return\n\ndef m(xs: list[int]) -> None:\n  log(*xs)\n  return\n";
    let module = build(source, &["m", "unshaped"])?;
    let rendered = body_named(&module, "m")?.render_snapshot();

    assert!(
        rendered.contains("call fn:log unbound(*clone(_0))"),
        "an unproven spread must stay a spread element on the unresolved-arity path: {rendered}"
    );
    Ok(())
}

#[test]
fn a_standalone_keyword_spread_call_lowers() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def log(**fields: int) -> None:\n  return\n\ndef m(kw: dict[str, int]) -> None:\n  log(**kw)\n  return\n";
    let module = build(source, &["m", "kw_spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a keyword spread call must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:log unbound(**move(_0, last_use))"),
        "a keyword spread must render with its own marker and ownership fact: {snapshot}"
    );
    Ok(())
}

/// Preserve written argument order while shared list parameter reads retain caller storage.
#[test]
fn fixed_elements_keep_their_positions_on_both_sides_of_a_spread() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(xs: list[int]) -> None:\n  out = [1, *xs, 2]\n  return\n";
    let module = build(source, &["m", "spread_middle"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("list[const(1), *clone(_0), const(2)]"),
        "surrounding fixed elements must keep their positions relative to the spread: {snapshot}"
    );
    Ok(())
}

/// Preserve written argument order while shared list parameter reads retain caller storage.
#[test]
fn multiple_spreads_each_keep_their_own_element() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(xs: list[int], ys: list[int]) -> None:\n  out = [*xs, *ys]\n  return\n";
    let module = build(source, &["m", "spread_multi"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "multiple spreads must lower: {snapshot}"
    );
    // Counting `list[*` would pass against an implementation that silently dropped the second spread, since it
    // only observes that the aggregate *begins* with one. Assert the whole rendering so a dropped, reordered,
    // or differently-owned second spread all fail.
    assert!(
        snapshot.contains("list[*clone(_0), *clone(_1)]"),
        "both spreads must survive, in written order, each with its own ownership fact: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_dict_spread_keeps_its_written_position_before_an_overriding_key() -> Result<(), Box<dyn std::error::Error>> {
    // The override rule is what makes this meaningful: entries take effect in order and a later entry wins,
    // so the spread must stay *before* the literal key rather than being reordered or merged.
    let source = "def m(d: dict[str, int]) -> None:\n  out = {**d, \"a\": 1}\n  return\n";
    let module = build(source, &["m", "dict_spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a dict spread must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("dict[**move(_0, last_use), const(\"a\"): const(1)]"),
        "the spread must precede the overriding key and stay a distinct entry: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_dict_spread_after_a_literal_key_keeps_that_order() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def m(d: dict[str, int]) -> None:\n  out = {\"a\": 1, **d}\n  return\n";
    let module = build(source, &["m", "dict_spread_after"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("dict[const(\"a\"): const(1), **move(_0, last_use)]"),
        "written entry order decides precedence, so it must survive lowering: {snapshot}"
    );
    Ok(())
}

/// Preserve written argument order while shared list parameter reads retain caller storage.
#[test]
fn a_positional_call_spread_lowers_without_a_declared_slot_claim() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def log(*items: int) -> None:\n  return\n\ndef m(xs: list[int]) -> None:\n  log(*xs)\n  return\n";
    let module = build(source, &["m", "call_spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a call spread must lower: {snapshot}"
    );
    // A spread makes the arity a runtime fact, so the call must record no declared-slot binding rather than
    // asserting an identity slot map nobody checked.
    assert!(
        snapshot.contains("call fn:log unbound(*clone(_0))"),
        "a spread call must be unbound and carry the spliced source's ownership fact: {snapshot}"
    );
    Ok(())
}

/// Preserve written argument order while shared list parameter reads retain caller storage.
#[test]
fn a_mixed_call_keeps_every_written_argument_form() -> Result<(), Box<dyn std::error::Error>> {
    // The issue's combined form. A named argument here has no declared slot to bind to, because the spread
    // makes the arity a runtime fact -- but discarding its name would lose source information.
    let source = "def log(a: int, b: int, *items: int, **fields: int) -> None:\n  return\n\ndef m(xs: list[int], kw: dict[str, int]) -> None:\n  log(1, *xs, b=2, **kw)\n  return\n";
    let module = build(source, &["m", "call_mixed"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "the combined call form must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:log unbound(const(1), *clone(_0), b=const(2), **move(_1, last_use))"),
        "positional, spread, named, and keyword-spread arguments must each keep their written form and order: {snapshot}"
    );
    Ok(())
}

/// Preserve written argument order while shared list parameter reads retain caller storage.
#[test]
fn a_method_call_spread_lowers_after_the_borrowed_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class C:\n  def take(self, *items: int) -> None:\n    return\n\ndef m(c: C, xs: list[int]) -> None:\n  c.take(*xs)\n  return\n";
    let module = build(source, &["m", "method_spread"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a method call spread must lower: {snapshot}"
    );
    assert!(
        snapshot.contains("call method:take unbound(borrow(_0), *clone(_1))"),
        "the receiver stays args[0] and is never spliced: {snapshot}"
    );
    Ok(())
}

#[test]
fn set_literals_have_no_spread_spelling_to_represent() -> Result<(), Box<dyn std::error::Error>> {
    // Documenting a finding rather than adding surface: the source language rejects set spread in every
    // position, and `ast::Expr::Set` has no entry enum that could carry one. RFC 038 excludes it deliberately.
    for source in [
        "def m(xs: list[int]) -> None:\n  out = {*xs}\n  return\n",
        "def m(xs: list[int]) -> None:\n  out = {1, *xs}\n  return\n",
    ] {
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let errors = parser::parse(&tokens)
            .err()
            .ok_or("the parser must reject set spread rather than Body IR having to refuse it")?;
        // `is_err()` alone would pass for any unrelated parse failure, including one this fixture introduced.
        assert!(
            errors
                .iter()
                .any(|error| error.message.to_lowercase().contains("spread")),
            "the rejection must name spread rather than being any parse failure: {errors:?}"
        );
    }
    Ok(())
}

/// The ownership fact of the first argument of the first call to `callee` in `body`.
fn first_argument_fact(body: &bir::Body, callee: &str) -> Option<bir::OwnershipFact> {
    body.block.stmts.iter().find_map(|stmt| match &stmt.kind {
        bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            args,
            ..
        } if target.name == callee => match args.first() {
            Some(bir::ArgumentElement::One(bir::Operand::Place(place))) => Some(place.fact),
            _ => None,
        },
        _ => None,
    })
}

#[test]
fn a_mut_parameter_of_a_model_is_borrowed_mutably_not_copied_issue2022() -> Result<(), Box<dyn std::error::Error>> {
    // A `mut` parameter of a non-`Copy` type writes through to the caller, as the emitted route's `&mut Counter` does:
    // the call borrows the argument mutably, the callee does not own or drop it, and the caller still does.
    let source = "model Counter:\n  value: int\n\n\
                  def bump(mut c: Counter) -> None:\n  c.value = c.value + 1\n\n\
                  def main() -> None:\n  mut c = Counter(value=1)\n  bump(c)\n  println(c.value)\n";
    let module = build(source, &["m", "mut_params"])?;
    let main = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("missing main")?;
    assert_eq!(
        first_argument_fact(main, "bump"),
        Some(bir::OwnershipFact::MutBorrow),
        "{}",
        module.render_snapshot()
    );
    let bump = module
        .bodies
        .iter()
        .find(|body| body.name == "bump")
        .ok_or("missing bump")?;
    assert!(
        bump.params.first().is_some_and(|param| param.mutable),
        "{}",
        module.render_snapshot()
    );
    let param_local = bump.param_locals.first().copied().ok_or("missing parameter")?;
    let drops_param = bump
        .block
        .stmts
        .iter()
        .any(|stmt| matches!(stmt.kind, bir::StatementKind::Drop { local } if local == param_local));
    assert!(
        !drops_param,
        "the callee must not drop a parameter it only borrows: {}",
        module.render_snapshot()
    );
    Ok(())
}

#[test]
fn a_borrowed_mut_parameter_is_never_moved_out_issue2022() -> Result<(), Box<dyn std::error::Error>> {
    // The caller owns a `mut` collection parameter's storage and the callee only borrows it, so an owned read in the
    // callee is a clone, never a move: returning it or passing it by value leaves the caller's list in place, and
    // passing it on to another `mut` parameter reborrows it.
    let source = "def same(mut xs: List[int]) -> List[int]:\n  return xs\n\n\
                  def take(ys: List[int]) -> int:\n  return len(ys)\n\n\
                  def forward(mut xs: List[int]) -> int:\n  return take(xs)\n\n\
                  def bump(mut xs: List[int]) -> None:\n  xs[0] = 1\n\n\
                  def relay(mut xs: List[int]) -> None:\n  bump(xs)\n";
    let module = build(source, &["m", "borrowed_mut"])?;
    let body = |name: &str| {
        module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or(format!("missing {name}"))
    };
    let returned_fact = body("same")?.block.stmts.iter().find_map(|stmt| match &stmt.kind {
        bir::StatementKind::Return {
            value: Some(bir::Operand::Place(place)),
        } => Some(place.fact),
        _ => None,
    });
    assert_eq!(
        returned_fact,
        Some(bir::OwnershipFact::Clone),
        "{}",
        module.render_snapshot()
    );
    assert_eq!(
        first_argument_fact(body("forward")?, "take"),
        Some(bir::OwnershipFact::Clone),
        "{}",
        module.render_snapshot()
    );
    assert_eq!(
        first_argument_fact(body("relay")?, "bump"),
        Some(bir::OwnershipFact::MutBorrow),
        "{}",
        module.render_snapshot()
    );
    Ok(())
}

#[test]
fn a_mut_scalar_parameter_stays_a_copy_issue2022() -> Result<(), Box<dyn std::error::Error>> {
    // A `mut` scalar is a mutable local copy, as the emitted route's `mut n: i64` is: the caller's value is unchanged.
    let source =
        "def inc(mut n: int) -> None:\n  n = n + 1\n\ndef main() -> None:\n  mut n = 1\n  inc(n)\n  println(n)\n";
    let module = build(source, &["m", "mut_scalar"])?;
    let main = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("missing main")?;
    assert_eq!(
        first_argument_fact(main, "inc"),
        Some(bir::OwnershipFact::Copy),
        "{}",
        module.render_snapshot()
    );
    Ok(())
}

#[test]
fn a_static_method_call_is_a_direct_call_to_the_method_body() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Point:\n  x: int\n\n  def origin() -> Point:\n    return Point(x=0)\n\n  def shifted(by: int) -> Point:\n    return Point(x=by)\n\ndef main() -> int:\n  p = Point.shifted(by=3)\n  q = Point.origin()\n  return p.x + q.x\n";
    let module = build(source, &["m", "static_calls"])?;
    let main = body_named(&module, "main")?;
    let rendered = main.render_snapshot();
    assert!(
        !rendered.contains("unsupported("),
        "a static method call must lower: {rendered}"
    );
    let origin = body_named(&module, "origin")?;
    let shifted = body_named(&module, "shifted")?;
    let targets = named_targets(&module, "main");
    for (method, body) in [("shifted", shifted), ("origin", origin)] {
        let target = targets
            .iter()
            .find(|target| target.name.ends_with(method))
            .ok_or_else(|| format!("no named call to `{method}`: {rendered}"))?;
        assert_eq!(
            target.direct_call_id.as_ref(),
            Some(&body.direct_call_id),
            "{method}: {rendered}"
        );
        assert_eq!(
            target.canonical.as_ref(),
            body.canonical.as_ref(),
            "{method}: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn a_module_qualified_stdlib_call_carries_the_selected_declaration() -> Result<(), Box<dyn std::error::Error>> {
    // `import std.math` + `math.sqrt(..)` selects the same declaration `from std.math import sqrt` does, and records
    // it.
    let module = build(
        "import std.math\n\ndef main() -> float:\n  return math.sqrt(16.0)\n",
        &["m", "stdlib_module_call"],
    )?;
    let rendered = body_named(&module, "main")?.render_snapshot();
    assert!(!rendered.contains("unsupported("), "{rendered}");
    let targets = named_targets(&module, "main");
    let sqrt = targets
        .iter()
        .find(|target| target.name == "sqrt")
        .ok_or_else(|| format!("no named call to `sqrt`: {rendered}"))?;
    assert!(
        sqrt.canonical.is_some(),
        "the stdlib callee must keep its identity: {rendered}"
    );
    let delegation = module
        .stdlib_delegations
        .iter()
        .find(|delegation| Some(&delegation.canonical) == sqrt.canonical.as_ref())
        .ok_or("sqrt must retain its checked transparent delegation")?;
    assert_eq!(delegation.rust_path().as_deref(), Some("libm::sqrt"));
    assert_eq!(
        delegation.parameters,
        vec![IncanType::Primitive(IncanPrimitiveType::Float)]
    );
    assert_eq!(delegation.return_type, IncanType::Primitive(IncanPrimitiveType::Float));
    Ok(())
}

/// Imported aliases carry the same delegation, while a local shadow has no stdlib delegation.
#[test]
fn stdlib_delegations_follow_aliases_and_preserve_local_shadowing() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.math import gcd as common\n\ndef gcd(a: int, b: int) -> int:\n  return a + b\n\ndef main() -> int:\n  return common(b=18, a=48) + gcd(4, 6)\n",
        &["m", "import_alias"],
    )?;
    let targets = named_targets(&module, "main");
    let imported = targets
        .iter()
        .find(|target| target.name == "common")
        .ok_or("missing imported call")?;
    let local = targets
        .iter()
        .find(|target| target.name == "gcd")
        .ok_or("missing local call")?;
    assert!(
        module
            .stdlib_delegations
            .iter()
            .any(|delegation| Some(&delegation.canonical) == imported.canonical.as_ref()
                && delegation.rust_path().as_deref() == Some("incan_std_core::num::gcd_i64"))
    );
    assert!(
        !module
            .stdlib_delegations
            .iter()
            .any(|delegation| Some(&delegation.canonical) == local.canonical.as_ref())
    );
    Ok(())
}

#[test]
fn a_static_call_on_a_type_parameter_keeps_the_type_it_is_called_on() -> Result<(), Box<dyn std::error::Error>> {
    // `T.default()` under a `Default` bound is a trait's associated function; the type parameter selects its
    // implementation, so the call records it.
    let module = build(
        "def make[T with Default]() -> T:\n  return T.default()\n\ndef main() -> int:\n  return make[int]()\n",
        &["m", "type_param_static"],
    )?;
    let rendered = body_named(&module, "make")?.render_snapshot();
    assert!(!rendered.contains("unsupported("), "{rendered}");
    let targets = named_targets(&module, "make");
    let default = targets
        .iter()
        .find(|target| target.name.ends_with("default"))
        .ok_or_else(|| format!("no named call to `default`: {rendered}"))?;
    assert_eq!(
        default.receiver_type,
        Some(IncanType::TypeVar("T".to_string())),
        "{rendered}"
    );
    Ok(())
}

#[test]
fn calling_a_call_dunder_adopter_is_a_call_of_its_dunder() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Doubler:\n  extra: int\n\n  def __call__(self, value: int) -> int:\n    return value * 2 + self.extra\n\ndef main() -> int:\n  d = Doubler(extra=1)\n  return d(3)\n",
        &["m", "call_dunder"],
    )?;
    let main = body_named(&module, "main")?;
    let rendered = main.render_snapshot();
    assert!(!rendered.contains("unsupported("), "{rendered}");
    assert!(
        rendered.contains("method:__call__"),
        "the call must dispatch to `__call__`: {rendered}"
    );
    Ok(())
}

/// Class layouts and constructors retain canonical class and field identities, including receiver origins.
#[test]
fn source_local_class_retains_checked_layout_and_receiver_facts() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class Counter:\n  value: int\n\n  def get(self) -> int:\n    return self.value\n\n  def bump(mut self) -> None:\n    self.value += 1\n\ndef main() -> None:\n  mut counter = Counter(value=3)\n  counter.bump()\n";
    let module = build(source, &["m", "class_layout"])?;
    let declaration = module.nominal_declarations.first().ok_or("missing class declaration")?;
    assert_eq!(declaration.canonical.kind, SemanticSourceTargetKind::Class);
    assert_eq!(declaration.fields, ["value"]);
    assert!(module.is_well_formed_nominal_declaration(declaration));
    for (name, mutable) in [("get", false), ("bump", true)] {
        let body = body_named(&module, name)?;
        assert_eq!(body.locals[0].origin, bir::LocalOrigin::Receiver { mutable });
        assert!(module.body_has_canonical_direct_call_id(body));
        let mut tampered_body = body.clone();
        tampered_body.locals[0].ty = IncanType::Named("ForeignOwner".to_string());
        assert!(!module.body_has_canonical_direct_call_id(&tampered_body));
    }
    let mut tampered = declaration.clone();
    tampered.canonical.kind = SemanticSourceTargetKind::Trait;
    assert!(!module.is_well_formed_nominal_declaration(&tampered));
    Ok(())
}
