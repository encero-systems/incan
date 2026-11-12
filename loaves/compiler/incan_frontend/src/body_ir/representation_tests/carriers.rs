use super::*;

/// A standard package owns generic newtype methods just like any other source package. Their wrapped-field
/// projections and callable coverage must survive canonical carrier qualification without a native bridge.
pub(super) fn assert_generic_newtype_provider_methods() -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::executable_representation::{DeclarationCoverage, SurfaceReader, build_surface};
    use std::collections::BTreeSet;

    for source in [
        "pub type Mutex[T] = newtype T:\n    pub def get(self) -> T:\n        return self.0\n",
        "@derive(Clone)\npub type Mutex[T with Clone] = newtype list[T]:\n    pub def get(self) -> list[T]:\n        return self.0\n",
    ] {
        for package in ["application", "incan_stdlib_primitives"] {
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let module_path = vec!["primitives".to_owned(), "async".to_owned(), "sync".to_owned()];
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module_path.clone()));
            checker.set_current_package_identity(Some(package.to_owned()));
            checker
                .check_program(&program)
                .map_err(|errors| format!("{errors:?}"))?;
            let mut facts = checker.into_type_info();
            let owner = facts
                .declarations
                .declaration_identities
                .values()
                .find(|identity| identity.declaration_name == "Mutex")
                .ok_or("missing checked generic newtype identity")?
                .clone();
            let key = bir::nominal_carrier_key(&owner).ok_or("missing checked generic carrier")?;
            // Generated SDK operations retain a provider identity even without an explicit source import.
            facts
                .declarations
                .body_ir_native_type_identities
                .insert(key.clone(), owner.clone());
            facts
                .declarations
                .body_ir_native_type_identities
                .insert(owner.declaration_name.clone(), owner);
            let providers = ProviderOperationCatalog::new();
            let context = BodyIrContext {
                modules: &[],
                provider_operations: &providers,
                registry_package: None,
                library_package: Some(package),
                include_inline_tests: false,
            };
            let module = super::build_body_ir_module(&program, &module_path, &facts, &context);
            let method = module
                .bodies
                .iter()
                .find(|body| body.name == "get")
                .ok_or("missing newtype method")?;
            let identity = method.canonical.clone().ok_or("missing newtype method identity")?;
            let public = BTreeSet::from([identity.clone()]);
            let bytes = build_surface(
                std::slice::from_ref(&module),
                package,
                "0.1.0",
                &public,
                &BTreeSet::new(),
            )?;
            let reader = SurfaceReader::open(&bytes)?;
            let coverage = reader.index().coverage(&identity)?;
            assert!(
                matches!(coverage, DeclarationCoverage::Covered { .. }),
                "{package}: {coverage:?}; body: {method:#?}; layouts: {:#?}",
                module.nominal_declarations
            );
        }
    }
    Ok(())
}

/// A method and its returned newtype may be retained in separate publication fragments. The owner remains exact,
/// and neither missing storage nor conflicting copies may be resolved by context order or a short name.
pub(super) fn assert_independently_retained_newtype_return() -> Result<(), Box<dyn std::error::Error>> {
    let source = "pub type Token = newtype str\npub class Factory:\n    pub def token(self) -> Token:\n        return Token(\"value\")\n";
    let (_, _, seed) = checked_seed(source)?;
    let owner = seed
        .nominal_declarations
        .iter()
        .find(|declaration| declaration.name == "Token")
        .ok_or("missing checked Token layout")?
        .clone();
    let method = seed
        .callable_declaration_frames()
        .find(|body| body.name.ends_with("token"))
        .and_then(|body| body.canonical.clone())
        .ok_or("missing checked token method")?;
    let mut methods = seed.clone();
    methods
        .nominal_declarations
        .retain(|declaration| declaration.canonical != owner.canonical);
    let mut layouts = seed.clone();
    layouts.bodies.clear();
    layouts
        .nominal_declarations
        .retain(|declaration| declaration.canonical == owner.canonical);
    assert!(layouts.is_well_formed_nominal_declaration(&owner));
    for context in [
        vec![methods.clone(), layouts.clone()],
        vec![layouts.clone(), methods.clone()],
    ] {
        let returns = super::collect::collect_published_newtype_returns(&context);
        let returned = returns
            .get(&method)
            .and_then(Option::as_ref)
            .ok_or("separately retained newtype owner was refused")?;
        assert_eq!(returned.declaration, owner);
    }
    let missing = super::collect::collect_published_newtype_returns(&[methods.clone()]);
    assert!(missing.get(&method).is_some_and(Option::is_none));
    let mut conflict = layouts.clone();
    conflict.nominal_declarations[0].field_types = vec![super::IncanType::Primitive(super::IncanPrimitiveType::Int)];
    for context in [
        vec![methods.clone(), layouts.clone(), conflict.clone()],
        vec![conflict, layouts, methods],
    ] {
        let returns = super::collect::collect_published_newtype_returns(&context);
        assert!(returns.get(&method).is_some_and(Option::is_none));
    }
    Ok(())
}

/// Prove nominal-carrier retention refines each callable's canonical map without replacing that allocation.
pub(super) fn assert_nominal_carrier_maps_stay_shared() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Value:\n    number: int\ndef make() -> Value:\n    return Value(number=1)\ndef forward() -> Value:\n    return make()\n";
    let (program, facts, mut module) = checked_seed(source)?;
    let opaque_carriers = super::opaque_returns::collect(&module, &facts);
    let selected = std::collections::HashSet::new();
    super::trait_values::retain_inputs(&mut module, &program, &facts, &[], &selected);
    super::trait_returns::retain_returns(&mut module, &program, &facts, &[], &selected);
    super::opaque_returns::retain(&mut module, &opaque_carriers, &selected);
    assert!(
        super::nominal_carriers::return_index_keeps_identity_maps_borrowed(&module),
        "the return index retained a whole canonical map per callable"
    );
    let identities = module
        .bodies
        .iter()
        .map(|body| {
            Ok((
                body.canonical.clone().ok_or("missing callable identity")?,
                std::sync::Arc::as_ptr(&body.named_type_identities),
            ))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, &'static str>>()?;

    super::nominal_carriers::retain(&mut module, &facts, &[], &selected);

    for body in &module.bodies {
        let canonical = body.canonical.as_ref().ok_or("missing retained callable identity")?;
        let original = identities.get(canonical).ok_or("missing original canonical map")?;
        assert!(
            std::ptr::eq(*original, std::sync::Arc::as_ptr(&body.named_type_identities)),
            "nominal retention replaced the shared canonical map for {}",
            body.name
        );
    }
    Ok(())
}

/// Prove a nested generic static call carries the enclosing symbolic bound arguments into Body IR.
pub(super) fn assert_nested_generic_static_call_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Source[E]:\n    def integer(mut self) -> Result[int, E]\ntrait Reconstruct:\n    @staticmethod\n    def reconstruct[E, S with Source[E]](mut source: S) -> Result[Self, E]\nmodel Inner with Reconstruct:\n    value: int\n    @staticmethod\n    def reconstruct[E, S with Source[E]](mut source: S) -> Result[Inner, E]:\n        return Ok(Inner(value=source.integer()?))\nmodel Outer with Reconstruct:\n    inner: Inner\n    @staticmethod\n    def reconstruct[E, S with Source[E]](mut source: S) -> Result[Outer, E]:\n        return Ok(Outer(inner=Inner.reconstruct(source)?))\n";
    let (_, facts, module) = checked_seed(source)?;
    let checked_arguments = vec![
        crate::symbols::ResolvedType::Named("E".to_owned()),
        crate::symbols::ResolvedType::Named("S".to_owned()),
    ];
    let (call_span, arguments) = facts
        .calls
        .body_ir_call_type_arguments
        .iter()
        .find(|(_, arguments)| *arguments == &checked_arguments)
        .ok_or("missing checked nested static call arguments")?;
    assert_eq!(
        facts.calls.call_site_monomorph_type_args.get(call_span),
        Some(arguments),
    );
    assert!(matches!(
        facts.calls.body_ir_call_result_types.get(call_span),
        Some(crate::symbols::ResolvedType::Generic(base, sides))
            if base == "Result"
                && matches!(sides.as_slice(), [_, crate::symbols::ResolvedType::Named(error)] if error == "E")
    ));

    let body_arguments = vec![
        super::IncanType::TypeVar("E".to_owned()),
        super::IncanType::TypeVar("S".to_owned()),
    ];
    let retained = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } if target.type_args == body_arguments => Some(target),
            _ => None,
        })
        .ok_or("nested static call omitted its Body IR type arguments")?;
    assert!(retained.canonical.is_some());
    assert!(retained.direct_call_id.is_some());
    Ok(())
}

/// Reflection records use their Incan provider layout, including the primitive package and same-named local types.
pub(super) fn assert_native_provider_field_layout() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
@derive(Eq)
pub model FieldInfo:
    pub name: FrozenStr
    pub identity: FrozenBytes
    pub canonical: bool
    pub renamed_from: FrozenList[FrozenStr]
    pub alias: Option[FrozenStr]
    pub description: Option[FrozenStr]
    pub wire_name: FrozenStr
    pub type_name: FrozenStr
    pub has_default: bool
    pub extra: FrozenDict[FrozenStr, FrozenStr]
pub model Holder:
    pub fields: FrozenList[FieldInfo]
pub model MetadataFacts:
    hidden: int = 1
    pub values [renamed_from { name="items", since="0.6", message="" }]: list[int] = []
    pub text: str = ""
"#;
    for kind in ["model", "class"] {
        let source = source.replace("pub model FieldInfo:", &format!("pub {kind} FieldInfo:"));
        for (package, module_parts) in [
            (None, vec!["reflection"]),
            (Some("application"), vec!["reflection"]),
            (Some("incan_stdlib_primitives"), vec!["ordinary"]),
            (Some("incan_stdlib_primitives"), vec!["reflection"]),
            (Some("incan_stdlib_primitives"), vec!["primitives", "reflection"]),
        ] {
            let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let module_path = module_parts.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let mut checker = TypeChecker::new();
            checker.set_current_module_path(Some(module_path.clone()));
            checker.set_current_package_identity(package.map(str::to_owned));
            checker
                .check_program(&program)
                .map_err(|errors| format!("{errors:?}"))?;
            let facts = checker.into_type_info();
            let providers = ProviderOperationCatalog::new();
            let context = BodyIrContext {
                modules: &[],
                provider_operations: &providers,
                registry_package: None,
                library_package: package,
                include_inline_tests: false,
            };
            let seed = super::build_module_seed(&program, &module_path, &facts, &context);
            let metadata = seed
                .nominal_declarations
                .iter()
                .find(|value| value.canonical.declaration_name == "MetadataFacts")
                .and_then(|value| value.reflection_metadata.as_ref())
                .ok_or("missing checked reflection facts")?;
            assert_eq!(metadata.len(), 2);
            assert_eq!(metadata[0].position, 1);
            assert!(metadata[0].default_is_empty);
            assert_eq!(metadata[0].renamed_from, ["items"]);
            let labels = metadata[0]
                .renamed_from_lifecycle
                .as_ref()
                .ok_or("missing lifecycle labels")?;
            assert_eq!(labels.since.as_deref(), Some("0.6"));
            assert_eq!(labels.message.as_deref(), Some(""));
            assert_eq!(labels.issue, None);
            assert_eq!(metadata[1].position, 2);
            assert!(!metadata[1].default_is_empty);
            let declaration = seed
                .nominal_declarations
                .iter()
                .find(|value| value.canonical.declaration_name == "FieldInfo")
                .ok_or("missing provider FieldInfo declaration")?;
            let holder = seed
                .nominal_declarations
                .iter()
                .find(|value| value.canonical.declaration_name == "Holder")
                .ok_or("missing containing model")?;
            let consumer = bir::native_nominal_item(&declaration.canonical).unwrap_or_default();
            assert!(consumer.is_empty());
            if declaration.native_item != consumer {
                return Err(format!(
                    "provider layout {:?}; consumer layout {:?}; declaration {:?}; containing field types {:?}",
                    declaration.native_item, consumer, declaration.canonical, holder.field_types
                )
                .into());
            }
            let key = bir::nominal_carrier_key(&declaration.canonical).ok_or("missing source carrier key")?;
            let mut published = seed.clone();
            for value in &mut published.nominal_declarations {
                value.named_type_identities.remove(&key);
            }
            super::nominal_carriers::retain_published_module(&mut published);
            let retained = published
                .nominal_declarations
                .iter()
                .find(|value| value.canonical.declaration_name == "FieldInfo")
                .ok_or("missing admitted provider layout")?;
            assert_eq!(retained.carrier_name(), key);
        }
    }
    Ok(())
}

/// Literal-only frozen argument adaptation must select a real trait declaration rather than the diagnostic fallback.
pub(super) fn assert_frozen_literal_bound_method_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
trait Observe:
    def reference(self, kind: FrozenStr, identity: FrozenBytes) -> int: ...
def positional[W with Observe](walker: W) -> int:
    return walker.reference("object", b"a")
def named[W with Observe](walker: W) -> int:
    return walker.reference(identity=b"a", kind="object")
trait Dynamic:
    def label(self, value: str) -> int: ...
trait Static:
    def label(self, value: FrozenStr) -> int: ...
def prefer[W with (Dynamic, Static)](walker: W) -> int:
    return walker.label("object")
"#;
    let (_, facts, module) = checked_seed(source)?;
    let selected = facts
        .calls
        .resolved_method_calls
        .values()
        .find(|call| call.method == "label")
        .ok_or("missing overload selection")?;
    let crate::typechecker::ResolvedMethodDispatch::Trait { trait_name, .. } = &selected.dispatch;
    assert_eq!(trait_name, "Dynamic");
    for name in ["positional", "named"] {
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or("missing bound caller")?;
        let target = body
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
            .ok_or("missing bound method call")?;
        let identity = target
            .canonical
            .as_ref()
            .ok_or("frozen literal lost the selected trait method")?;
        assert_eq!(identity.declaration_name, "reference");
    }
    for arguments in ["kind, b'a'", "'object', identity"] {
        let invalid = format!(
            "trait Observe:\n    def reference(self, kind: FrozenStr, identity: FrozenBytes) -> int: ...\ndef runtime[W with Observe](walker: W, kind: str, identity: bytes) -> int:\n    return walker.reference({arguments})\n"
        );
        let tokens = crate::lexer::lex(&invalid).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        assert!(TypeChecker::new().check_program(&program).is_err());
    }
    Ok(())
}

/// Prove an enclosing placeholder bound cannot satisfy a nested static call requiring other trait arguments.
pub(super) fn assert_nested_generic_static_call_bound_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Source[E]:\n    def integer(mut self) -> Result[int, E]\nmodel Inner:\n    value: int\n    @staticmethod\n    def reconstruct[E, S with Source[E]](mut source: S) -> Result[Inner, E]:\n        return Ok(Inner(value=source.integer()?))\nmodel Outer:\n    inner: Inner\n    @staticmethod\n    def reconstruct[S with Source[str]](mut source: S) -> Result[Outer, str]:\n        return Ok(Outer(inner=Inner.reconstruct[int, S](source)?))\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    assert!(checker.check_program(&program).is_err());
    Ok(())
}

/// Prove a generic carrier returned from a declared source keeps its nominal owner while replacing only the selected
/// ownership argument with the physical reference used by the body and its callers.
pub(super) fn assert_generic_view_carrier_storage() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class GraphState:\n    pub nodes: list[int]\nclass Cursor[S]:\n    pub selection: S\n    pub offset: int\nclass Selection[R]:\n    pub reader: R\n    pub key: int\n    pub def cursor(self, offset: int) -> Cursor[Borrowed[Selection[R]]] from self:\n        return Cursor[Borrowed[Selection[R]]](selection=self, offset=offset)\ndef select(reader: Borrowed[GraphState], key: int) -> Selection[Borrowed[GraphState]] from reader:\n    return Selection[Borrowed[GraphState]](reader=reader, key=key)\ndef select_mut(reader: BorrowedMut[GraphState], key: int) -> Selection[Reference[GraphState]] from reader:\n    return Selection[Reference[GraphState]](reader=reader, key=key)\ndef select_result(reader: Borrowed[GraphState]) -> Result[Selection[Borrowed[GraphState]], str] from reader:\n    return Ok(select(reader, 1))\ndef select_list(reader: Borrowed[GraphState]) -> list[Selection[Borrowed[GraphState]]] from reader:\n    return [select(reader, 0)]\ndef main() -> None:\n    mut state = GraphState(nodes=[1, 2])\n    selected = select(state, 1)\n    println(selected.reader.nodes[selected.key])\n    cursor = selected.cursor(0)\n    println(cursor.selection.reader.nodes[cursor.offset])\n    result = select_result(state)\n    match result:\n        Ok(matched) => println(matched.reader.nodes[matched.key])\n        Err(_) => pass\n    listed = select_list(state)\n    println(listed[0].reader.nodes[0])\n    mut exclusive = select_mut(state, 0)\n    exclusive.reader.nodes = [3]\n";
    let (_, _, module) = checked_seed(source)?;
    let select = module
        .bodies
        .iter()
        .find(|body| body.name == "select")
        .ok_or("missing generic view producer")?;
    let main = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("missing generic view consumer")?;
    let select_mut = module
        .bodies
        .iter()
        .find(|body| body.name == "select_mut")
        .ok_or("missing exclusive generic view producer")?;
    let select_list = module
        .bodies
        .iter()
        .find(|body| body.name == "select_list")
        .ok_or("missing list generic view producer")?;
    let cursor = module
        .bodies
        .iter()
        .find(|body| body.name == "cursor")
        .ok_or("missing chained generic view producer")?;
    let cursor_local = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("cursor"))
        .ok_or("missing chained generic view consumer local")?;
    assert!(
        matches!(
            &select.return_type,
            super::IncanType::Generic { args, .. }
                if matches!(args.as_slice(), [super::IncanType::Ref(_)])
        ),
        "select: {select:#?}"
    );
    assert!(
        matches!(
            &select_mut.return_type,
            super::IncanType::Generic { args, .. }
                if matches!(args.as_slice(), [super::IncanType::RefMut(_)])
        ),
        "select_mut: {select_mut:#?}"
    );
    assert!(
        matches!(
            &select_list.return_type,
            super::IncanType::Generic { args, .. }
                if matches!(
                    args.as_slice(),
                    [super::IncanType::Generic { args, .. }]
                        if matches!(args.as_slice(), [super::IncanType::Ref(_)])
                )
        ),
        "select_list: {select_list:#?}"
    );
    assert!(
        matches!(
            &cursor.return_type,
            super::IncanType::Generic { args, .. }
                if matches!(
                    args.as_slice(),
                    [super::IncanType::Ref(selection)]
                        if matches!(selection.as_ref(), super::IncanType::Generic { .. })
                )
        ),
        "cursor: {cursor:#?}"
    );
    assert_eq!(
        cursor.returned_view.as_ref().map(|view| view.component.as_slice()),
        Some(&[0][..]),
        "cursor: {cursor:#?}"
    );
    assert!(
        matches!(
            &cursor_local.ty,
            super::IncanType::Generic { args, .. }
                if matches!(
                    args.as_slice(),
                    [super::IncanType::Ref(selection)]
                        if matches!(
                            selection.as_ref(),
                            super::IncanType::Generic { args, .. }
                                if matches!(args.as_slice(), [super::IncanType::Ref(_)])
                        )
                )
        ),
        "cursor local: {cursor_local:#?}"
    );
    assert!(
        main.locals.iter().all(|local| !matches!(
            &local.ty,
            super::IncanType::Generic { args, .. }
                if matches!(args.as_slice(), [super::IncanType::Generic { base, .. }] if base == "Borrowed")
        )),
        "main: {main:#?}"
    );
    Ok(())
}

/// Prove builtin method meaning crosses the checker/Body IR boundary as a closed operation, independently of the
/// retained source spelling and canonical provenance.
pub(super) fn assert_builtin_method_identities() -> Result<(), Box<dyn std::error::Error>> {
    use incan_lang::lang::builtins::BuiltinMethodId;
    use incan_lang::lang::surface::{
        bytes_methods::BytesMethodId, clone_methods::CloneMethodId, dict_methods::DictMethodId,
        float_methods::FloatMethodId, iterator_methods::IteratorMethodId, list_methods::ListMethodId,
        option_methods::OptionMethodId, result_methods::ResultMethodId, set_methods::SetMethodId,
        string_methods::StringMethodId,
    };
    use incan_lang::lang::types::numerics::{
        IntegerHelperFamily, IntegerHelperOperation, NumericResizeMethodId, NumericTypeId,
    };

    let source = "def operations() -> None:\n    mut values: list[int] = []\n    values.append(1)\n    values.count(1)\n    values.iter().take(1).collect()\n    numbers().collect()\n    mut labels: dict[str, int] = {}\n    labels.insert(\"one\", 1)\n    labels.keys()\n    mut seen: set[int] = set()\n    seen.add(1)\n    seen.contains(1)\n    \"value\".encode()\n    b\"value\".decode()\n    Some(1).is_some()\n    result: Result[int, str] = Ok(1)\n    result.is_ok()\n    1u8.checked_add(1u8)\n    resized: u16 = 1u8.resize()\n    1.0.sqrt()\n\n\ndef numbers() -> Generator[int]:\n    yield 1\n\n\ndef clone_number(value: &int) -> int:\n    return value.clone()\n";
    let (_, facts, module) = checked_seed(source)?;
    let expected = [
        BuiltinMethodId::List(ListMethodId::Append),
        BuiltinMethodId::List(ListMethodId::Count),
        BuiltinMethodId::ListIter,
        BuiltinMethodId::Iterator(IteratorMethodId::Take),
        BuiltinMethodId::Iterator(IteratorMethodId::Collect),
        BuiltinMethodId::Generator(IteratorMethodId::Collect),
        BuiltinMethodId::Dict(DictMethodId::Insert),
        BuiltinMethodId::Dict(DictMethodId::Keys),
        BuiltinMethodId::Set(SetMethodId::Add),
        BuiltinMethodId::Set(SetMethodId::Contains),
        BuiltinMethodId::Str(StringMethodId::Encode),
        BuiltinMethodId::Bytes(BytesMethodId::Decode),
        BuiltinMethodId::Option(OptionMethodId::IsSome),
        BuiltinMethodId::Result(ResultMethodId::IsOk),
        BuiltinMethodId::IntegerHelper(
            NumericTypeId::U8,
            IntegerHelperFamily::Checked,
            IntegerHelperOperation::Add,
        ),
        BuiltinMethodId::NumericResize(NumericTypeId::U8, NumericResizeMethodId::Lossless),
        BuiltinMethodId::Float(NumericTypeId::F64, FloatMethodId::Sqrt),
        BuiltinMethodId::Clone(CloneMethodId::Clone),
    ];
    let checked = facts
        .calls
        .resolved_builtin_method_calls
        .values()
        .copied()
        .collect::<Vec<_>>();
    assert!(
        expected.iter().all(|operation| checked.contains(operation)),
        "checker operations: {checked:?}",
    );

    let carried = module
        .bodies
        .iter()
        .flat_map(|body| &body.block.stmts)
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => target.builtin_operation(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        expected.iter().all(|operation| carried.contains(operation)),
        "Body IR operations: {carried:?}",
    );
    Ok(())
}
