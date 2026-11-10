use super::*;

/// Prove the decreasing shared-access fixed point across forwarding, recursion and a function-item adapter while a
/// value-returning body and explicit source contracts keep their established ownership.
pub(super) fn assert_shared_loans() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Native:\n    value: int\nmodel Wrapper:\n    item: Native\nclass Reader:\n    pub def read(self, items: Native) -> int:\n        return items.value\ntrait Fetcher:\n    def fetch(self, scope: Native, request: Native) -> int: ...\nclass Fetch with Fetcher:\n    def fetch(self, scope: Native, request: Native) -> int:\n        return scope.value + request.value\ndef leaf(items: Native) -> int:\n    return items.value\ndef explicit(items: &Native) -> int:\n    return items.value\ndef forward(items: Native) -> int:\n    return leaf(items)\ndef explicit_forward(items: Native) -> int:\n    return explicit(items)\ndef explicit_projected(wrapper: &Wrapper) -> int:\n    return explicit(wrapper.item)\ndef method_forward(reader: Reader, items: Native) -> int:\n    return reader.read(items)\ndef trait_forward[F with Fetcher](source: F, scope: Native, request: Native) -> int:\n    return source.fetch(scope, request)\ndef left(items: Native, remaining: int) -> int:\n    if remaining == 0:\n        return items.value\n    return right(items, remaining - 1)\ndef right(items: Native, remaining: int) -> int:\n    if remaining == 0:\n        return items.value\n    return left(items, remaining - 1)\ndef escaped(items: Native) -> int:\n    return items.value\ndef take(items: Native) -> Native:\n    return items\ndef use() -> int:\n    callback = escaped\n    return callback(Native(value=1))\n";
    let (_, _, mut candidate) = checked_seed(source)?;
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
pub(super) fn assert_projected_shared_loans() -> Result<(), Box<dyn std::error::Error>> {
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
pub(super) fn assert_complete_shared_read_shapes() -> Result<(), Box<dyn std::error::Error>> {
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

/// Read-only string callees and every ordinary caller storage shape retain a loan, while ownership-changing bodies
/// keep their existing access.
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

def moved(text: str) -> str:
    return text
def stored(text: str) -> list[str]:
    return [text]
def mutated(mut text: str) -> int:
    return len(text)
"#;
    let (_, _, mut module) = checked_seed(source)?;
    super::shared_reads::retain_modules(std::slice::from_mut(&mut module), &[]);

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
    for name in ["moved", "stored", "mutated"] {
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
        ("greet", 1),
    ] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing retained owned-string boundary")?;
        assert_eq!(
            body.params[parameter].input_access,
            incan_semantics_core::IncanInputAccess::Inferred,
            "{name}: {body:#?}"
        );
    }
    for name in ["dictionary_key", "optional_dictionary_key"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing dictionary-key boundary")?;
        assert!(
            body.locals.iter().all(|local| {
                !matches!(local.origin, bir::LocalOrigin::Temporary)
                    || local.ty
                        != super::IncanType::Ref(Box::new(super::IncanType::Primitive(super::IncanPrimitiveType::Str)))
            }),
            "{name}: {body:#?}"
        );
    }
    let declaration_strings = module
        .bodies
        .iter()
        .find(|body| body.name == "declaration_strings")
        .ok_or("missing declaration-string caller")?;
    assert!(
        declaration_strings.locals.iter().all(|local| {
            !matches!(local.origin, bir::LocalOrigin::Temporary)
                || local.ty
                    != super::IncanType::Ref(Box::new(super::IncanType::Primitive(super::IncanPrimitiveType::Str)))
        }),
        "{declaration_strings:#?}"
    );
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
        super::IncanType::Primitive(super::IncanPrimitiveType::Str),
        "{keyed_call:#?}"
    );
    Ok(())
}

/// Prove a read lent to one input and moved to another of the same call clones for the move, so the callee's loan
/// never reads moved storage.
pub(super) fn assert_lent_and_moved_read() -> Result<(), Box<dyn std::error::Error>> {
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
