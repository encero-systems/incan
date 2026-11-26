//! Tests of the checker's owner records: scalar scopes, package and nominal reads, stable dependency identities,
//! restored owners and call provenance.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{
    CheckedOwnerDependencyRead, CheckedOwnerRecordOutput, GraphBodyRecord, GraphBodyRecords,
    checked_graph_record_outputs, checked_owner_record_outputs, stable_dependency_contexts,
};
use crate::library_exports::collect_checked_public_exports;
use crate::library_manifest::LibraryManifest;
use crate::loaf_index::LoafIndex;
use crate::typechecker::TypeChecker;

mod graph_bodies;

#[test]
fn scalar_scope_requires_checked_types_and_a_closed_return() -> Result<(), Box<dyn std::error::Error>> {
    let parsed = program(
        "def leaf() -> str:\n    \"\"\"Keeps documentation out of the lookup scope.\"\"\"\n    return \"hello\"\n\ndef copy(value: int) -> int:\n    return value\n\ndef caller() -> str:\n    return leaf()\n\ndef printed() -> None:\n    println(leaf())\n\ndef add(value: int) -> int:\n    return value + 1\n",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    assert!(super::closed_scalar_scopes(&parsed, &checker).is_empty());
    checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
    let scopes = super::closed_scalar_scopes(&parsed, &checker);
    let slots = parsed
        .declarations
        .iter()
        .map(|item| {
            scopes
                .get(&item.span)
                .map(|names| names.iter().map(String::as_str).collect::<Vec<_>>())
        })
        .collect::<Vec<_>>();
    let none =
        incan_lang::lang::surface::constructors::as_str(incan_lang::lang::surface::constructors::ConstructorId::None);
    assert_eq!(
        slots,
        vec![
            Some(vec!["leaf", "str"]),
            Some(vec!["copy", "i64", "int"]),
            Some(vec!["caller", "leaf", "str"]),
            Some(vec![none, "leaf", "printed", "println"]),
            None,
        ]
    );
    Ok(())
}

/// Parse one focused checker-record fixture through the ordinary source frontend.
fn program(source: &str) -> Result<crate::ast::Program, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    Ok(crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?)
}

/// Build one checked provider artifact so the consumer reads a real package declaration identity.
fn provider_index(source: &str) -> Result<(Arc<LoafIndex>, tempfile::TempDir), Box<dyn std::error::Error>> {
    let provider = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("provider".into()));
    checker.set_current_module_path(Some(vec!["lib".into()]));
    checker
        .check_program(&provider)
        .map_err(|errors| format!("{errors:?}"))?;
    let exports = collect_checked_public_exports(&provider, &checker);
    let manifest = LibraryManifest::from_checked_exports("provider", "0.1.0", &exports);
    let directory = tempfile::tempdir()?;
    std::fs::write(directory.path().join("provider.incnlib"), manifest.to_json_string()?)?;
    let entry = LoafIndex::dependency("provider", "provider", directory.path())?;
    Ok((Arc::new(LoafIndex::new([entry])?), directory))
}

/// A program owner that calls an editable dependency publishes itself and the exact declaration record it read.
#[test]
fn program_owner_publishes_exact_package_declaration_read() -> Result<(), Box<dyn std::error::Error>> {
    let (loaves, _directory) = provider_index("pub def value() -> int:\n    return 41\n")?;
    let source = "from pub::provider import value\n\ndef main() -> int:\n    return value()\n";
    let consumer = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.set_loaves(loaves);
    checker
        .check_program(&consumer)
        .map_err(|errors| format!("{errors:?}"))?;

    let owner = consumer
        .declarations
        .iter()
        .find(|declaration| matches!(declaration.node, crate::ast::Declaration::Function(_)))
        .ok_or("missing consumer owner")?
        .span;
    assert!(
        checker
            .type_info()
            .declarations
            .function_bindings_by_span
            .get(&owner)
            .and_then(|binding| binding.identity.as_ref())
            .is_some(),
        "the checked owner has no canonical binding identity"
    );
    assert!(
        checker.available_declaration_record(owner).is_some(),
        "the checked owner has no declaration record"
    );
    assert!(
        checker.available_local_record(owner).is_some(),
        "the checked owner has no local record: {:?}",
        checker.local_record_refusal(owner)
    );
    assert!(
        checker.available_final_record(owner).is_some(),
        "the checked owner has no final record"
    );
    let owners = checked_owner_record_outputs(&consumer, &checker, &["main".into()])?;
    assert_eq!(owners.len(), 1, "a successful checked owner was silently omitted");
    let CheckedOwnerRecordOutput::Value(owner) = &owners[0] else {
        return Err("the dependency-consuming owner fell back".into());
    };
    assert_eq!(owner.name, "main");
    let read = owner
        .dependency_reads
        .first()
        .ok_or("missing provider dependency read")?;
    assert_eq!(read.package, Some("provider".into()));
    assert_eq!(read.module_path, vec!["lib".to_string()]);
    assert_eq!(read.declaration_name, "value");
    assert_eq!(read.declaration_identity, "function:pub::provider::lib::value");
    assert!(read.reference_subject.starts_with("expr:main#"));
    for source in [
        "def main() -> None:\n    println(\"hello\")\n",
        "def greeting() -> str:\n    return \"hello\"\n\ndef main() -> None:\n    println(greeting())\n",
        "def increment(value: int) -> int:\n    return value + 1\n\npub def main() -> None:\n    println(increment(increment(1)))\n",
    ] {
        reusable_ordinary_calls(source)?;
    }
    let base = "def value() -> int:\n    return 1\n\ndef main() -> None:\n    println(value())\n";
    let signature = local_callee_identity(base)?;
    assert_eq!(signature, local_callee_identity(&base.replace("return 1", "return 2"))?);
    assert_ne!(
        signature,
        local_callee_identity(&base.replace("-> int", "-> str").replace("return 1", "return \"one\""))?
    );
    Ok(())
}

/// Public-package fields with the same spelling retain the selected model in each dependency identity.
#[test]
fn public_package_member_reads_keep_their_selected_nominal() -> Result<(), Box<dyn std::error::Error>> {
    let provider = "pub model First:\n    pub pin: Option[int] = None\n\n    pub def message(self) -> str:\n        return \"first\"\n\npub model Second:\n    pub pin: Option[int] = None\n\n    pub def message(self) -> str:\n        return \"second\"\n";
    let (loaves, _directory) = provider_index(provider)?;
    let source = "from pub::provider import First, Second\n\ndef first(value: First) -> Option[int]:\n    return value.pin\n\ndef second(value: Second) -> Option[int]:\n    return value.pin\n\ndef first_message(value: First) -> str:\n    return value.message()\n\ndef second_message(value: Second) -> str:\n    return value.message()\n";
    let consumer = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.set_loaves(loaves);
    checker
        .check_program(&consumer)
        .map_err(|errors| format!("{errors:?}"))?;
    let facts = checker.type_info().semantic_fact_store(&["main".into()]);
    let contexts = stable_dependency_contexts(&facts);
    let mut reads = contexts
        .iter()
        .filter(|(identity, _)| identity.declaration_name == "pin" || identity.declaration_name == "message")
        .filter_map(|(identity, context)| {
            checker
                .stable_dependency_identity_with_context(identity, Some(context))
                .map(|identity| identity.render_compact())
        })
        .collect::<Vec<_>>();
    reads.sort();
    assert_eq!(
        reads,
        vec![
            "member/field:pub::provider::lib::pin#in(model:pub::provider::lib::First)[0]",
            "member/field:pub::provider::lib::pin#in(model:pub::provider::lib::Second)[0]",
            "member/method:pub::provider::lib::message#in(model:pub::provider::lib::First)[0]",
            "member/method:pub::provider::lib::message#in(model:pub::provider::lib::Second)[0]",
        ]
    );
    Ok(())
}

/// An editable sibling enum variant keeps the provider enum as its stable owner, even when another top-level
/// declaration has the variant's spelling.
#[test]
fn sibling_nominal_member_stable_dependency_identity_keeps_owner() -> Result<(), Box<dyn std::error::Error>> {
    let provider = program(
        "pub model Cancelled:\n    reason: str\n\npub model First:\n    pub transaction: str\n\npub model Second:\n    pub transaction: str\n\npub enum CompletionStatus:\n    Success\n    Failed\n    Cancelled\n",
    )?;
    let module_path = vec!["engine_messages".into()];
    let mut provider_checker = TypeChecker::new();
    provider_checker.set_current_module_path(Some(module_path.clone()));
    provider_checker.register_dependency_module_path_segments("engine_messages", module_path.clone());
    let interface = provider_checker
        .build_dependency_interface_component(
            &[(
                "engine_messages",
                &provider,
                Arc::from(crate::module::exported_symbols(&provider)),
            )],
            &[],
            true,
        )
        .pop()
        .ok_or("provider namespace was not publishable")?;

    let consumer = program(
        "from engine_messages import CompletionStatus, First, Second\n\ndef selected() -> CompletionStatus:\n    return CompletionStatus.Failed\n\ndef cancelled() -> CompletionStatus:\n    return CompletionStatus.Cancelled\n\ndef first(value: First) -> str:\n    return value.transaction\n\ndef second(value: Second) -> str:\n    return value.transaction\n",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.register_dependency_module_path_segments("engine_messages", module_path);
    checker.select_dependency_interfaces(vec![interface]);
    checker
        .check_with_imports(&consumer, &[("engine_messages", &provider)])
        .map_err(|errors| format!("{errors:?}"))?;

    let mut reads = checker
        .symbols
        .all_symbols()
        .iter()
        .flat_map(|symbol| match &symbol.kind {
            crate::symbols::SymbolKind::Type(crate::symbols::TypeInfo::Enum(info)) => {
                info.variant_identities.values().collect::<Vec<_>>()
            }
            crate::symbols::SymbolKind::Type(crate::symbols::TypeInfo::Model(info)) => info
                .fields
                .values()
                .filter_map(|field| field.identity.as_ref())
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .filter(|identity| {
            identity.declaration_name == "Failed"
                || identity.declaration_name == "Cancelled"
                || identity.declaration_name == "transaction"
        })
        .map(|identity| {
            checker
                .stable_dependency_identity(identity)
                .map(|stable| stable.render_compact())
                .ok_or_else(|| "missing variant dependency identity".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    reads.sort();
    reads.dedup();
    assert_eq!(
        reads,
        vec![
            "member/field:engine_messages::transaction#in(model:engine_messages::First)[0]",
            "member/field:engine_messages::transaction#in(model:engine_messages::Second)[0]",
            "member/variant:engine_messages::Cancelled#in(enum:engine_messages::CompletionStatus)[0]",
            "member/variant:engine_messages::Failed#in(enum:engine_messages::CompletionStatus)[0]",
        ]
    );
    let facts = checker.type_info().semantic_fact_store(&["main".into()]);
    let contexts = stable_dependency_contexts(&facts);
    let mut field_reads = contexts
        .iter()
        .filter(|(identity, _)| identity.declaration_name == "transaction")
        .filter_map(|(identity, context)| {
            checker
                .stable_dependency_identity_with_context(identity, Some(context))
                .map(|identity| identity.render_compact())
        })
        .collect::<Vec<_>>();
    field_reads.sort();
    assert_eq!(
        field_reads,
        vec![
            "member/field:engine_messages::transaction#in(model:engine_messages::First)[0]",
            "member/field:engine_messages::transaction#in(model:engine_messages::Second)[0]",
        ]
    );
    Ok(())
}

/// A generated field identity retains its checked enclosing declaration when same-spelled defaults coexist.
#[test]
fn local_field_reads_keep_their_stable_declaration_owner() -> Result<(), Box<dyn std::error::Error>> {
    let program = program(
        "pub model ProgramOutcome:\n    pub transaction: Option[int] = None\n\npub model NativeSelection:\n    pub transaction: Option[int] = None\n",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["engine_programs".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let owner = checker
        .symbols
        .lookup("ProgramOutcome")
        .and_then(|symbol| checker.symbols.identity_of(symbol))
        .cloned()
        .ok_or("missing ProgramOutcome identity")?;
    let dependency = owner.generated_member("transaction", incan_semantics_core::SemanticSourceTargetKind::Field);
    let context = incan_semantics_core::CanonicalStableDeclarationContext {
        owner: owner.clone(),
        binding_ordinal: 0,
    };
    assert_eq!(
        checker
            .stable_dependency_identity_with_context(&dependency, Some(&context))
            .map(|identity| identity.render_compact()),
        Some("member/field:engine_programs::transaction#in(model:engine_programs::ProgramOutcome)[0]".into())
    );
    checker
        .type_info
        .record_compiler_generated_member_identity(dependency.clone(), owner);
    assert_eq!(
        checker
            .stable_dependency_identity(&dependency)
            .map(|identity| identity.render_compact()),
        Some("member/field:engine_programs::transaction#in(model:engine_programs::ProgramOutcome)[0]".into())
    );
    Ok(())
}

/// A body-local parameter read, as in Oven's `apple_sdk.retained_sdk`, is not an external source dependency.
#[test]
fn body_parameter_reads_stay_inside_their_owner() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def retained_sdk(basis: str) -> Result[str, str]:\n    if basis != \"selected\":\n        return Err(\"the developer selection changed\")\n    return Ok(basis)\n";
    let program = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["apple_sdk".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let outputs = checked_owner_record_outputs(&program, &checker, &["apple_sdk".into()])?;
    let CheckedOwnerRecordOutput::Value(owner) = &outputs[0] else {
        return Err("retained_sdk did not publish a checked owner".into());
    };
    assert!(
        owner.dependency_reads.is_empty(),
        "body-local parameter escaped as a source dependency: {:?}",
        owner.dependency_reads
    );
    Ok(())
}

/// A local call publishes exactly its callee's checked signature identity, independently of the callee's body.
fn local_callee_identity(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let parsed = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
    let outputs = checked_owner_record_outputs(&parsed, &checker, &["main".into()])?;
    let mut values = BTreeMap::new();
    for output in outputs {
        let CheckedOwnerRecordOutput::Value(value) = output else {
            return Err("local callee signature fixture fell back".into());
        };
        values.insert(value.name.clone(), value);
    }
    let callee = values.get("value").ok_or("missing callee")?;
    let caller = values.get("main").ok_or("missing caller")?;
    assert_eq!(caller.dependency_reads.len(), 1);
    let read = caller.dependency_reads.first().ok_or("missing callee read")?;
    assert_eq!(read.package, None);
    assert_eq!(read.declaration_identity, callee.identity);
    Ok(read.declaration_identity.clone())
}

/// A checker receipt owns only references in its body; a callee's dependencies belong to that callee's receipt.
#[test]
fn checked_owner_records_publish_only_direct_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def leaf() -> int:\n    return 1\n\ndef middle() -> int:\n    return leaf()\n\ndef main() -> None:\n    println(middle())\n";
    let parsed = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
    let outputs = checked_owner_record_outputs(&parsed, &checker, &["main".into()])?;
    let mut reads = BTreeMap::new();
    for output in outputs {
        let CheckedOwnerRecordOutput::Value(value) = output else {
            return Err("direct dependency fixture fell back".into());
        };
        reads.insert(
            value.name,
            value
                .dependency_reads
                .into_iter()
                .map(|read| read.declaration_name)
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(reads.get("leaf"), Some(&Vec::new()));
    assert_eq!(reads.get("middle"), Some(&vec!["leaf".into()]));
    assert_eq!(reads.get("main"), Some(&vec!["middle".into()]));
    Ok(())
}

/// Adding a third specialization changes the exact receiver membership while preserving the two prior members.
#[test]
fn checked_overload_reader_names_the_candidates_it_consulted() -> Result<(), Box<dyn std::error::Error>> {
    let first = "trait Describe[T]:\n    def describe(self, value: T) -> str\n\nmodel Describable with Describe[str], Describe[int]:\n    def describe(self, value: str) -> str:\n        return \"item\"\n    def describe(self, value: int) -> str:\n        return \"integer\"\n";
    let third = "    def describe(self, value: FrozenStr) -> str:\n        return \"frozen\"\n";
    let reader = "\ndef read_description(item: Describable, message: str) -> str:\n    return item.describe(message)\n";
    let mut prior = Vec::new();
    for (source, count) in [
        (format!("{first}{reader}"), 2),
        (
            format!(
                "{}{third}{reader}",
                first.replace("Describe[int]:", "Describe[int], Describe[FrozenStr]:")
            ),
            3,
        ),
    ] {
        let parsed = program(&source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(vec!["main".to_owned()]));
        checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
        let span = parsed.declarations.last().ok_or("missing reader")?.span;
        let reads = checker
            .checked_method_lookups(span)
            .ok_or("incomplete method-membership evidence")?;
        assert_eq!(reads.len(), 1, "missing read for {count} declarations");
        let read = reads.first().ok_or("missing overload set")?;
        assert_eq!(read.receiver, "Describable");
        assert_eq!(read.method, "describe");
        assert_eq!(read.module, ["main"]);
        assert_eq!(read.candidates.len(), count);
        assert!(prior.iter().all(|member| read.candidates.contains(member)));
        prior = read.candidates.clone();
    }
    Ok(())
}

/// Frame raw producer headers the same way as the Incan codec's native-fields projection.
fn selected_graph_record(
    mut owner: super::CheckedOwnerRecordFields,
) -> Result<super::GraphBodyRecord, Box<dyn std::error::Error>> {
    for (fields, family) in [(&mut owner.local, "local"), (&mut owner.final_value, "final")] {
        fields
            .first_mut()
            .ok_or("missing checked record header")?
            .insert(3, family.into());
    }
    Ok(super::GraphBodyRecord {
        identity: owner.identity,
        local: owner.local.into(),
        final_value: owner.final_value.into(),
        body_ir: None,
    })
}

/// A ranked check enters the demanded statements once and declines an absent peer before its statement producer.
#[test]
fn ranked_reader_counts_actual_statement_entries() -> Result<(), Box<dyn std::error::Error>> {
    let source = program(concat!(
        "model Peer:\n",
        "    value: int\n",
        "    def read(self) -> int:\n        return self.value\n",
        "    property doubled -> int:\n        return self.value * 2\n\n",
        "def left(value: int) -> int:\n    return value + 1\n\n",
        "def right(value: int) -> int:\n    return value * 2\n",
    ))?;
    let module = vec!["main".to_owned()];
    let mut cold = TypeChecker::new();
    cold.set_current_module_path(Some(module.clone()));
    cold.check_program(&source).map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(cold.check_work().method_bodies, 1);
    assert_eq!(cold.check_work().property_bodies, 1);
    let mut prior = Vec::new();
    let mut target = None;
    for output in checked_owner_record_outputs(&source, &cold, &module)? {
        let CheckedOwnerRecordOutput::Value(owner) = output else {
            return Err("ranked fixture fell back".into());
        };
        if owner.name == "left" {
            target = Some(owner.identity.clone());
        }
        prior.push(selected_graph_record(owner)?);
    }
    let target = target.ok_or("missing ranked fixture owner")?;
    for (records, missing) in [(prior, false), (Vec::new(), true)] {
        let scope = super::GraphBodyRecords::new(records)?.for_ranked_owner(&target);
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module.clone()));
        checker.set_graph_body_records(scope.clone());
        let result = checker.check_program(&source);
        assert_eq!(result.is_err(), missing, "{result:?}");
        assert_eq!(checker.check_work().body_local_checks, 1);
        assert_eq!(checker.check_work().method_bodies, 0);
        assert_eq!(checker.check_work().property_bodies, 0);
        assert_eq!(scope.ranked_admission(), Some((1, missing, false)));
    }
    Ok(())
}

/// Builtins and nested ordinary calls retain their complete tooling provenance when a shifted owner reuses
/// graph records. The independent cold checker supplies expected facts, never reconstruction inputs.
fn reusable_ordinary_calls(source: &str) -> Result<(), Box<dyn std::error::Error>> {
    let consumer = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker
        .check_program(&consumer)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut records = Vec::new();
    for owner in checked_owner_record_outputs(&consumer, &checker, &["main".into()])? {
        let CheckedOwnerRecordOutput::Value(owner) = owner else {
            let CheckedOwnerRecordOutput::Fallback(fallback) = owner else {
                return Err("missing checker output".into());
            };
            return Err(format!("ordinary owner {} fell back: {}", fallback.name, fallback.reason).into());
        };
        records.push(selected_graph_record(owner)?);
    }
    let moved = program(&format!("# shifted declaration\n{source}"))?;
    let mut selected = TypeChecker::new();
    selected.set_current_module_path(Some(vec!["main".into()]));
    selected.set_graph_body_records(super::GraphBodyRecords::new(records)?);
    selected.check_program(&moved).map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(selected.check_work().function_bodies, 0);
    assert_eq!(selected.check_work().local_record_evaluations, 0);
    let mut cold = TypeChecker::new();
    cold.set_current_module_path(Some(vec!["main".into()]));
    cold.check_program(&moved).map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(
        selected.type_info().c_abi.function_calls,
        cold.type_info().c_abi.function_calls
    );
    Ok(())
}

/// Same-name closure parameters bind to distinct structural ordinals when restoring a selected body.
#[test]
fn restores_each_closure_parameter_in_its_current_source_position() -> Result<(), Box<dyn std::error::Error>> {
    reusable_ordinary_calls(concat!(
        "def normalize(value: Result[int, str]) -> Result[int, str]:\n",
        "    number = value.map_err((error) => error)?\n",
        "    return value.map_err((error) => error)\n",
    ))
}

/// An aliased Rust item is bound to its registered source import. A different module's item with the
/// same leaf and local spelling must cause a new check before its reference is published.
#[test]
fn registered_rust_item_reads_require_the_current_import() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from rust::std::fmt import Error as NativeError\n\ndef read_item() -> None:\n    _ = NativeError\n";
    let module = vec!["main".to_owned()];
    let check = |program: &crate::ast::Program, records| -> Result<TypeChecker, String> {
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module.clone()));
        checker.set_graph_body_records(super::GraphBodyRecords::new(records).map_err(|error| error.to_string())?);
        checker.check_program(program).map_err(|errors| format!("{errors:?}"))?;
        Ok(checker)
    };
    let original = program(source)?;
    let prior = check(&original, Vec::new())?;
    let outputs = checked_owner_record_outputs(&original, &prior, &module)?;
    let [CheckedOwnerRecordOutput::Value(fields)] = outputs.as_slice() else {
        return Err("missing checked record for a registered Rust item read".into());
    };
    let record = selected_graph_record(fields.clone())?;
    let reused = check(&original, vec![record.clone()])?;
    assert_eq!(reused.check_work().function_bodies, 0);
    assert_eq!(
        reused.type_info().references.resolved_identities,
        prior.type_info().references.resolved_identities
    );
    let changed = program(&source.replace("std::fmt", "std::io"))?;
    let checked = check(&changed, vec![record])?;
    let fresh = check(&changed, Vec::new())?;
    assert_eq!(checked.check_work().function_bodies, 1);
    assert_eq!(
        checked.type_info().references.resolved_identities,
        fresh.type_info().references.resolved_identities
    );
    assert_ne!(
        checked.type_info().references.resolved_identities,
        prior.type_info().references.resolved_identities
    );
    Ok(())
}

/// The interpolation's outer range includes its braces while the callee's range starts at its identifier.
#[test]
fn preserves_nested_call_provenance_inside_an_interpolation() -> Result<(), Box<dyn std::error::Error>> {
    reusable_ordinary_calls(concat!(
        "def inner(value: int) -> int:\n    return value + 1\n\n",
        "def outer(value: int) -> int:\n    return value + 2\n\n",
        "def rendered(value: int) -> str:\n    return f\"result: {outer(inner(value))}\"\n",
    ))
}

/// A generic function, its neighbor and the caller that instantiates it at two types all publish reusable values,
/// so a shifted program re-checks none of their bodies.
#[test]
fn generic_owner_and_its_caller_publish_reusable_values() -> Result<(), Box<dyn std::error::Error>> {
    reusable_ordinary_calls(concat!(
        "def pick[T](items: list[T]) -> T:\n    return items[0]\n\n",
        "def unrelated(value: int) -> int:\n    return value + 1\n\n",
        "def main() -> None:\n    println(pick([1, 2, 3]))\n    println(pick([\"a\", \"b\", \"c\"]))\n",
        "    println(unrelated(41))\n",
    ))
}

/// A registry-only builtin has no physical root-scope symbol to supply its restored callee identity.
#[test]
fn registry_builtin_callee_restores_after_source_moves() -> Result<(), Box<dyn std::error::Error>> {
    reusable_ordinary_calls("def main() -> None:\n    println(json_stringify(1))\n")
}

/// A nominal expression retains its canonical type identity without excluding an otherwise reusable owner.
#[test]
fn nominal_expression_type_identities_publish_reusable_owner() -> Result<(), Box<dyn std::error::Error>> {
    let (loaves, _directory) = provider_index(
        "pub model Choice:\n    pub value: int\n\npub def value() -> Choice:\n    return Choice(value=41)\n",
    )?;
    let source = "from pub::provider import value\n\ndef keep() -> int:\n    choice = value()\n    return 1\n";
    let program = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.set_loaves(loaves.clone());
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;

    let outputs = checked_owner_record_outputs(&program, &checker, &["main".into()])?;
    assert_eq!(outputs.len(), 1, "the nominal owner was silently omitted");
    let CheckedOwnerRecordOutput::Value(owner) = &outputs[0] else {
        let CheckedOwnerRecordOutput::Fallback(fallback) = &outputs[0] else {
            return Err("the nominal expression owner had no publication decision".into());
        };
        return Err(format!("the nominal expression owner fell back: {}", fallback.reason).into());
    };
    assert_eq!(owner.name, "keep");
    assert!(
        super::verify_fixture_with_loaves(source, "keep", owner.final_value.clone(), loaves, vec!["main".into()],)?
            .is_empty()
    );
    Ok(())
}

/// Both isolated producers derive the qualified owner and match the independently checked module byte for byte.
#[test]
fn scoped_producers_recompute_a_qualified_project_owner() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def supported_schema(version: str) -> bool:\n    return version == \"1\"\n";
    let module = vec!["project".to_owned(), "importer".to_owned()];
    let parsed = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module.clone()));
    checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
    let outputs = checked_owner_record_outputs(&parsed, &checker, &module)?;
    let [CheckedOwnerRecordOutput::Value(owner)] = outputs.as_slice() else {
        return Err("expected one reusable project owner".into());
    };
    let mut local = super::produce_local_record(source, "supported_schema", owner.declaration.clone(), vec![])?;
    assert_eq!(local, owner.local);
    local
        .first_mut()
        .ok_or("missing local header")?
        .insert(3, "local".into());
    let settled = super::produce_final_record(source, "supported_schema", local, vec![])?;
    assert_eq!(settled, owner.final_value);
    let before = "def supported_schema(version: int) -> bool:\n    return version >= 1 and version <= 9\n";
    let after = before.replace("version", "schema_version");
    let checked = |source: &str| -> Result<super::CheckedOwnerRecordFields, Box<dyn std::error::Error>> {
        let parsed = program(source)?;
        let mut checker = TypeChecker::new();
        checker.set_current_module_path(Some(module.clone()));
        checker.check_program(&parsed).map_err(|errors| format!("{errors:?}"))?;
        let outputs = checked_owner_record_outputs(&parsed, &checker, &module)?;
        let [CheckedOwnerRecordOutput::Value(output)] = outputs.as_slice() else {
            return Err("missing closed owner".into());
        };
        Ok(output.clone())
    };
    let previous = checked(before)?;
    let changed = checked(&after)?;
    assert_eq!(previous.identity, changed.identity);
    let fresh = super::produce_closed_declaration(&after, "supported_schema", previous.declaration.clone())?
        .ok_or("missing independently checked declaration")?;
    assert_eq!(fresh, changed.declaration);
    assert_eq!(
        super::produce_local_record(&after, "supported_schema", fresh, vec![])?,
        changed.local
    );
    let nonlocal = after.replace("schema_version >= 1 and schema_version <= 9", "check(schema_version)");
    assert!(super::produce_closed_declaration(&nonlocal, "supported_schema", previous.declaration)?.is_none());
    Ok(())
}

/// After a provider edit shifts its variant, restoring the consumer must bind to the variant's current span.
#[test]
fn restored_enum_member_uses_the_current_provider_anchor() -> Result<(), Box<dyn std::error::Error>> {
    let provider = "pub enum Failure:\n    Missing\n    Type\n";
    let (original, _first) = provider_index(provider)?;
    let source = "from pub::provider import Failure\n\ndef kind() -> Failure:\n    return Failure.Type\n";
    let consumer = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.set_loaves(original);
    checker
        .check_program(&consumer)
        .map_err(|errors| format!("{errors:?}"))?;
    let outputs = checked_owner_record_outputs(&consumer, &checker, &["main".into()])?;
    let [CheckedOwnerRecordOutput::Value(owner)] = outputs.as_slice() else {
        return Err("expected one reusable consumer of the package variant".into());
    };
    assert!(
        owner
            .final_value
            .iter()
            .any(|row| row.first().is_some_and(|tag| tag == "identity-context")
                && row.last().is_some_and(|name| name == "Type"))
    );
    let (current, _second) = provider_index(&provider.replace("    Type", "    # a moved declaration\n    Type"))?;
    assert!(
        super::verify_fixture_with_loaves(source, "kind", owner.final_value.clone(), current, vec!["main".into()])?
            .is_empty()
    );
    Ok(())
}

/// Imported method calls must resolve to the current executable declaration after a provider edit.
#[test]
fn restored_method_uses_its_declaration_inside_the_current_nominal() -> Result<(), Box<dyn std::error::Error>> {
    let provider = "pub model Failure:\n    code: int\n\n    pub def message(self) -> int:\n        return self.code\n";
    let source =
        "from pub::provider import Failure\n\ndef result(error: Failure) -> int:\n    return error.message()\n";
    let (original, _original_files) = provider_index(provider)?;
    let consumer = program(source)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker.set_loaves(original);
    checker
        .check_program(&consumer)
        .map_err(|errors| format!("{errors:?}"))?;
    let outputs = checked_owner_record_outputs(&consumer, &checker, &["main".into()])?;
    let [CheckedOwnerRecordOutput::Value(owner)] = outputs.as_slice() else {
        return Err("expected one reusable consumer of the package method".into());
    };
    let changed = provider.replace(
        "    pub def message",
        "    # change only the method locator\n    pub def message",
    );
    let (current, _current_files) = provider_index(&changed)?;
    let differences = super::verify_fixture_with_loaves(
        source,
        "result",
        owner.final_value.clone(),
        current,
        vec!["main".into()],
    )?;
    assert!(differences.is_empty(), "{differences:?}");
    Ok(())
}

/// Repeated references to an indirectly returned nominal retain each call site and one current provider method.
#[test]
fn restored_repeated_method_on_an_imported_return_type_skips_statements() -> Result<(), Box<dyn std::error::Error>> {
    let provider = "pub model Failure:\n    pub code: int\n\n    pub def message(self) -> int:\n        return self.code\n\npub def failure() -> Failure:\n    return Failure(code=3)\n";
    let source = "from pub::provider import failure\n\ndef result() -> int:\n    error = failure()\n    return error.message() + error.message()\n";
    let consumer = program(source)?;
    let (original, _original_files) = provider_index(provider)?;
    let mut prior = TypeChecker::new();
    prior.set_current_module_path(Some(vec!["main".into()]));
    prior.set_loaves(original);
    prior.check_program(&consumer).map_err(|errors| format!("{errors:?}"))?;
    let outputs = checked_owner_record_outputs(&consumer, &prior, &["main".into()])?;
    let [CheckedOwnerRecordOutput::Value(owner)] = outputs.as_slice() else {
        return Err("expected the indirect package method caller to publish records".into());
    };
    let (current, _current_files) =
        provider_index(&provider.replace("    pub def message", "    # new coordinates\n    pub def message"))?;
    let mut fresh = TypeChecker::new();
    fresh.set_current_module_path(Some(vec!["main".into()]));
    fresh.set_loaves(current.clone());
    fresh.check_program(&consumer).map_err(|errors| format!("{errors:?}"))?;
    let mut warm = TypeChecker::new();
    warm.set_current_module_path(Some(vec!["main".into()]));
    warm.set_loaves(current);
    warm.set_graph_body_records(super::GraphBodyRecords::new(vec![selected_graph_record(
        owner.clone(),
    )?])?);
    warm.check_program(&consumer).map_err(|errors| format!("{errors:?}"))?;
    assert_eq!(warm.check_work().function_bodies, 0);
    assert_eq!(
        warm.type_info().references.resolved_identities,
        fresh.type_info().references.resolved_identities
    );
    Ok(())
}
