use super::*;

/// A caller rebuilt against an admitted namespace uses its published ABI, even if running the optimizer
/// a second time on the provider body would infer a different access after its reads were rewritten.
#[test]
fn restored_namespace_access_guides_fresh_calls() -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::IncanInputAccess::{Inferred, InferredShared};
    let (_, _, seed) = checked_seed(
        "model Request:\n    value: int\ndef inspect(request: Request) -> int:\n    return request.value\ndef caller(request: Request) -> int:\n    return inspect(request)\n",
    )?;
    for access in [Inferred, InferredShared] {
        let mut provider = seed.clone();
        provider.bodies.retain(|body| body.name == "inspect");
        let body = provider.bodies.first_mut().ok_or("missing provider body")?;
        body.params[0].input_access = access;
        let mut caller = seed.clone();
        caller.bodies.retain(|body| body.name == "caller");
        let mut modules = [provider.clone(), caller];
        crate::body_ir::settle_shared_reads_with_namespaces(
            &mut modules,
            &[],
            &std::collections::HashSet::new(),
            &[true, false],
        );
        assert_eq!(modules[0].bodies, provider.bodies);
        let caller = modules[1].bodies.first().ok_or("missing fresh caller")?;
        let operand = caller
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Call { args, .. } => match args.first()? {
                    bir::ArgumentElement::One(bir::Operand::Place(read)) => Some(read),
                    _ => None,
                },
                _ => None,
            })
            .ok_or("missing caller operand")?;
        assert_eq!(operand.fact == bir::OwnershipFact::Borrow, access == InferredShared);
    }
    Ok(())
}

/// The ownership frontier reads the actual selected callees. Changes stop at callers whose own
/// exported input access stays equal, even when their outgoing reads changed.
#[test]
fn only_direct_readers_see_a_changed_settled_input() -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::IncanInputAccess::{Inferred, InferredShared};
    let (_, _, mut module) = checked_seed(
        "model Request:\n    value: int\ndef leaf(request: Request) -> int:\n    return request.value\ndef invoke(value: int) -> int:\n    return leaf(Request(value=value))\ndef main() -> None:\n    println(invoke(4))\n",
    )?;
    let leaf = module
        .bodies
        .iter_mut()
        .find(|b| b.name == "leaf")
        .ok_or("missing leaf")?;
    leaf.params[0].input_access = InferredShared;
    let first = crate::body_ir::SettledCallInputs::from_modules(std::slice::from_ref(&module))?;
    let prior = first.reads(&mut module);
    assert_eq!(prior.len(), 2);
    assert!(prior.iter().all(|r| first.matches(r)));
    module
        .bodies
        .iter_mut()
        .find(|b| b.name == "leaf")
        .ok_or("missing leaf")?
        .params[0]
        .input_access = Inferred;
    let changed = crate::body_ir::SettledCallInputs::from_modules(std::slice::from_ref(&module))?;
    let stale = prior.iter().filter(|r| !changed.matches(r)).collect::<Vec<_>>();
    assert_eq!(stale.len(), 1);
    let invoke = module
        .bodies
        .iter()
        .find(|b| b.name == "invoke")
        .ok_or("missing invoke")?;
    assert_eq!(stale[0].reader, invoke.direct_call_id.path());
    Ok(())
}

/// Prove immutable parameter sharing across forwarding, recursion and a function-item adapter while explicit source
/// contracts keep their established ownership and escaping operations retain their checked transfer facts.
pub(super) fn assert_shared_loans() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Native:\n    value: int\nmodel Wrapper:\n    item: Native\nclass Reader:\n    pub def read(self, items: Native) -> int:\n        return items.value\ntrait Fetcher:\n    def fetch(self, scope: Native, request: Native) -> int: ...\nclass Fetch with Fetcher:\n    def fetch(self, scope: Native, request: Native) -> int:\n        return scope.value + request.value\ndef leaf(items: Native) -> int:\n    return items.value\ndef explicit(items: &Native) -> int:\n    return items.value\ndef forward(items: Native) -> int:\n    return leaf(items)\ndef explicit_forward(items: Native) -> int:\n    return explicit(items)\ndef explicit_projected(wrapper: &Wrapper) -> int:\n    return explicit(wrapper.item)\ndef method_forward(reader: Reader, items: Native) -> int:\n    return reader.read(items)\ndef trait_forward[F with Fetcher](source: F, scope: Native, request: Native) -> int:\n    return source.fetch(scope, request)\ndef left(items: Native, remaining: int) -> int:\n    if remaining == 0:\n        return items.value\n    return right(items, remaining - 1)\ndef right(items: Native, remaining: int) -> int:\n    if remaining == 0:\n        return items.value\n    return left(items, remaining - 1)\ndef escaped(items: Native) -> int:\n    return items.value\ndef take(items: Native) -> Native:\n    return items\ndef use() -> int:\n    callback = escaped\n    return callback(Native(value=1))\n";
    let (program, facts, mut candidate) = checked_seed(source)?;
    let leaf = candidate
        .bodies
        .iter_mut()
        .find(|body| body.name == "leaf")
        .ok_or("missing profiled loan body")?;
    leaf.checked_input_profile = Some(bir::CheckedInputProfile {
        result_requirements: Vec::new(),
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
    let selected_leaf = leaf.clone();
    let selected_identity = selected_leaf
        .canonical
        .clone()
        .ok_or("missing selected loan identity")?;
    let mut partially_selected = candidate.clone();
    let selected = std::collections::HashSet::from([selected_identity]);
    super::retain_module_representations(
        &mut partially_selected,
        &program,
        &facts,
        crate::body_ir::ContextModules::default(),
        &selected,
    );
    super::shared_reads::retain_modules_with_selected(
        std::slice::from_mut(&mut partially_selected),
        crate::body_ir::ContextModules::default(),
        &selected,
    );
    let retained_selected_leaf = partially_selected
        .bodies
        .iter()
        .find(|body| body.name == "leaf")
        .ok_or("missing selected loan body")?;
    assert_eq!(retained_selected_leaf, &selected_leaf);
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
        super::shared_reads::retain_modules(
            std::slice::from_mut(&mut explicit),
            crate::body_ir::ContextModules::default(),
        );
        let parameter = explicit
            .bodies
            .iter()
            .find(|body| body.name == "leaf")
            .ok_or("missing retained explicit loan control")?;
        assert_eq!(parameter.params[0].input_access, access);
        assert_eq!(parameter.params[0].ty, original_type);
        assert_eq!(parameter.locals[parameter.params[0].local.index()].ty, original_type);
    }
    super::shared_reads::retain_modules(
        std::slice::from_mut(&mut candidate),
        crate::body_ir::ContextModules::default(),
    );
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

/// Prove an exact checked field projection is lent to a read-only callee, while a callee that consumes its input keeps
/// it by value and a projected clone into it leaves the containing parameter shared.
pub(super) fn assert_projected_shared_loans() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Native:\n    value: int\nmodel Wrapper:\n    item: Native\ndef observe(item: Native) -> int:\n    return item.value\ndef projected(wrapper: Wrapper) -> int:\n    return observe(wrapper.item)\ndef take(item: Native) -> Native:\n    return item\ndef projected_take(wrapper: Wrapper) -> Native:\n    return take(wrapper.item)\n";
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(
        std::slice::from_mut(&mut module),
        crate::body_ir::ContextModules::default(),
    );

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
    for (name, access) in [
        ("take", incan_semantics_core::IncanInputAccess::Inferred),
        ("projected_take", incan_semantics_core::IncanInputAccess::InferredShared),
    ] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing projected transfer body")?;
        assert_eq!(body.params[0].input_access, access, "{name}: {body:#?}");
    }
    let conversion_source =
        "def count(values: list[int]) -> int:\n    return 42\ndef ranged() -> int:\n    return count(range(1, 3))\n";
    let (_, _, mut conversions) = checked_seed(conversion_source)?;
    super::shared_reads::retain_modules(
        std::slice::from_mut(&mut conversions),
        crate::body_ir::ContextModules::default(),
    );
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

/// Prove every immutable parameter with a shared executable form borrows at entry unless its body consumes it on any
/// path, as a store, a return or a capture does. Unrelated aggregates, results, closures and matches do not weaken the
/// ABI. A result or an optional union input has no shared executable form and keeps its by-value input.
pub(super) fn assert_complete_shared_read_shapes() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Native:\n    value: int\ndef narrowed(value: str | None) -> str:\n    if value is not None:\n        return value.upper()\n    return \"missing\"\ndef discarded(item: Native) -> int:\n    _ = item\n    return 1\ndef formatted(item: Native) -> str:\n    return f\"{item:?}\"\ndef stored(item: Native) -> list[Native]:\n    return [item]\ndef compared(value: str) -> bool:\n    return value == \"chosen\"\ndef returned(value: str) -> str:\n    return value\ndef matched(item: Result[Native, str]) -> int:\n    match item:\n        Ok(_) => return 1\n        Err(_) => return 0\ndef match_take(item: Result[Native, str]) -> Native:\n    match item:\n        Ok(value) => return value\n        Err(_) => return Native(value=0)\ndef indexed(items: list[Native]) -> int:\n    return items[0].value\ndef index_take(items: list[Native]) -> Native:\n    return items[0]\ndef iterated(items: list[Native]) -> int:\n    mut total = 0\n    for item in items:\n        total += item.value\n    return total\ndef loop_take(items: list[Native]) -> Native:\n    for item in items:\n        return item\n    return Native(value=0)\ndef unrelated(item: Native) -> int:\n    values = [1, 2]\n    callback: Callable[int, int] = (value) => value + 1\n    result: Result[int, str] = Ok(callback(values[0]))\n    match result:\n        Ok(_) => pass\n        Err(_) => pass\n    return item.value\ndef captured(item: Native) -> int:\n    callback: () -> int = () => item.value\n    return callback()\n";
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(
        std::slice::from_mut(&mut module),
        crate::body_ir::ContextModules::default(),
    );

    for (name, access) in [
        ("formatted", incan_semantics_core::IncanInputAccess::InferredShared),
        ("compared", incan_semantics_core::IncanInputAccess::InferredShared),
        ("indexed", incan_semantics_core::IncanInputAccess::InferredShared),
        ("iterated", incan_semantics_core::IncanInputAccess::InferredShared),
        ("unrelated", incan_semantics_core::IncanInputAccess::InferredShared),
        ("stored", incan_semantics_core::IncanInputAccess::Inferred),
        ("returned", incan_semantics_core::IncanInputAccess::Inferred),
        ("index_take", incan_semantics_core::IncanInputAccess::InferredShared),
        ("discarded", incan_semantics_core::IncanInputAccess::InferredShared),
        ("loop_take", incan_semantics_core::IncanInputAccess::InferredShared),
        ("captured", incan_semantics_core::IncanInputAccess::Inferred),
        ("matched", incan_semantics_core::IncanInputAccess::Inferred),
        ("match_take", incan_semantics_core::IncanInputAccess::Inferred),
        ("narrowed", incan_semantics_core::IncanInputAccess::Inferred),
    ] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing immutable parameter shape")?;
        assert_eq!(body.params[0].input_access, access, "{name}: {body:#?}");
    }
    Ok(())
}

/// Read-only string callees and every ordinary caller storage shape retain a loan, while mutable, partial-target and
/// consumed parameters keep their by-value access.
#[test]
fn read_only_string_calls_borrow_the_callers_storage() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Holder:
    text: str

model Greeter:
    name: str

    def greet(self, greeting: str) -> str:
        return f"{greeting} {self.name}"

    hello = partial greet(greeting="hello")

model Registration:
    kind: str

model RegistrationIndex:
    entries: dict[str, Registration]

    def shape(self, kind: str) -> int:
        if kind not in self.entries:
            return 0
        return len(self.entries[kind].kind)

    def registration(self, kind: str) -> int:
        self.shape(kind)
        return charge_registration(self.entries[kind])

const PREFIX: str = "pre"
static GREETING: str = "hello"

def ignored(text: str) -> int:
    return 1
def length(text: str) -> int:
    return len(text)
def sliced(text: str) -> int:
    return len(text[0:1])
def compared(text: str) -> bool:
    return text == "!"

def local() -> int:
    text = "payload"
    return ignored(text) + length(text) + sliced(text)
def forwarded(text: str) -> int:
    return ignored(text) + length(text) + sliced(text)
def field(holder: Holder) -> int:
    return ignored(holder.text) + length(holder.text) + sliced(holder.text)
def indexed(values: list[str]) -> int:
    return length(values[0])
def set_contains(values: set[str], text: str) -> bool:
    return text in values
def keyed_pair(key: str, value: int) -> int:
    return length(key) + value
def keyed_call(key: str, values: dict[str, int]) -> int:
    return keyed_pair(key, values[key])
def dictionary_key(values: dict[str, int], key: str) -> int:
    return values[key]
def optional_dictionary_key(values: dict[str, Option[bool | int]], key: str) -> str:
    match values[key]:
        int(number) => return f"int {number}"
        bool(truth) => return f"bool {truth}"
        None => return "absent"
def charge_registration(registration: Registration) -> int:
    return len(registration.kind)
def registration_lookup(entries: dict[str, Registration], kind: str) -> int:
    if kind not in entries:
        return 0
    return charge_registration(entries[kind])
def declaration_strings() -> int:
    return ignored(PREFIX) + ignored(GREETING)

enum Policy(str):
    Kept = "kept"

def policy(name: str) -> Option[Policy]:
    return Policy.from_value(name)

def moved(text: str) -> str:
    return text
def stored(text: str) -> list[str]:
    return [text]
def mutated(mut text: str) -> int:
    return len(text)
"#;
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(
        std::slice::from_mut(&mut module),
        crate::body_ir::ContextModules::default(),
    );

    for name in ["ignored", "length", "sliced", "compared"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing read-only string callee")?;
        assert_eq!(
            body.params[0].input_access,
            incan_semantics_core::IncanInputAccess::InferredShared,
            "{name}: {body:#?}"
        );
    }
    let sliced = module
        .bodies
        .iter()
        .find(|body| body.name == "sliced")
        .ok_or("missing sliced string callee")?;
    assert!(
        sliced.locals.iter().any(|local| {
            matches!(local.origin, bir::LocalOrigin::Temporary)
                && local.ty == super::IncanType::Primitive(super::IncanPrimitiveType::Str)
        }),
        "{sliced:#?}"
    );
    for name in ["local", "forwarded", "field"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing string caller")?;
        let calls = body
            .block
            .stmts
            .iter()
            .filter_map(|statement| match &statement.kind {
                bir::StatementKind::Call {
                    callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                    args,
                    ..
                } if matches!(target.name.as_str(), "ignored" | "length" | "sliced") => args.first(),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(calls.len(), 3, "{name}: {body:#?}");
        assert!(
            calls.iter().all(|argument| matches!(
                argument,
                bir::ArgumentElement::One(bir::Operand::Place(read))
                    if read.fact == bir::OwnershipFact::Borrow && !read.last_use
            )),
            "{name}: {body:#?}"
        );
    }
    for name in ["moved", "stored"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing ownership-changing string control")?;
        assert_eq!(
            body.params[0].input_access,
            incan_semantics_core::IncanInputAccess::Inferred,
            "{name}: {body:#?}"
        );
    }
    let mutated = module
        .bodies
        .iter()
        .find(|body| body.name == "mutated")
        .ok_or("missing mutable string control")?;
    assert_eq!(
        mutated.params[0].input_access,
        incan_semantics_core::IncanInputAccess::Inferred,
        "{mutated:#?}"
    );
    let indexed = module
        .bodies
        .iter()
        .find(|body| body.name == "indexed")
        .ok_or("missing indexed string control")?;
    let indexed_argument = indexed.block.stmts.iter().find_map(|statement| match &statement.kind {
        bir::StatementKind::Call {
            callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
            args,
            ..
        } if target.name == "length" => args.first(),
        _ => None,
    });
    let Some(bir::ArgumentElement::One(bir::Operand::Place(read))) = indexed_argument else {
        return Err("missing indexed string call argument".into());
    };
    assert_eq!(
        indexed.locals[read.place.local_id().ok_or("missing indexed argument local")?.index()].ty,
        super::IncanType::Primitive(super::IncanPrimitiveType::Str),
        "{indexed:#?}"
    );
    let set_contains = module
        .bodies
        .iter()
        .find(|body| body.name == "set_contains")
        .ok_or("missing set-key control")?;
    assert_eq!(
        set_contains.params[1].input_access,
        incan_semantics_core::IncanInputAccess::Inferred,
        "{set_contains:#?}"
    );
    for (name, parameter) in [
        ("dictionary_key", 1),
        ("optional_dictionary_key", 1),
        ("registration_lookup", 1),
        ("registration", 1),
    ] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing retained owned-string boundary")?;
        assert_eq!(
            body.params[parameter].input_access,
            incan_semantics_core::IncanInputAccess::InferredShared,
            "{name}: {body:#?}"
        );
    }
    let greet = module
        .bodies
        .iter()
        .find(|body| body.name == "greet")
        .ok_or("missing partial-target boundary")?;
    assert_eq!(
        greet.params[1].input_access,
        incan_semantics_core::IncanInputAccess::Inferred,
        "{greet:#?}"
    );
    for name in ["dictionary_key", "optional_dictionary_key"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing dictionary-key boundary")?;
        assert!(
            body.locals.iter().any(|local| {
                matches!(local.origin, bir::LocalOrigin::Temporary)
                    && local.ty
                        == super::IncanType::Ref(Box::new(super::IncanType::Primitive(super::IncanPrimitiveType::Str)))
            }),
            "{name}: {body:#?}"
        );
    }
    // A temporary that hands the input to an owned use, here `from_value`'s owned text, keeps its owned value.
    for name in ["declaration_strings", "policy"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing owned-temporary control")?;
        assert!(
            body.locals.iter().all(|local| {
                !matches!(local.origin, bir::LocalOrigin::Temporary)
                    || local.ty
                        != super::IncanType::Ref(Box::new(super::IncanType::Primitive(super::IncanPrimitiveType::Str)))
            }),
            "{name}: {body:#?}"
        );
    }
    let keyed_call = module
        .bodies
        .iter()
        .find(|body| body.name == "keyed_call")
        .ok_or("missing keyed call control")?;
    let keyed_argument = keyed_call
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                args,
                ..
            } if target.name == "keyed_pair" => args.first(),
            _ => None,
        });
    let Some(bir::ArgumentElement::One(bir::Operand::Place(read))) = keyed_argument else {
        return Err("missing keyed call argument".into());
    };
    assert_eq!(
        keyed_call.locals[read.place.local_id().ok_or("missing keyed argument local")?.index()].ty,
        super::IncanType::Ref(Box::new(super::IncanType::Primitive(super::IncanPrimitiveType::Str))),
        "{keyed_call:#?}"
    );
    Ok(())
}

/// Prove a read lent to one input and moved to another of the same call clones for the move, so the callee's loan
/// never reads moved storage. The input the callee returns is consumed and keeps its by-value access.
pub(super) fn assert_lent_and_moved_read() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Item:\n    count: int\ndef both(kept: Item, taken: Item) -> Item:\n    total = kept.count\n    return taken\ndef caller(item: Item) -> Item:\n    return both(item, item)\n";
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(
        std::slice::from_mut(&mut module),
        crate::body_ir::ContextModules::default(),
    );
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
    let returned = both.block.stmts.iter().find_map(|statement| match &statement.kind {
        bir::StatementKind::Return {
            value: Some(bir::Operand::Place(read)),
        } => Some(read.fact),
        _ => None,
    });
    assert_eq!(returned, Some(bir::OwnershipFact::Move), "{both:#?}");
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
