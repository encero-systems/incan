//! SDK newtypes retain source-owned binders and method signatures without exposing foreign storage.

use super::{
    TypeCheckInfo, TypeChecker, body_named, build, lexer, library_call_delegations, library_nominal_declarations,
    parser,
};
use crate::symbols::TypeInfo;
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_semantics_core::{IncanType, SymbolOrigin};

/// A static SDK constructor closes owner binders on its receiver without inventing method binders.
#[test]
fn sdk_static_constructor_keeps_owner_arguments_separate() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.async import Mutex\n\ndef main() -> None:\n    locks: list[Mutex[int]] = [Mutex.new(1)]\n    println(len(locks))\n",
        &["m", "sdk_static_owner"],
    )?;
    let target = super::named_targets(&module, "main")
        .into_iter()
        .find(|target| target.name == "Mutex.new")
        .ok_or("missing SDK constructor")?;
    assert!(matches!(&target.receiver_type, Some(IncanType::Generic { args, .. })
        if args == &[IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int)]));
    assert!(target.type_args.is_empty(), "{target:?}");
    Ok(())
}

/// SDK linkage retains nominal returns even when a facade import initially resolved to a module.
#[test]
fn root_channel_linkage_keeps_returned_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_values_and_calls/passing_channel_senders_and_locks_on.incn"
    );
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let (_, layouts, calls) = super::super::build_body_ir_module_v0_with_sdk_linkage(
        &program,
        &["main".to_owned()],
        checker.type_info(),
        &[],
    );
    for name in ["Sender", "Receiver", "Mutex"] {
        assert!(
            layouts.iter().any(|layout| layout.name == name),
            "missing {name}: {layouts:?}"
        );
    }
    assert!(calls.iter().any(|call| call.canonical.declaration_name == "channel"));
    assert!(calls.iter().any(|call| call.canonical.declaration_name == "new"));
    Ok(())
}

/// A checked root-module channel import is a callable, with closed storage for the handles later used in lists.
#[test]
fn root_channel_import_keeps_checked_callable_storage() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_values_and_calls/passing_channel_senders_and_locks_on.incn"
    );
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".to_owned(), "root_channel_storage".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.type_info();
    let module = build(source, &["m", "root_channel_storage"])?;
    let body = body_named(&module, "main")?;
    let tx = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("tx"))
        .ok_or("missing sender")?;
    assert!(
        matches!(&tx.ty, IncanType::Generic { args, .. } if args == &[IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int)]),
        "{}",
        body.render_snapshot()
    );
    assert!(
        body.locals.iter().all(|local| local.ty != IncanType::Unknown),
        "{}\nimports={:?}\npaths={:?}\nfunctions={:?}",
        body.render_snapshot(),
        facts.declarations.resolved_import_identities,
        facts.import_bindings,
        facts.declarations.function_bindings,
    );
    Ok(())
}

/// A source module import cannot acquire the callable identity of a same-spelled exported member.
#[test]
fn sdk_module_import_does_not_gain_function_identity() -> Result<(), Box<dyn std::error::Error>> {
    let tokens = lexer::lex("import std.async.channel as channel\n\ndef main() -> None:\n    pass\n")
        .map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let info = checker.type_info();
    let module = super::super::build_body_ir_module_v0(&program, &["main".to_owned()], info);
    assert!(!module.stdlib_delegations.iter().any(|delegate| delegate.canonical.kind
        == incan_semantics_core::SemanticSourceTargetKind::Function
        && delegate.canonical.declaration_name == "channel"));
    Ok(())
}

/// Plain generic channel wrappers keep the exact declaring owner and binder order for native metadata validation.
#[test]
fn sdk_channel_newtypes_retain_declared_binders() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "async".to_owned(), "channel".to_owned()];
    for name in ["Sender", "Receiver", "OneshotSender", "OneshotReceiver"] {
        let identity = cache.lookup_identity(&path, name).ok_or("missing channel owner")?;
        let mut facts = TypeCheckInfo::default();
        facts
            .declarations
            .named_type_identities
            .insert(name.to_owned(), identity);
        let layouts = library_nominal_declarations(&facts);
        let owner = layouts
            .iter()
            .find(|layout| layout.name == name)
            .ok_or("missing generic SDK newtype")?;
        assert_eq!(owner.type_parameters, ["T"]);
        assert_eq!(owner.type_parameter_count, 1);
        assert_eq!(owner.fields, ["0"]);
        assert_eq!(owner.field_identities, std::slice::from_ref(&owner.canonical));
        assert!(matches!(owner.canonical.origin, SymbolOrigin::Package { .. }));
        assert_eq!(owner.derives.iter().any(|derive| derive == "Clone"), name == "Sender");
    }
    Ok(())
}

/// A generic owner's synchronous method keeps a shared receiver and the owner binder, rather than raw runtime ABI.
#[test]
fn sdk_generic_newtype_method_retains_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "async".to_owned(), "channel".to_owned()];
    let TypeInfo::Newtype(owner) = cache.lookup_type(&path, "Sender").ok_or("missing sender")? else {
        return Err("sender must remain a source newtype".into());
    };
    let identity = owner
        .methods
        .get("is_closed")
        .and_then(|method| method.identity.clone())
        .ok_or("missing method")?;
    let mut facts = TypeCheckInfo::default();
    facts.references.resolved_identities.insert((1, 2), identity.clone());
    let calls = library_call_delegations(&facts);
    let call = calls
        .iter()
        .find(|call| call.canonical == identity)
        .ok_or("missing generic SDK method")?;
    assert_eq!(call.type_parameters, ["T"]);
    assert_eq!(
        call.parameters,
        [IncanType::Ref(Box::new(IncanType::Generic {
            base: "Sender".to_owned(),
            args: vec![IncanType::TypeVar("T".to_owned())],
        }))]
    );
    Ok(())
}

/// An async method retains a deferred result and the declared receiver; boxing may not erase its borrow contract.
#[test]
fn sdk_async_newtype_method_retains_deferred_result() -> Result<(), Box<dyn std::error::Error>> {
    let mut cache = StdlibAstCache::new();
    let path = vec!["std".to_owned(), "async".to_owned(), "channel".to_owned()];
    let TypeInfo::Newtype(owner) = cache.lookup_type(&path, "Receiver").ok_or("missing receiver")? else {
        return Err("receiver must remain a source newtype".into());
    };
    let identity = owner
        .methods
        .get("recv")
        .and_then(|method| method.identity.clone())
        .ok_or("missing receive method")?;
    let mut facts = TypeCheckInfo::default();
    facts.references.resolved_identities.insert((1, 2), identity.clone());
    let calls = library_call_delegations(&facts);
    let call = calls
        .iter()
        .find(|call| call.canonical == identity)
        .ok_or("missing async SDK method")?;
    assert_eq!(call.type_parameters, ["T"]);
    assert!(matches!(call.parameters.first(), Some(IncanType::Ref(_))));
    assert_eq!(
        call.return_type,
        IncanType::Generic {
            base: "Awaitable".to_owned(),
            args: vec![IncanType::Generic {
                base: "Option".to_owned(),
                args: vec![IncanType::TypeVar("T".to_owned())]
            }],
        }
    );
    Ok(())
}

/// A same-spelled application newtype keeps its source identity and cannot acquire an SDK carrier item.
#[test]
fn source_sender_keeps_its_own_layout() -> Result<(), Box<dyn std::error::Error>> {
    let module = build("type Sender[T] = newtype T\n", &["m", "source_sender"])?;
    let owner = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "Sender")
        .ok_or("missing source sender")?;
    assert!(matches!(owner.canonical.origin, SymbolOrigin::Module(_)));
    assert!(owner.native_item.is_empty());
    Ok(())
}

/// A checked SDK synchronization wrapper supersedes its synthesized runtime fallback instead of duplicating a name.
#[test]
fn sdk_mutex_retains_one_source_owned_wrapper() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_values_and_calls/passing_channel_senders_and_locks_on.incn"
    );
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let declarations = library_nominal_declarations(checker.type_info());
    let owners = declarations
        .iter()
        .filter(|owner| owner.name == "Mutex")
        .collect::<Vec<_>>();
    assert_eq!(owners.len(), 1, "{owners:?}");
    let owner = owners.first().ok_or("missing synchronization owner")?;
    assert_eq!(
        owner.canonical.kind,
        incan_semantics_core::SemanticSourceTargetKind::Newtype
    );
    assert_eq!(owner.type_parameters, ["T"]);
    assert!(owner.derives.iter().any(|derive| derive == "Clone"));
    Ok(())
}
