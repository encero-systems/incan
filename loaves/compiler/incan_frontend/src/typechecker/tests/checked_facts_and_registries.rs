//! The semantic fact store exported by `type_info` (#1629), registry descriptions and explicit entries (#1004), and
//! `@derive(Descriptor)` shapes.

use super::*;

/// Issue #1004: an imported public registry retains its defining module as the only registry authority.
#[test]
fn imported_registry_description_uses_canonical_catalogue_definition_issue1004() -> Result<(), String> {
    let mut catalog = parse_program(
        r#"
from std.registry import Registry, SubjectKind

@derive(Clone, Eq, Descriptor)
pub model FunctionKey:
  pub name: str

@derive(Clone, Descriptor)
pub model FunctionDescriptor:
  pub deterministic: bool

pub static functions: Registry[FunctionKey, FunctionDescriptor] = Registry.define(
  subjects=[SubjectKind.Function],
)
"#,
        "issue1004 catalog",
    );
    catalog.source_path = Some("/workspace/src/function_catalog.incn".to_string());
    let consumer = parse_program(
        r#"
from std.registry import describe
from function_catalog import FunctionDescriptor, FunctionKey, functions

@describe(
  functions,
  FunctionKey(name="normalize"),
  FunctionDescriptor(deterministic=true)
)
pub def normalize(value: str) -> str:
  return value
"#,
        "issue1004 consumer",
    );
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["normalize".to_string()]));
    checker
        .check_with_imports(&consumer, &[("function_catalog", &catalog)])
        .map_err(|errors| format!("imported catalog @describe should typecheck: {errors:?}"))?;

    assert!(
        checker.type_info().registry.definitions.is_empty(),
        "the consumer must not recreate the catalog registry definition"
    );
    let description = checker
        .type_info()
        .registry
        .descriptions
        .first()
        .ok_or("the consumer should record one checked description")?;
    assert!(
        matches!(
            &description.registry,
            RegistryDescriptionRegistry::Imported { module_path, binding, public }
                if module_path == &["function_catalog".to_string()] && binding == "functions" && *public
        ),
        "description must retain the catalog registry owner, got {:?}",
        description.registry
    );

    let metadata = crate::registry_metadata::collect_checked_registry_metadata(
        checker.type_info(),
        vec!["normalize".to_string()],
        "issue1004",
    );
    assert!(
        metadata.registries.is_empty(),
        "the consumer metadata must not publish a duplicate registry definition"
    );
    assert_eq!(metadata.entries.len(), 1);
    assert_eq!(metadata.entries[0].registry_identity, "function_catalog::functions");
    assert!(metadata.entries[0].registry_public);
    Ok(())
}

#[test]
fn type_info_semantic_fact_store_exports_expression_types_deterministically() -> Result<(), Box<dyn std::error::Error>>
{
    let module_path = vec!["facts".to_string(), "sample".to_string()];
    let info = typecheck_info_for_module(
        r#"
def run() -> int:
  value = 41
  return value + 1
"#,
        module_path.clone(),
        "semantic fact type export",
    )?;

    let facts = info.semantic_fact_store(&module_path);
    let rendered = facts.iter().map(|fact| fact.render_snapshot()).collect::<Vec<_>>();
    let repeated = info.semantic_fact_store(&module_path);
    let structured = facts.iter().cloned().collect::<Vec<_>>();
    let repeated_structured = repeated.iter().cloned().collect::<Vec<_>>();

    assert_eq!(
        structured, repeated_structured,
        "semantic facts should iterate deterministically"
    );
    assert!(
        structured.windows(2).all(|pair| pair[0] <= pair[1]),
        "semantic facts should preserve their structured store order"
    );
    assert!(
        rendered
            .iter()
            .any(|fact| fact.starts_with("expr:facts::sample#") && fact.ends_with(" type=int")),
        "expected at least one expression type fact, got {rendered:?}"
    );
    assert!(
        facts.iter().any(|fact| {
            fact.kind == incan_semantics_core::SemanticFactKind::Type
                && matches!(
                    &fact.value,
                    incan_semantics_core::SemanticFactValue::Type(incan_semantics_core::IncanType::Primitive(
                        incan_semantics_core::IncanPrimitiveType::Int
                    ))
                )
        }),
        "expected a structured int semantic type fact"
    );
    Ok(())
}

#[test]
fn type_info_semantic_fact_store_exports_source_targets() -> Result<(), Box<dyn std::error::Error>> {
    let module_path = vec!["facts".to_string(), "sample".to_string()];
    let info = typecheck_info_for_module(
        r#"
def helper() -> int:
  return 1

def run() -> int:
  return helper()
"#,
        module_path.clone(),
        "semantic fact source target export",
    )?;

    let facts = info.semantic_fact_store(&module_path);
    let rendered = facts.iter().map(|fact| fact.render_snapshot()).collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|fact| fact.contains(" symbol_target=function:facts::sample::helper")),
        "expected helper source-target fact, got {rendered:?}"
    );
    assert!(
        facts.iter().any(|fact| matches!(
            &fact.value,
            incan_semantics_core::SemanticFactValue::SourceTarget(target)
                if target.kind == incan_semantics_core::SemanticSourceTargetKind::Function
                    && target.module_path == vec!["facts".to_string(), "sample".to_string()]
                    && target.name == "helper"
        )),
        "expected structured helper source-target fact"
    );
    Ok(())
}

#[test]
fn semantic_fact_store_owns_stable_nested_declaration_contexts_issue1629() -> Result<(), Box<dyn std::error::Error>> {
    let module_path = vec!["stable_context".to_string()];
    let info = typecheck_info_for_module(
        r#"
def left(value: int) -> int:
  return value

def right(value: int) -> int:
  return value

def sibling_blocks() -> int:
  mut total = 0
  for value in [1]:
    total += value
  for value in [2]:
    total += value
  return total
"#,
        module_path.clone(),
        "stable nested declaration context export",
    )?;

    let facts = info.semantic_fact_store(&module_path);
    let mut contexts = Vec::new();
    for subject in facts.subjects() {
        let Some(identity) = facts.symbol_identities_for(subject).next() else {
            continue;
        };
        let Some(context) = facts.stable_declaration_contexts_for(subject).next() else {
            continue;
        };
        if identity.declaration_name == "value" {
            contexts.push((
                identity.kind.clone(),
                context.owner.declaration_name.clone(),
                context.binding_ordinal,
            ));
        }
    }
    contexts.sort();
    assert_eq!(
        contexts,
        vec![
            (SemanticSourceTargetKind::Local, "sibling_blocks".to_string(), 0),
            (SemanticSourceTargetKind::Local, "sibling_blocks".to_string(), 1),
            (SemanticSourceTargetKind::Parameter, "left".to_string(), 0),
            (SemanticSourceTargetKind::Parameter, "right".to_string(), 0),
        ],
        "the shared semantic projection must own stable owner and collision-local ordinal policy"
    );
    Ok(())
}

#[test]
fn stable_nested_context_counts_unused_sibling_declarations_issue1629() -> Result<(), Box<dyn std::error::Error>> {
    let module_path = vec!["stable_unused_context".to_string()];
    let unused_info = typecheck_info_for_module(
        r#"
def sibling_blocks() -> int:
  mut total = 0
  for value in [1]:
    total += 1
  for value in [2]:
    total += value
  return total
"#,
        module_path.clone(),
        "unused stable nested declaration context export",
    )?;
    let used_info = typecheck_info_for_module(
        r#"
def sibling_blocks() -> int:
  mut total = 0
  for value in [1]:
    total += value
  for value in [2]:
    total += value
  return total
"#,
        module_path.clone(),
        "used stable nested declaration context export",
    )?;

    let unused_facts = unused_info.semantic_fact_store(&module_path);
    let used_facts = used_info.semantic_fact_store(&module_path);
    let value_ordinals = |facts: &incan_semantics_core::SemanticFactStore| {
        facts
            .subjects()
            .filter_map(|subject| {
                let identity = facts.symbol_identities_for(subject).next()?;
                if identity.declaration_name != "value" {
                    return None;
                }
                facts
                    .stable_declaration_contexts_for(subject)
                    .next()
                    .map(|context| context.binding_ordinal)
            })
            .collect::<Vec<_>>()
    };
    let mut unused_ordinals = value_ordinals(&unused_facts);
    let mut used_ordinals = value_ordinals(&used_facts);
    unused_ordinals.sort_unstable();
    used_ordinals.sort_unstable();
    assert_eq!(
        unused_ordinals.len(),
        1,
        "only the used sibling should emit a reference fact"
    );
    assert_eq!(
        unused_ordinals[0], 1,
        "an unused earlier declaration must still reserve its collision-local ordinal"
    );
    assert_eq!(used_ordinals, vec![0, 1]);
    assert_eq!(
        unused_ordinals[0], used_ordinals[1],
        "adding the first use must not rekey the later sibling"
    );
    Ok(())
}

#[test]
fn captured_outer_binding_keeps_its_declaring_named_owner_issue1629() -> Result<(), Box<dyn std::error::Error>> {
    let module_path = vec!["stable_capture_context".to_string()];
    let info = typecheck_info_for_module(
        r#"
def build(value: int) -> int:
  callback = () => value
  return callback()
"#,
        module_path.clone(),
        "captured outer stable declaration context export",
    )?;
    let facts = info.semantic_fact_store(&module_path);
    let owners = facts
        .subjects()
        .filter_map(|subject| {
            let identity = facts.symbol_identities_for(subject).next()?;
            if identity.declaration_name != "value" {
                return None;
            }
            facts
                .stable_declaration_contexts_for(subject)
                .next()
                .map(|context| context.owner.declaration_name.clone())
        })
        .collect::<Vec<_>>();
    assert_eq!(owners, vec!["build".to_string()]);
    Ok(())
}

#[test]
fn type_info_semantic_fact_store_preserves_imported_source_targets() -> Result<(), Box<dyn std::error::Error>> {
    let helper_source = r#"
pub def helper() -> int:
  return 1
"#;
    let main_source = r#"
from helpers import helper

def run() -> int:
  return helper()
"#;
    let helper_tokens = lexer::lex(helper_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let helper_ast = parser::parse(&helper_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let main_tokens = lexer::lex(main_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let main_ast = parser::parse(&main_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_with_imports(&main_ast, &[("helpers", &helper_ast)])
        .map_err(|errs| std::io::Error::other(format!("semantic fact imported target export: {errs:?}")))?;

    let facts = checker.type_info().semantic_fact_store(&module_path);
    assert!(
        facts.iter().any(|fact| matches!(
            &fact.value,
            incan_semantics_core::SemanticFactValue::SourceTarget(target)
                if target.kind == incan_semantics_core::SemanticSourceTargetKind::Function
                    && target.module_path == vec!["helpers".to_string()]
                    && target.name == "helper"
        )),
        "expected imported helper source-target fact"
    );
    Ok(())
}

#[test]
fn type_info_semantic_fact_store_exports_nominal_and_collection_types() -> Result<(), Box<dyn std::error::Error>> {
    let module_path = vec!["facts".to_string(), "nominal".to_string()];
    let info = typecheck_info_for_module(
        r#"
model User:
  name: str

enum Status:
  Active

def first_user(users: list[User]) -> User:
  return users[0]

def current_status() -> Status:
  return Status.Active
"#,
        module_path.clone(),
        "semantic fact nominal type export",
    )?;

    let facts = info.semantic_fact_store(&module_path);
    assert!(
        facts.iter().any(|fact| matches!(
            &fact.value,
            incan_semantics_core::SemanticFactValue::Type(incan_semantics_core::IncanType::Named(name))
                if name == "User"
        )),
        "expected a model nominal semantic type fact"
    );
    assert!(
        facts.iter().any(|fact| matches!(
            &fact.value,
            incan_semantics_core::SemanticFactValue::Type(incan_semantics_core::IncanType::Named(name))
                if name == "Status"
        )),
        "expected an enum nominal semantic type fact"
    );
    assert!(
        facts.iter().any(|fact| matches!(
            &fact.value,
            incan_semantics_core::SemanticFactValue::Type(incan_semantics_core::IncanType::Generic { base, args })
                if base == collection_types::as_str(CollectionTypeId::List)
                    && matches!(
                        args.as_slice(),
                        [incan_semantics_core::IncanType::Named(name)] if name == "User"
                    )
        )),
        "expected a collection semantic type fact for list[User]"
    );
    Ok(())
}

#[test]
fn type_info_semantic_fact_store_exports_function_declaration_type() -> Result<(), Box<dyn std::error::Error>> {
    let module_path = vec!["facts".to_string(), "decls".to_string()];
    let info = typecheck_info_for_module(
        r#"
def add(x: int, y: int = 1) -> int:
  return x + y
"#,
        module_path.clone(),
        "semantic fact declaration type export",
    )?;

    let facts = info.semantic_fact_store(&module_path);
    let add_fact = facts
        .iter()
        .find(|fact| {
            fact.subject.to_string() == "decl:facts::decls::add"
                && fact.kind == incan_semantics_core::SemanticFactKind::Type
        })
        .ok_or("missing add declaration type fact")?;
    let incan_semantics_core::SemanticFactValue::Type(incan_semantics_core::IncanType::Function {
        params,
        return_type,
    }) = &add_fact.value
    else {
        return Err(format!("expected function type fact, got {add_fact:?}").into());
    };

    assert_eq!(params.len(), 2);
    assert_eq!(params[0].name.as_deref(), Some("x"));
    assert!(!params[0].has_default);
    assert_eq!(params[1].name.as_deref(), Some("y"));
    assert!(params[1].has_default);
    assert!(matches!(
        return_type.as_ref(),
        incan_semantics_core::IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int)
    ));
    Ok(())
}

#[test]
fn type_info_semantic_fact_store_exports_checked_registry_descriptions() -> Result<(), Box<dyn std::error::Error>> {
    let module_path = vec!["facts".to_string(), "registry".to_string()];
    let info = typecheck_info_for_module(
        r#"
from std.registry import Registry, SubjectKind, describe

type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
  summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
  subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
def normalize(value: str) -> str:
  return value
"#,
        module_path.clone(),
        "registry fact export",
    )?;

    let facts = info.semantic_fact_store(&module_path);
    let fact = facts
        .iter()
        .find(|fact| {
            fact.subject.to_string() == "decl:facts::registry::normalize"
                && fact.kind == incan_semantics_core::SemanticFactKind::Registry
        })
        .ok_or("missing checked registry description fact")?;
    let incan_semantics_core::SemanticFactValue::RegistryEntry(entry) = &fact.value else {
        return Err(format!("expected registry fact, got {fact:?}").into());
    };
    assert_eq!(entry.registry.to_string(), "decl:facts::registry::functions");
    assert_eq!(
        entry.subject_kind,
        incan_semantics_core::SemanticRegistrySubjectKind::Function
    );
    assert_eq!(entry.subject_identity, "facts::registry.normalize");
    assert!(matches!(
        &entry.key,
        incan_semantics_core::SemanticRegistryValue::Newtype { name, .. } if name == "FunctionId"
    ));
    assert!(matches!(
        &entry.descriptor,
        incan_semantics_core::SemanticRegistryValue::Model { name, fields }
            if name == "FunctionSpec" && fields == &vec![("summary".to_string(), incan_semantics_core::SemanticRegistryValue::String("Normalize text".to_string()))]
    ));
    Ok(())
}

#[test]
fn type_info_semantic_fact_store_exports_checked_registry_method_descriptions() -> Result<(), Box<dyn std::error::Error>>
{
    let module_path = vec!["facts".to_string(), "methods".to_string()];
    let info = typecheck_info_for_module(
        r#"
from std.registry import Registry, SubjectKind, describe

type MethodId = newtype str

@derive(Descriptor)
model MethodSpec:
  summary: str

pub static methods: Registry[MethodId, MethodSpec] = Registry.define(
  subjects=[SubjectKind.Method],
)

model Normalizer:
  @describe(methods, MethodId("normalize"), MethodSpec(summary="Normalize text"))
  def normalize(self, value: str) -> str:
    return value
"#,
        module_path.clone(),
        "registry method fact export",
    )?;

    let facts = info.semantic_fact_store(&module_path);
    let fact = facts
        .iter()
        .find(|fact| {
            fact.subject.to_string() == "decl:facts::methods::Normalizer.normalize"
                && fact.kind == incan_semantics_core::SemanticFactKind::Registry
        })
        .ok_or("missing checked registry method description fact")?;
    let incan_semantics_core::SemanticFactValue::RegistryEntry(entry) = &fact.value else {
        return Err(format!("expected registry fact, got {fact:?}").into());
    };
    assert_eq!(entry.registry.to_string(), "decl:facts::methods::methods");
    assert_eq!(
        entry.subject_kind,
        incan_semantics_core::SemanticRegistrySubjectKind::Method
    );
    assert_eq!(entry.subject_identity, "facts::methods.Normalizer.normalize");
    Ok(())
}

#[test]
fn type_info_semantic_fact_store_exports_explicit_registry_subject_entries() -> Result<(), Box<dyn std::error::Error>> {
    let module_path = vec!["facts".to_string(), "capabilities".to_string()];
    let info = typecheck_info_for_module(
        r#"
from std.registry import Registry, RegistryEntry, RegistrySubject, SubjectKind

type FeatureId = newtype str

@derive(Descriptor)
model CapabilitySpec:
  title: str

pub static capabilities: Registry[FeatureId, CapabilitySpec] = Registry.define(
  subjects=[SubjectKind.CompilationUnit, SubjectKind.Package],
)

pub static unit_capability: RegistryEntry[FeatureId, CapabilitySpec] = capabilities.entry(
  key=FeatureId("unit"),
  subject=RegistrySubject.current_unit(),
  descriptor=CapabilitySpec(title="Current unit"),
)

pub static package_capability: RegistryEntry[FeatureId, CapabilitySpec] = capabilities.entry(
  key=FeatureId("package"),
  subject=RegistrySubject.package(),
  descriptor=CapabilitySpec(title="Current package"),
)
"#,
        module_path.clone(),
        "explicit registry entry fact export",
    )?;

    let facts = info.semantic_fact_store(&module_path);
    assert!(
        info.references.resolved_identities.values().any(|identity| {
            identity.kind == incan_semantics_core::SemanticSourceTargetKind::Method
                && identity.declaration_name == "entry"
        }),
        "the declaration-only Registry.entry validation path must retain its selected source method identity"
    );
    let unit_entry = facts
        .iter()
        .find(|fact| {
            fact.subject.to_string() == "module:facts::capabilities"
                && fact.kind == incan_semantics_core::SemanticFactKind::Registry
        })
        .ok_or("missing checked compilation-unit registry entry")?;
    let incan_semantics_core::SemanticFactValue::RegistryEntry(unit_entry) = &unit_entry.value else {
        return Err(format!("expected registry fact, got {unit_entry:?}").into());
    };
    assert_eq!(
        unit_entry.subject_kind,
        incan_semantics_core::SemanticRegistrySubjectKind::CompilationUnit
    );
    assert_eq!(unit_entry.subject_identity, "facts::capabilities");

    let package_entry = facts
        .iter()
        .find(|fact| {
            fact.subject.to_string() == "package:facts::capabilities::package"
                && fact.kind == incan_semantics_core::SemanticFactKind::Registry
        })
        .ok_or("missing checked package registry entry")?;
    let incan_semantics_core::SemanticFactValue::RegistryEntry(package_entry) = &package_entry.value else {
        return Err(format!("expected registry fact, got {package_entry:?}").into());
    };
    assert_eq!(
        package_entry.subject_kind,
        incan_semantics_core::SemanticRegistrySubjectKind::Package
    );
    assert_eq!(package_entry.subject_identity, "facts::capabilities::package");

    let package_facts = info.semantic_fact_store_with_package(&module_path, Some("capability_catalog"));
    let package_entry = package_facts
        .iter()
        .find(|fact| {
            fact.subject.to_string() == "package:capability_catalog"
                && fact.kind == incan_semantics_core::SemanticFactKind::Registry
        })
        .ok_or("missing package-identified checked registry entry")?;
    let incan_semantics_core::SemanticFactValue::RegistryEntry(package_entry) = &package_entry.value else {
        return Err(format!("expected registry fact, got {package_entry:?}").into());
    };
    assert_eq!(package_entry.subject_identity, "capability_catalog");
    Ok(())
}

#[test]
fn registry_entry_rejects_runtime_and_compiler_reserved_mutation_surfaces() {
    let runtime_entry = check_str_err(
        r#"
from std.registry import Registry, RegistryEntry, RegistrySubject, SubjectKind

@derive(Clone, Eq)
type FeatureId = newtype str

@derive(Descriptor)
model CapabilitySpec:
    title: str

static capabilities: Registry[FeatureId, CapabilitySpec] = Registry.define(
    subjects=[SubjectKind.CompilationUnit],
)

static parenthesized_entry: RegistryEntry[FeatureId, CapabilitySpec] = (
    capabilities.entry(
        key=FeatureId("parenthesized"),
        subject=RegistrySubject.current_unit(),
        descriptor=CapabilitySpec(title="parenthesized"),
    )
)

def register_at_runtime() -> None:
    capabilities.entries.append(
        RegistryEntry(
            key=FeatureId("forged-field"),
            descriptor=CapabilitySpec(title="forged field"),
            subject=RegistrySubject.current_unit(),
        ),
    )
    capabilities.entry(
        key=FeatureId("runtime"),
        subject=RegistrySubject.current_unit(),
        descriptor=CapabilitySpec(title="runtime"),
    )
    capabilities._describe(
        FeatureId("forged"),
        CapabilitySpec(title="forged"),
        RegistrySubject.current_unit(),
    )
"#,
        "Registry.entry must not become a dynamic runtime registration API",
    );
    assert!(
        runtime_entry
            .iter()
            .any(|error| error.message.contains("Registry.entry(...) is declaration-only")),
        "expected declaration-only Registry.entry diagnostic, got: {runtime_entry:?}"
    );
    assert!(
        runtime_entry
            .iter()
            .any(|error| error.message.contains("Field 'entries' on 'Registry' is private")),
        "expected private runtime-entry storage diagnostic, got: {runtime_entry:?}"
    );
    assert!(
        runtime_entry
            .iter()
            .any(|error| error.message.contains("Registry._describe(...) is compiler-reserved")),
        "expected compiler-reserved Registry._describe diagnostic, got: {runtime_entry:?}"
    );

    let compiler_helper = check_str_err(
        r#"
from std.registry import RegistrySubject

def forge_package_subject() -> RegistrySubject:
    return RegistrySubject._checked_package("forged")
"#,
        "compiler-only RegistrySubject constructor must not be callable from source",
    );
    assert!(
        compiler_helper.iter().any(|error| error
            .message
            .contains("RegistrySubject._checked_package(...) is compiler-reserved")),
        "expected compiler-reserved RegistrySubject diagnostic, got: {compiler_helper:?}"
    );
}

#[test]
fn registry_descriptions_reject_dynamic_metadata_expressions() {
    let errors = check_str_err(
        r#"
from std.registry import Registry, SubjectKind, describe

type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
  summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
  subjects=[SubjectKind.Function],
)

def dynamic_key() -> FunctionId:
  return FunctionId("dynamic")

@describe(functions, dynamic_key(), FunctionSpec(summary="Normalize text"))
def normalize(value: str) -> str:
  return value
"#,
        "expected registry structural-value rejection",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("structural calls must construct")),
        "expected structural registry metadata diagnostic, got {errors:?}"
    );
}

#[test]
fn registry_descriptions_reject_trait_method_targets_until_traits_have_canonical_subjects() {
    let errors = check_str_err(
        r#"
from std.registry import Registry, SubjectKind, describe

type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
  summary: str

static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
  subjects=[SubjectKind.Method],
)

trait Normalizer:
  @describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
  def normalize(self, value: str) -> str:
    return value
"#,
        "expected unsupported trait registry description to fail",
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("@describe is currently supported only on concrete functions and methods, not a trait method")),
        "expected a source-target diagnostic for trait @describe, got {errors:?}"
    );
}

#[test]
fn registry_descriptions_expand_structural_const_values() -> Result<(), Box<dyn std::error::Error>> {
    let module_path = vec!["facts".to_string(), "const_registry".to_string()];
    let info = typecheck_info_for_module(
        r#"
from std.registry import Registry, SubjectKind, describe

type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
  forms: FrozenList[str]

const FORMS: FrozenList[str] = ["normalize(value)", "normalize_all(values)"]

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
  subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(forms=FORMS))
def normalize(value: str) -> str:
  return value
"#,
        module_path,
        "registry const descriptor expansion",
    )?;

    let description = info
        .registry
        .descriptions
        .first()
        .ok_or("missing checked registry description")?;
    assert!(matches!(
        &description.descriptor,
        incan_semantics_core::SemanticRegistryValue::Model { fields, .. }
            if fields == &vec![(
                "forms".to_string(),
                incan_semantics_core::SemanticRegistryValue::List(vec![
                    incan_semantics_core::SemanticRegistryValue::String("normalize(value)".to_string()),
                    incan_semantics_core::SemanticRegistryValue::String("normalize_all(values)".to_string()),
                ]),
            )]
    ));
    Ok(())
}

#[test]
fn registry_descriptions_encode_some_as_a_structural_option() -> Result<(), Box<dyn std::error::Error>> {
    let info = typecheck_info_for_module(
        r#"
from std.registry import Registry, SubjectKind, describe

type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
  replacement: Option[str]

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
  subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(replacement=Some("normalized")))
def normalize(value: str) -> str:
  return value
"#,
        vec!["facts".to_string(), "option_registry".to_string()],
        "registry option descriptor snapshot",
    )?;

    let description = info
        .registry
        .descriptions
        .first()
        .ok_or("missing checked registry description")?;
    assert!(matches!(
        &description.descriptor,
        incan_semantics_core::SemanticRegistryValue::Model { fields, .. }
            if fields == &vec![(
                "replacement".to_string(),
                incan_semantics_core::SemanticRegistryValue::Option(Box::new(
                    incan_semantics_core::SemanticRegistryValue::String("normalized".to_string())
                )),
            )]
    ));
    Ok(())
}

#[test]
fn registry_descriptions_encode_concrete_type_tokens() -> Result<(), Box<dyn std::error::Error>> {
    let info = typecheck_info_for_module(
        r#"
from std.registry import Registry, SubjectKind, describe

type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
  target: Type[int]

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
  subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(target=int))
def normalize(value: str) -> str:
  return value
"#,
        vec!["facts".to_string(), "type_token_registry".to_string()],
        "registry type-token descriptor snapshot",
    )?;

    let description = info
        .registry
        .descriptions
        .first()
        .ok_or("missing checked registry description")?;
    assert!(matches!(
        &description.descriptor,
        incan_semantics_core::SemanticRegistryValue::Model { fields, .. }
            if fields == &vec![(
                "target".to_string(),
                incan_semantics_core::SemanticRegistryValue::Type("int".to_string()),
            )]
    ));
    Ok(())
}

#[test]
fn descriptor_derive_rejects_mutable_and_non_descriptor_field_shapes() {
    let errors = check_str_err(
        r#"
@derive(Descriptor)
model BadDescriptor:
  labels: list[str]
  callback: (str) -> str
"#,
        "expected descriptor shape diagnostics",
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("mutable collections are not descriptor snapshots")),
        "expected mutable collection diagnostic, got {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("functions require runtime execution")),
        "expected function field diagnostic, got {errors:?}"
    );
}

#[test]
fn descriptor_derive_accepts_frozen_and_nested_structural_fields() {
    assert_check_ok(
        r#"
type DescriptorId = newtype str

enum Level:
  Info
  Error

@derive(Descriptor)
model Nested:
  label: str

@derive(Descriptor)
model Descriptor:
  id: DescriptorId
  level: Level
  nested: Nested
  tags: FrozenList[str]
  labels: FrozenDict[str, str]
  optional_label: Option[str]
"#,
    );
}

#[test]
fn descriptor_derive_rejects_nested_models_without_descriptor_contract() {
    let errors = check_str_err(
        r#"
model RuntimeState:
  label: str

@derive(Descriptor)
model BadDescriptor:
  state: RuntimeState
"#,
        "expected nested descriptor contract diagnostic",
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("nested models must also use @derive(Descriptor)")),
        "expected nested descriptor contract diagnostic, got {errors:?}"
    );
}
