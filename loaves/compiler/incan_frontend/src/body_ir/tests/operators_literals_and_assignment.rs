//! Expression lowering with its ownership facts: arithmetic, string concatenation, copy / move / clone / drop facts on
//! non-copy bindings, the #1160 power, bitwise, shift, membership, concatenation and identity operators, RFC 028
//! user-defined operator dispatch, dict / set / bytes literals, slices and tuple projections, compound, field, index
//! and multi-target assignment, and the #1072 rule that plain assignment reuses the active Body IR local.

use super::*;

/// Both operand and recursively lowered place fields retain checked tuple structure without nominal identities.
#[test]
fn nested_source_tuple_fields_retain_checked_structural_projection() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def answer() -> int:\n    pair = ((1, 42), 3)\n    return (pair.0).1\n",
        &["lib"],
    )?;
    let body = module.bodies.first().ok_or("tuple body missing")?;
    let projection = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Return {
                value: Some(bir::Operand::Place(value)),
            } => Some(&value.place.projection),
            _ => None,
        })
        .ok_or("nested tuple return missing")?;
    assert_eq!(
        projection,
        &[
            bir::PlaceElem::structural_field("0"),
            bir::PlaceElem::structural_field("1")
        ]
    );
    Ok(())
}

#[test]
fn lowers_arithmetic_with_a_copy_last_use_and_a_move_return() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def add(x: int, y: int) -> int:\n  return x + y\n";
    let module = build(source, &["m", "arith"])?;
    let snapshot_first = module.render_snapshot();
    let snapshot_second = build(source, &["m", "arith"])?.render_snapshot();
    assert_eq!(snapshot_first, snapshot_second, "lowering must be deterministic");

    let body = body_named(&module, "add")?;
    assert_eq!(
        body.decl_id, body.direct_call_id,
        "top-level Body IR and declaration HIR must correlate by the same span-derived node id"
    );
    assert!(snapshot_first.contains("body add decl:m::arith#decl."));
    assert!(snapshot_first.contains("local 0 x : int [param]"));
    assert!(snapshot_first.contains("local 1 y : int [param]"));
    // x is not the last read (y is), so x is Copy either way (int is a Copy type); both reads should be `copy`.
    assert!(snapshot_first.contains("copy(_0)"));
    assert!(snapshot_first.contains("copy(_1"));
    // `int` is a Copy-shaped type, so even a freshly created temporary reads as `copy`, not `move`.
    assert!(snapshot_first.contains("return copy(_2, last_use)"));
    Ok(())
}

#[test]
fn lowers_string_concat_as_an_explicit_helper_call_with_runtime_requirements() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "def greet(name: str) -> str:\n  return \"hi \" + name\n";
    let module = build(source, &["m", "strs"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("call helper:str_concat"));
    assert!(snapshot.contains("runtime_requirements:"));
    assert!(snapshot.contains("runtime_helper(str_concat)"));
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn lowers_a_non_copy_binding_and_drops_it_when_never_moved() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make() -> None:\n  s = \"hello\"\n  return\n";
    let module = build(source, &["m", "drop"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("local 0 s : str [binding]"));
    assert!(snapshot.contains("drop _0"));
    Ok(())
}

#[test]
fn lowers_a_non_copy_binding_and_skips_the_drop_when_moved_via_return() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make() -> str:\n  s = \"hello\"\n  return s\n";
    let module = build(source, &["m", "moved"])?;
    let snapshot = module.render_snapshot();

    assert!(snapshot.contains("return move(_0, last_use)"));
    assert!(
        !snapshot.contains("drop _0"),
        "a moved-out local must not also be dropped: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_the_power_operator_as_a_primitive_keeping_the_checked_float_promotion()
-> Result<(), Box<dyn std::error::Error>> {
    // A dynamic `int ** int` exponent resolves `float`: the typechecker owns that promotion and lowering must carry
    // its answer onto the assigned temporary rather than re-deriving a result type from the operator. A
    // non-negative integer-literal exponent is the separate `int` case.
    let rendered = rendered_f("def f(a: int, b: int) -> float:\n  return a ** b\n", "pow")?;

    assert!(
        rendered.contains("_2 = copy(_0) ** copy(_1)"),
        "`**` must lower as a primitive binary op: {rendered}"
    );
    assert!(
        rendered.contains("local 2 <tmp> : float"),
        "the checked `float` result of `int ** int` must survive onto the temporary: {rendered}"
    );
    Ok(())
}

#[test]
fn exact_binary_float_arithmetic_keeps_the_checked_body_ir_width() -> Result<(), Box<dyn std::error::Error>> {
    for kind in ["f32", "f64"] {
        let source = format!("def f(left: {kind}, right: {kind}) -> {kind}:\n  return left * right\n");
        let rendered = rendered_f(&source, &format!("exact_{kind}"))?;
        assert!(
            rendered.contains(&format!("local 2 <tmp> : {kind}")),
            "{kind} multiplication must retain its checked exact result in Body IR: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn lowers_the_bitwise_and_shift_operators_as_primitives_keeping_the_checked_int_result()
-> Result<(), Box<dyn std::error::Error>> {
    for (spelling, module_leaf) in [
        ("&", "bitand"),
        ("|", "bitor"),
        ("^", "bitxor"),
        ("<<", "shl"),
        (">>", "shr"),
    ] {
        let source = format!("def f(a: int, b: int) -> int:\n  return a {spelling} b\n");
        let rendered = rendered_f(&source, module_leaf)?;

        assert!(
            rendered.contains(&format!("_2 = copy(_0) {spelling} copy(_1)")),
            "`{spelling}` must lower as a primitive binary op over both operands: {rendered}"
        );
        assert!(
            rendered.contains("local 2 <tmp> : int"),
            "`int {spelling} int` must keep its checked `int` result: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn lowers_identity_operators_distinctly_from_equality() -> Result<(), Box<dyn std::error::Error>> {
    // The Rust-emission backend currently emits `is`/`is not` exactly like `==`/`!=`. Body IR must still record
    // which operator the source wrote -- it is the representation any later identity/equality split gets decided
    // against, and a collapsed `is` leaves nothing to decide from.
    let is_rendered = rendered_f("def f(a: int, b: int) -> bool:\n  return a is b\n", "is_op")?;
    assert!(
        is_rendered.contains("_2 = copy(_0) is copy(_1)"),
        "`is` must lower as its own operator: {is_rendered}"
    );
    assert!(
        !is_rendered.contains("=="),
        "`is` must not be collapsed into equality: {is_rendered}"
    );

    let is_not_rendered = rendered_f("def f(a: int, b: int) -> bool:\n  return a is not b\n", "is_not_op")?;
    assert!(
        is_not_rendered.contains("_2 = copy(_0) is not copy(_1)"),
        "`is not` must lower as its own operator: {is_not_rendered}"
    );
    assert!(
        !is_not_rendered.contains("!="),
        "`is not` must not be collapsed into inequality: {is_not_rendered}"
    );
    Ok(())
}

#[test]
fn lowers_string_membership_as_an_explicit_helper_call_with_its_runtime_requirement()
-> Result<(), Box<dyn std::error::Error>> {
    // This is the Body IR representation `parity-987-0003` was missing. That row records string `in` as
    // `Preserved` -- the runtime helper's substring policy -- but evaluates through the stdlib-runtime evidence
    // lane, so nothing proved the behavior was expressible here at all. An explicit `Callee::Helper` call with a
    // recorded runtime requirement is what closes that gap: the substring choice is now a represented fact rather
    // than something a reader has to infer from the operand types.
    let rendered = rendered_f(
        "def f(hay: str, needle: str) -> bool:\n  return needle in hay\n",
        "str_in",
    )?;

    // Membership is the one string operator whose surface order is the reverse of its helper's signature. The call
    // is emitted haystack-first to match `incan_lang::strings::str_contains`, so a backend can bind every string
    // helper positionally without knowing that one of them disagrees with the rest.
    assert!(
        rendered.contains("_2 = call helper:str_contains(move(_0, last_use), move(_1, last_use))"),
        "string `in` must lower to a helper call carrying the haystack then the needle: {rendered}"
    );
    assert!(
        rendered.contains("runtime_helper(str_contains)"),
        "the helper call must record its runtime requirement: {rendered}"
    );
    assert!(
        !rendered.contains("unsupported("),
        "string membership must not fall back to a placeholder: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_negated_string_membership_as_its_own_helper_rather_than_a_wrapped_negation()
-> Result<(), Box<dyn std::error::Error>> {
    // One source operator stays one Body IR operation, following the `str_eq`/`str_ne` pair: a consumer reading
    // this call knows the source wrote `not in` without having to recognize a negation wrapper around `in`.
    let rendered = rendered_f(
        "def f(hay: str, needle: str) -> bool:\n  return needle not in hay\n",
        "str_not_in",
    )?;

    assert!(
        rendered.contains("_2 = call helper:str_not_contains(move(_0, last_use), move(_1, last_use))"),
        "string `not in` must lower to its own helper call, haystack first: {rendered}"
    );
    assert!(
        rendered.contains("runtime_helper(str_not_contains)"),
        "the negated membership helper must record its own runtime requirement: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_collection_membership_as_a_helper_call_naming_its_own_container() -> Result<(), Box<dyn std::error::Error>> {
    // Each collection names its own helper rather than sharing one `contains`. A single variant would leave a
    // consumer to re-derive list-versus-set-versus-dict from operand types, which is the inference Body IR exists
    // to replace with a represented fact -- and it is why `str in str` keeps its own helper too.
    //
    // The call is emitted haystack-first to match `str_contains` and every `contains` in Rust, while the source
    // spelling reads needle-first, so a backend can bind every membership helper positionally.
    for (container, module_leaf, helper) in [
        ("xs: List[int], v: int", "list_in", "list_contains"),
        ("xs: Set[int], v: int", "set_in", "set_contains"),
        ("xs: Dict[int, str], v: int", "dict_in", "dict_contains_key"),
    ] {
        let source = format!("def f({container}) -> bool:\n  return v in xs\n");
        let rendered = rendered_f(&source, module_leaf)?;

        assert!(
            rendered.contains(&format!("call helper:{helper}(move(_0, last_use)")),
            "`in` over {container} must lower to {helper} with the container first: {rendered}"
        );
        assert!(
            rendered.contains(&format!("runtime_helper({helper})")),
            "{helper} must record its runtime requirement: {rendered}"
        );
        assert!(
            !rendered.contains("unsupported("),
            "collection membership must not fall back to a placeholder: {rendered}"
        );
        assert!(
            !rendered.contains("str_contains"),
            "collection membership must not borrow the string substring policy: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn lowers_negated_collection_membership_as_its_own_helper_per_container() -> Result<(), Box<dyn std::error::Error>> {
    // One source operator stays one Body IR operation, following the `str_contains`/`str_not_contains` pair: a
    // consumer reading this call knows the source wrote `not in` without recognizing a negation wrapper.
    for (container, module_leaf, helper) in [
        ("xs: List[int], v: int", "list_not_in", "list_not_contains"),
        ("xs: Set[int], v: int", "set_not_in", "set_not_contains"),
        ("xs: Dict[int, str], v: int", "dict_not_in", "dict_not_contains_key"),
    ] {
        let source = format!("def f({container}) -> bool:\n  return v not in xs\n");
        let rendered = rendered_f(&source, module_leaf)?;

        assert!(
            rendered.contains(&format!("call helper:{helper}(move(_0, last_use)")),
            "`not in` over {container} must lower to {helper}, container first: {rendered}"
        );
        assert!(
            rendered.contains(&format!("runtime_helper({helper})")),
            "{helper} must record its own runtime requirement: {rendered}"
        );
        assert!(
            !rendered.contains("un_op") && !rendered.contains("Not("),
            "`not in` must be its own operation, not a negation wrapped around `in`: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn dict_membership_names_key_lookup_rather_than_element_lookup() -> Result<(), Box<dyn std::error::Error>> {
    // Dict membership tests keys while its sibling collections test elements. Leaving that to be inferred from the
    // receiver type would make key-versus-value a backend convention; naming it in the operation makes it a fact.
    let rendered = rendered_f(
        "def f(d: Dict[str, int], k: str) -> bool:\n  return k in d\n",
        "dict_key_in",
    )?;

    assert!(
        rendered.contains("call helper:dict_contains_key("),
        "dict `in` must name key lookup: {rendered}"
    );
    assert!(
        !rendered.contains("helper:dict_contains("),
        "dict membership must not use an element-lookup spelling: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_list_concatenation_as_a_helper_call_rather_than_a_primitive_addition()
-> Result<(), Box<dyn std::error::Error>> {
    // The regression this test exists for: `+` on two lists used to satisfy `binary_op_is_supported` through the
    // numeric mapping and lower to `BinOp::Add`, a machine addition over two heap containers, with no
    // `Unsupported` marker for a consumer to notice. The typechecker accepts list `+` through a builtin branch
    // that records no operator dispatch, so nothing downstream marked it as a call -- which is exactly the
    // wrong-representation failure `lower_operator_dispatch` guards user-defined `__add__` against.
    let rendered = rendered_f(
        "def f(xs: List[int], ys: List[int]) -> List[int]:\n  return xs + ys\n",
        "list_concat",
    )?;

    assert!(
        rendered.contains("call helper:list_concat(move(_0, last_use), move(_1, last_use))"),
        "list `+` must lower to the concatenation helper in source order: {rendered}"
    );
    assert!(
        rendered.contains("runtime_helper(list_concat)"),
        "list concatenation must record its runtime requirement: {rendered}"
    );
    assert!(
        !rendered.contains(") + "),
        "list `+` must not lower to a primitive addition: {rendered}"
    );
    Ok(())
}

#[test]
fn compound_list_assignment_routes_through_the_same_concatenation_helper() -> Result<(), Box<dyn std::error::Error>> {
    // `lower_compound_assignment` shares both the admission gate and the emission path with `lower_binary`, so
    // giving list `+` a helper silently changed `xs += ys` too. That is the behavior the Rust-emission backend
    // already has -- `determine_binop_plan` sees `Add` over two lists whichever spelling produced it -- but a
    // shared path that changed without coverage is exactly where a later divergence would hide.
    let rendered = rendered_f(
        "def f(ys: List[int]) -> List[int]:\n  mut xs = [1, 2]\n  xs += ys\n  return xs\n",
        "list_concat_assign",
    )?;

    assert!(
        rendered.contains("call helper:list_concat("),
        "`+=` on lists must reuse the concatenation helper rather than a primitive addition: {rendered}"
    );
    assert!(
        rendered.contains("runtime_helper(list_concat)"),
        "the compound form must record the same runtime requirement as the binary form: {rendered}"
    );
    Ok(())
}

#[test]
fn list_equality_stays_a_primitive_because_that_is_what_the_other_backend_emits()
-> Result<(), Box<dyn std::error::Error>> {
    // The counterweight to the concatenation test above, and the reason closing the admission hole is not "refuse
    // every primitive over a collection". `determine_binop_plan` in the Rust-emission backend routes list `+` to
    // `incan_std_core::collections::list_concat` -- so calling it `BinOp::Add` contradicted that backend -- but it
    // emits comparisons as an infix operator, which on two `Vec`s resolves to Rust's `PartialEq` and matches
    // Python's element-wise `==`. Both backends therefore agree that this one *is* an operator, and refusing it
    // here would manufacture a divergence instead of closing one.
    let rendered = rendered_f(
        "def f(xs: List[int], ys: List[int]) -> bool:\n  return xs == ys\n",
        "list_eq",
    )?;

    assert!(
        !rendered.contains("unsupported("),
        "list equality must keep lowering, matching the Rust-emission backend: {rendered}"
    );
    assert!(
        rendered.contains(" == "),
        "list equality must stay a primitive comparison rather than becoming a helper call: {rendered}"
    );
    Ok(())
}

#[test]
fn an_unresolved_binary_operand_refuses_before_either_expression_is_lowered() -> Result<(), Box<dyn std::error::Error>>
{
    let binary = "def f(value: int) -> int:\n  return value & missing\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(binary, &["m", "unknown_binary"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the source checker must reject the unknown operand before Body IR lowers it"
    );
    assert!(
        rendered.contains("unsupported(binary operator BitAnd)"),
        "an unresolved primitive operand must refuse at the binary expression: {rendered}"
    );
    assert!(
        !rendered.contains("copy(_0)") && !rendered.contains("missing"),
        "the refusal must precede both operand reads, not materialize an external unknown: {rendered}"
    );

    let compound = "def f() -> int:\n  mut value = 1\n  value &= missing\n  return value\n";
    let (module, diagnostics) = build_after_expected_typecheck_errors(compound, &["m", "unknown_compound"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        !diagnostics.is_empty(),
        "the source checker must reject the unknown compound right operand before Body IR lowers it"
    );
    assert!(
        rendered.contains("unsupported(compound assignment operator BitAnd)"),
        "an unresolved compound operand must refuse at the compound statement: {rendered}"
    );
    assert!(
        !rendered.contains(" & ") && !rendered.contains("missing"),
        "the compound refusal must not synthesize a primitive operation or external read: {rendered}"
    );
    Ok(())
}

#[test]
fn lowers_every_bitwise_and_shift_compound_assignment_as_a_read_modify_write() -> Result<(), Box<dyn std::error::Error>>
{
    // Each `<op>=` form must produce exactly the operator its binary spelling produces, plus a write back to the
    // same local -- the compound path shares `lower_binary_from_operands` precisely so the two cannot drift.
    let source = "def f() -> int:\n  mut v = 8\n  v &= 3\n  v |= 4\n  v ^= 1\n  v <<= 2\n  v >>= 1\n  return v\n";
    let rendered = rendered_f(source, "compound_bits")?;

    for (index, (spelling, operand)) in [("&", 3), ("|", 4), ("^", 1), ("<<", 2), (">>", 1)].iter().enumerate() {
        let temp = index + 1;
        assert!(
            rendered.contains(&format!("_{temp} = copy(_0) {spelling} const({operand})")),
            "`{spelling}=` must combine the current value with the right operand: {rendered}"
        );
        assert!(
            rendered.contains(&format!("_0 = copy(_{temp}, last_use)")),
            "`{spelling}=` must write the result back to the assigned local: {rendered}"
        );
    }
    assert!(
        !rendered.contains("unsupported("),
        "no bitwise or shift compound form may fall back: {rendered}"
    );
    Ok(())
}

#[test]
fn a_compound_assignment_through_an_operator_hook_is_refused_by_name() -> Result<(), Box<dyn std::error::Error>> {
    // `v &= w` on a type with `__and__` is a method call. Now that `&` has a primitive `BinOp`, combining the
    // operands here would claim a machine operation the source never asked for -- the wrong-representation
    // failure `lower_binary` already guards against for the binary spelling. Body IR has no place-targeted
    // dispatch form yet, so the refusal names the hook it would have to call.
    let source = "model Box:\n  value: int\n\n  def __and__(self, other: Box) -> Box:\n    return Box(value=self.value & other.value)\n\ndef f() -> int:\n  mut v = Box(value=1)\n  v &= Box(value=2)\n  return v.value\n";
    let rendered = rendered_f(source, "hook_compound")?;

    assert!(
        rendered.contains("unsupported(compound assignment through operator hook `__and__`)"),
        "a hooked compound assignment must refuse by naming the method it would dispatch to: {rendered}"
    );
    assert!(
        !rendered.contains("copy(_1) & "),
        "it must not fall through to the primitive bitwise operator: {rendered}"
    );
    Ok(())
}

#[test]
fn protocol_hook_operators_reach_lowering_as_resolved_method_calls() -> Result<(), Box<dyn std::error::Error>> {
    // #1160's refusal-boundary question, answered: `@` and both pipes are protocol hooks with no primitive form.
    // The typechecker resolves them through `__matmul__` / `__pipe_forward__` / `__pipe_backward__` and rejects
    // the expression outright when no hook resolves, so a well-typed program always arrives here with a recorded
    // dispatch. They need no operator-table entry and carry no refusal -- and so no `Disposition::Unsupported`
    // corpus row, which would otherwise have needed an owner.
    let source = "model OpBox:\n  value: int\n\n  def __matmul__(self, other: OpBox) -> OpBox:\n    return other\n\n  def __pipe_forward__(self, other: OpBox) -> OpBox:\n    return other\n\n  def __pipe_backward__(self, other: OpBox) -> OpBox:\n    return other\n\ndef matmul(a: OpBox, b: OpBox) -> OpBox:\n  return a @ b\n\ndef forward(a: OpBox, b: OpBox) -> OpBox:\n  return a |> b\n\ndef backward(a: OpBox, b: OpBox) -> OpBox:\n  return a <| b\n";
    let module = build(source, &["m", "protocol_hooks"])?;

    for (body_name, method, spelling) in [
        ("matmul", "__matmul__", "@"),
        ("forward", "__pipe_forward__", "|>"),
        ("backward", "__pipe_backward__", "<|"),
    ] {
        let rendered = body_named(&module, body_name)?.render_snapshot();
        assert!(
            rendered.contains(&format!("call method:{method} unbound(borrow(_0), move(_1, last_use))")),
            "`{spelling}` must lower as the method the typechecker resolved, receiver borrowed: {rendered}"
        );
        assert!(
            !rendered.contains("unsupported("),
            "`{spelling}` must not reach the operator table at all: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn the_shift_and_power_operators_record_no_panic_fact() -> Result<(), Box<dyn std::error::Error>> {
    // A stated decision, not an omission. `**` and the shifts can only trap by exceeding the result width, which
    // is the same arithmetic-overflow class as `+`, `-`, and `*` -- none of which record a fact either. Recording
    // one here would claim these three operators fail in a way ordinary arithmetic does not.
    let rendered = rendered_f(
        "def f(a: int, b: int) -> int:\n  return (a << b) + (a >> b)\n",
        "shift_panics",
    )?;
    assert!(
        !rendered.contains("panic_facts:"),
        "shifts must not record a panic fact: {rendered}"
    );

    // The contrast that gives that decision meaning: floor division, whose divisor may be zero on every build
    // profile, still records one.
    let divide = rendered_f("def f(a: int, b: int) -> int:\n  return a // b\n", "div_panics")?;
    assert!(
        divide.contains("division_or_modulo"),
        "division must still record its panic fact: {divide}"
    );
    Ok(())
}

#[test]
fn lowers_a_clone_when_a_non_copy_binding_is_read_more_than_once() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def dup(s: str) -> str:\n  first = s\n  return s\n";
    let module = build(source, &["m", "clone"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("clone(_0)"),
        "the first, non-last read of `s` should clone: {snapshot}"
    );
    assert!(snapshot.contains("return move(_0, last_use)"));
    Ok(())
}

#[test]
fn lowers_a_dict_literal_as_a_dict_aggregate_with_paired_operands() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make() -> dict[str, int]:\n  return {\"a\": 1, \"b\": 2}\n";
    let module = build(source, &["m", "dict_lit"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("dict[const(\"a\"): const(1), const(\"b\"): const(2)]"),
        "dict aggregate should render key/value pairs: {snapshot}"
    );
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn lowers_a_set_literal_as_a_set_aggregate() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make() -> set[str]:\n  return {\"a\", \"b\"}\n";
    let module = build(source, &["m", "set_lit"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("set[const(\"a\"), const(\"b\")]"),
        "set aggregate should render as a flat element list: {snapshot}"
    );
    assert!(snapshot.contains("allocator"));
    Ok(())
}

#[test]
fn lowers_a_slice_expression_as_a_slice_projected_place_read() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def middle(s: str) -> str:\n  return s[1:3]\n";
    let module = build(source, &["m", "slice"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("[const(1):const(3)]"),
        "slice projection should render start/end operands: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_tuple_unpack_into_field_projected_reads_off_a_materialized_tuple() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def sum_pair() -> int:\n  pair = (1, 2)\n  a, b = pair\n  return a + b\n";
    let module = build(source, &["m", "tuple_unpack"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains(".0") && snapshot.contains(".1"),
        "tuple unpack should project each element by index: {snapshot}"
    );
    assert!(
        !snapshot.contains("unsupported("),
        "tuple unpack should not fall back: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_compound_assignment_as_a_read_modify_write() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def accumulate(step: int) -> int:\n  mut total = 0\n  total += step\n  return total\n";
    let module = build(source, &["m", "compound"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "compound assignment should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains(" + "),
        "compound assignment should desugar through a binary op: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_compound_string_assignment_through_the_string_concat_helper() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def greet(name: str) -> str:\n  mut out = \"hi \"\n  out += name\n  return out\n";
    let module = build(source, &["m", "compound_str"])?;
    let snapshot = module.render_snapshot();

    assert!(
        snapshot.contains("call helper:str_concat"),
        "string compound assignment should route through the same helper as `+`: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_field_assignment_on_a_mutable_model_parameter() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "model Counter:\n  count: int\n\ndef bump(mut c: Counter) -> int:\n  c.count = c.count + 1\n  return c.count\n";
    let module = build(source, &["m", "field_assign"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "field assignment should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains(".count = "),
        "should assign into the `.count` projection: {snapshot}"
    );
    Ok(())
}

#[test]
fn lowers_index_assignment_on_a_mutable_list_parameter() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def set_first(mut items: list[int], value: int) -> None:\n  items[0] = value\n  return\n";
    let module = build(source, &["m", "index_assign"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "index assignment should not fall back: {snapshot}"
    );
    assert!(
        snapshot.contains("[const(0)] = "),
        "should assign into the `[0]` projection: {snapshot}"
    );
    Ok(())
}

#[test]
fn index_assignment_evaluates_object_before_index() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def make_items() -> list[int]:\n  return [1]\n\ndef make_index() -> int:\n  return 0\n\ndef assign() -> None:\n  make_items()[make_index()] = 7\n  return\n";
    let module = build(source, &["m", "index_assignment_order"])?;
    let snapshot = module.render_snapshot();
    let object_call = snapshot
        .find("call fn:make_items()")
        .ok_or("missing index-assignment object call")?;
    let index_call = snapshot
        .find("call fn:make_index()")
        .ok_or("missing index-assignment index call")?;

    assert!(
        object_call < index_call,
        "index assignment must evaluate its object before its index: {snapshot}"
    );
    Ok(())
}

const VEC2_SRC: &str = "@derive(Debug)\nmodel Vec2:\n  x: int\n  y: int\n\n  def __add__(self, other: Vec2) -> Vec2:\n    return Vec2(x=self.x + other.x, y=self.y + other.y)\n\n";

#[test]
fn a_user_defined_operator_lowers_to_the_method_the_typechecker_resolved() -> Result<(), Box<dyn std::error::Error>> {
    // Representing this as `BinOp::Add` would claim a primitive machine operation where the source calls a
    // method -- a wrong representation rather than an honest refusal, with no marker for a consumer to notice.
    let source = format!("{VEC2_SRC}def f(a: Vec2, b: Vec2) -> Vec2:\n  return a + b\n");
    let module = build(&source, &["m", "user_op"])?;
    let rendered = body_named(&module, "f")?.render_snapshot();

    assert!(
        rendered.contains("call method:__add__ unbound(borrow(_0),"),
        "a user-defined operator must dispatch to its resolved method, with the left operand borrowed as the \
             receiver: {rendered}"
    );
    assert!(
        !rendered.contains("copy(_0) + copy(_1)") && !rendered.contains("move(_0, last_use) + "),
        "it must not also lower as a primitive operation: {rendered}"
    );
    Ok(())
}

#[test]
fn primitive_operators_are_unaffected_by_operator_dispatch() -> Result<(), Box<dyn std::error::Error>> {
    // The typechecker records no dispatch for primitives, so these must keep their existing representations.
    let ints = build("def f(a: int, b: int) -> int:\n  return a + b\n", &["m", "prim_int"])?;
    assert!(
        body_named(&ints, "f")?
            .render_snapshot()
            .contains("copy(_0) + copy(_1)"),
        "integer addition must stay a primitive binary operation"
    );

    let strings = build("def f(a: str, b: str) -> str:\n  return a + b\n", &["m", "prim_str"])?;
    assert!(
        body_named(&strings, "f")?
            .render_snapshot()
            .contains("call helper:str_concat("),
        "string concatenation must stay a compiler-owned helper call"
    );
    Ok(())
}

// ---- Bytes literals and range values (#1165) ----

#[test]
fn bytes_literals_lower_to_their_own_constant_rather_than_a_string() -> Result<(), Box<dyn std::error::Error>> {
    let source = concat!(
        "def send(payload: bytes) -> int:\n",
        "  return 1\n",
        "\n",
        "def keep() -> bytes:\n",
        "  greeting = b\"hi\"\n",
        "  return greeting\n",
        "\n",
        "def run() -> int:\n",
        "  return send(b\"\\x00\\xff\")\n",
    );
    let module = build(source, &["m", "bytes_literal"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a byte-string literal must lower to a real constant: {snapshot}"
    );
    assert!(
        snapshot.contains("const(b\"\\x68\\x69\")"),
        "a bound byte-string literal must render as its own bytes constant: {snapshot}"
    );
    assert!(
        !snapshot.contains("const(\"hi\")"),
        "a byte-string literal must never be represented as the string constant it is not: {snapshot}"
    );
    assert!(
        snapshot.contains("call fn:send(const(b\"\\x00\\xff\"))"),
        "a byte-string literal must survive as a constant in argument position: {snapshot}"
    );
    assert!(
        snapshot.contains(" greeting : bytes [binding]"),
        "the bound local must keep its checked `bytes` type: {snapshot}"
    );

    // The owned-buffer representation is what makes this read a move rather than a copy: `bytes` reports
    // `AbiV0Ownership::Owned`, so its last read transfers ownership exactly as a `str` local's would.
    let greeting = local_for_binding(&snapshot, "greeting").ok_or("expected a local for `greeting`")?;
    assert!(
        snapshot.contains(&format!("return move({greeting}, last_use)")),
        "the last read of an owned bytes local must be a move: {snapshot}"
    );
    Ok(())
}

// ---- #1072: preserve the typechecker's lexical assignment decision in Body IR ----

#[test]
fn plain_assignment_reuses_its_active_body_ir_local() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def run() -> int:\n  mut x = 1\n  x = 2\n  return x\n",
        &["m", "plain_reassign"],
    )?;
    let body = body_named(&module, "run")?;
    let names: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("x"))
        .collect();

    assert_eq!(
        names.len(),
        1,
        "plain reassignment must write the original local rather than declare a duplicate: {}",
        module.render_snapshot()
    );
    Ok(())
}

#[test]
fn branch_shadowing_does_not_leak_into_following_body_ir_reads() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def run() -> int:\n  let x = 1\n  if true:\n    let x = 2\n  return x\n",
        &["m", "branch_shadow"],
    )?;
    let body = body_named(&module, "run")?;
    let x_locals: Vec<_> = body
        .locals
        .iter()
        .filter(|local| local.name.as_deref() == Some("x"))
        .collect();
    let outer = x_locals.first().ok_or("fixture must declare an outer `x`")?.id;
    assert_eq!(x_locals.len(), 2, "the explicit `let` shadow must retain both locals");

    let returned_local = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Return {
                value: Some(bir::Operand::Place(operand)),
            } => operand.place.local_id(),
            _ => None,
        })
        .ok_or("fixture must return a local place")?;
    assert_eq!(
        returned_local, outer,
        "a read after the branch must resolve to the enclosing local, not the branch-only shadow"
    );
    Ok(())
}

#[test]
fn plain_multi_target_assignment_reuses_active_body_ir_locals() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def run() -> int:\n  mut left = 0\n  mut right = 0\n  left, right = (1, 2)\n  return left + right\n",
        &["m", "plain_multi_reassign"],
    )?;
    let body = body_named(&module, "run")?;
    for name in ["left", "right"] {
        let count = body
            .locals
            .iter()
            .filter(|local| local.name.as_deref() == Some(name))
            .count();
        assert_eq!(
            count,
            1,
            "plain multi-target assignment must reuse `{name}` rather than create a duplicate: {}",
            module.render_snapshot()
        );
    }
    Ok(())
}
