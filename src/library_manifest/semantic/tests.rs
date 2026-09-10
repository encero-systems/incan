//! Pure input-contract controls; no file access, publisher, SDK or native selection.

use super::*;
use crate::library_manifest::{NativeCompilerSupportRequirement, NativeSourceInput, NativeSourcePackage};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Give fixture inputs visibly distinct valid physical identities.
fn digest(digit: char) -> String {
    format!("sha256:{}", digit.to_string().repeat(64))
}

/// Give fixtures an explicit producer domain without consulting a host or selected native plan.
fn activation(features: &BTreeSet<String>, default_features: bool) -> SemanticActivationContext<'_> {
    SemanticActivationContext {
        schema_version: 1,
        host: "aarch64-apple-darwin",
        target: "aarch64-apple-darwin",
        purpose: SemanticActivationPurpose::Normal,
        build_unit_present: false,
        features,
        default_features,
    }
}

/// Own one checked provider's original inputs while individual tests borrow its bindings.
struct Fixture {
    identity: ProviderIdentity,
    manifest: LibraryManifest,
    definition: NativeSourceUnitDefinition,
}

impl Fixture {
    /// Construct a provider with the actual mandatory emitted stdlib support request.
    fn new(name: &str) -> Self {
        let mut manifest = LibraryManifest::new(name, "1.0.0");
        manifest.contract_metadata.provider.semantic_source_digest = Some(digest('a'));
        Self {
            identity: ProviderIdentity {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                digest: digest('b'),
                feature_projection: BTreeSet::new(),
            },
            manifest,
            definition: NativeSourceUnitDefinition {
                schema_version: 2,
                package: NativeSourcePackage {
                    name: name.to_string(),
                    version: "1.0.0".to_string(),
                },
                crate_name: name.to_string(),
                crate_kind: NativeSourceCrateKind::Rlib,
                edition: "2021".to_string(),
                authored_source_digest: digest('a'),
                entrypoint: NativeSourceInput {
                    path: "src/lib.rs".to_string(),
                    digest: digest('c'),
                },
                source_tree: NativeSourceInput {
                    path: "src".to_string(),
                    digest: digest('d'),
                },
                source_members: None,
                requirements: Vec::new(),
                compiler_support: Some(vec![NativeCompilerSupportRequirement {
                    support: NativeCompilerSupport::Stdlib,
                    features: Vec::new(),
                }]),
            },
        }
    }

    /// Borrow the exact original records, leaving each explicit binding table empty.
    fn input(&self) -> ProviderSemanticInputs<'_> {
        ProviderSemanticInputs {
            activation: activation(&self.identity.feature_projection, false),
            identity: &self.identity,
            manifest: &self.manifest,
            definition: Some(&self.definition),
            edges: &[],
            requirements: &[],
            compiler_support: &[],
            origins: &[],
        }
    }
    /// Bind actual stdlib evidence before invoking the projection; no native candidate participates.
    fn project(
        &self,
        context: &mut SemanticProjectionBuilder,
    ) -> Result<ProviderSemanticDigest, SemanticProjectionError> {
        self.project_with(context, &[], &[], &[])
    }

    /// Supply extra original requirement, graph and type-origin associations alongside mandatory compiler support.
    fn project_with(
        &self,
        context: &mut SemanticProjectionBuilder,
        requirements: &[ProviderRequirementBinding<'_>],
        edges: &[ProviderSemanticEdge<'_>],
        origins: &[&ProviderSemanticDigest],
    ) -> Result<ProviderSemanticDigest, SemanticProjectionError> {
        let source = support_leaf('4')?;
        let definition_digest = definition_binding_digest(&self.definition)?;
        let support = [ProviderSupportBinding {
            definition_digest: &definition_digest,
            support_index: 0,
            selected: &source,
        }];
        let mut input = self.input();
        input.compiler_support = &support;
        input.requirements = requirements;
        input.edges = edges;
        input.origins = origins;
        context.register(&input)
    }
}

/// Produce a fully specified registry-source leaf through the same pure versioned input contract.
fn rust_leaf(source_digit: char, features: BTreeSet<String>) -> Result<RustSemanticDigest, SemanticProjectionError> {
    let files = BTreeMap::from([("src/lib.rs".to_string(), digest(source_digit))]);
    digest_rust_source_inputs(&RustSemanticInputs {
        activation: activation(&features, true),
        contract: RUST_SEMANTIC_INPUT_CONTRACT,
        contract_version: 1,
        package: "regex",
        version: "1.2.0",
        crate_name: "regex",
        crate_kind: NativeSourceCrateKind::Rlib,
        edition: "2021",
        entrypoint: "src/lib.rs",
        source: RustSemanticSource::Registry {
            registry: "registry+example",
            checksum: &"a".repeat(64),
        },
        files: &files,
        configuration: &BTreeMap::new(),
        features: &features,
        default_features: true,
        dependencies: &[],
        selections: &[],
    })
}

/// Retain a normal request's actual version, alias and feature dimensions.
fn registry_request() -> NativeSourceRequirement {
    NativeSourceRequirement {
        role: NativeRequirementRole::Normal,
        alias: "pattern".to_string(),
        package: Some("regex".to_string()),
        version_requirement: Some("^1".to_string()),
        features: Vec::new(),
        default_features: true,
        optional: false,
        source: NativeRequirementSource::RegistryRequest,
    }
}

/// Relocation has no input path to hash; physical generated evidence stays outside semantic output.
#[test]
fn relocation_equal_and_authored_source_unequal() -> TestResult {
    let first = Fixture::new("provider");
    let mut second = Fixture::new("provider");
    second.identity.digest = digest('e');
    second.definition.entrypoint.digest = digest('f');
    let a = first.project(&mut SemanticProjectionBuilder::new())?;
    let b = second.project(&mut SemanticProjectionBuilder::new())?;
    assert_eq!(a.value(), b.value());
    second.definition.authored_source_digest = digest('1');
    second.manifest.contract_metadata.provider.semantic_source_digest = Some(digest('1'));
    assert_ne!(
        a.value(),
        second.project(&mut SemanticProjectionBuilder::new())?.value()
    );
    Ok(())
}

/// Selected source bytes and features both affect the provider without hashing physical locations.
#[test]
fn selected_rust_source_and_feature_changes_are_unequal() -> TestResult {
    let mut fixture = Fixture::new("provider");
    fixture.definition.requirements.push(registry_request());
    let definition_digest = definition_binding_digest(&fixture.definition)?;
    let first = rust_leaf('2', BTreeSet::new())?;
    let changed_source = rust_leaf('3', BTreeSet::new())?;
    let changed_features = rust_leaf('2', BTreeSet::from(["unicode".to_string()]))?;
    let project = |source: &RustSemanticDigest| {
        let activation_digest = activation_context_digest(&fixture.input().activation)?;
        let bindings = [ProviderRequirementBinding {
            activation_digest: &activation_digest,
            definition_digest: &definition_digest,
            requirement_index: 0,
            selected: ProviderRequirementSelection::Rust(source),
        }];
        fixture.project_with(&mut SemanticProjectionBuilder::new(), &bindings, &[], &[])
    };
    let baseline = project(&first)?;
    assert_ne!(baseline.value(), project(&changed_source)?.value());
    assert_ne!(baseline.value(), project(&changed_features)?.value());
    Ok(())
}

/// Absent, duplicate, stale-definition and extra request bindings cannot produce a digest.
#[test]
fn missing_and_mismatched_bindings_refuse() -> TestResult {
    let mut fixture = Fixture::new("provider");
    fixture.definition.requirements.push(registry_request());
    assert!(matches!(
        SemanticProjectionBuilder::new().register(&fixture.input()),
        Err(SemanticProjectionError::Missing { .. })
    ));
    let selected = rust_leaf('2', BTreeSet::new())?;
    let activation_digest = activation_context_digest(&fixture.input().activation)?;
    let bindings = [ProviderRequirementBinding {
        activation_digest: &activation_digest,
        definition_digest: &digest('9'),
        requirement_index: 0,
        selected: ProviderRequirementSelection::Rust(&selected),
    }];
    let mut input = fixture.input();
    input.requirements = &bindings;
    assert!(matches!(
        SemanticProjectionBuilder::new().register(&input),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    let definition_digest = definition_binding_digest(&fixture.definition)?;
    for indexes in [[0, 0], [0, 1]] {
        let rows = indexes.map(|requirement_index| ProviderRequirementBinding {
            activation_digest: &activation_digest,
            definition_digest: &definition_digest,
            requirement_index,
            selected: ProviderRequirementSelection::Rust(&selected),
        });
        assert!(matches!(
            fixture.project_with(&mut SemanticProjectionBuilder::new(), &rows, &[], &[]),
            Err(SemanticProjectionError::Invalid { .. })
        ));
    }
    input.definition = None;
    assert!(matches!(
        SemanticProjectionBuilder::new().register(&input),
        Err(SemanticProjectionError::Missing { .. })
    ));
    Ok(())
}

/// Registration retains full selected features and refuses changed inputs within one original input context.
#[test]
fn full_feature_identity_and_fixed_context_conflicts() -> TestResult {
    let first = Fixture::new("provider");
    let mut second = Fixture::new("provider");
    second.identity.feature_projection.insert("feature".to_string());
    let mut context = SemanticProjectionBuilder::new();
    let a = first.project(&mut context)?;
    let b = second.project(&mut context)?;
    assert_ne!(a.value(), b.value());
    assert_eq!(context.results.len(), 2);
    second.definition.authored_source_digest = digest('3');
    second.manifest.contract_metadata.provider.semantic_source_digest = Some(digest('3'));
    assert!(matches!(
        second.project(&mut context),
        Err(SemanticProjectionError::Conflict { .. })
    ));
    Ok(())
}

/// Finalized consumers borrow the registered result without needing the original body or a new input record.
#[test]
fn finalized_context_reuses_results_and_refuses_unregistered_identities() -> TestResult {
    let fixture = Fixture::new("provider");
    let identity = fixture.identity.clone();
    let mut builder = SemanticProjectionBuilder::new();
    let registered = fixture.project(&mut builder)?;
    let context = builder.finish();
    drop(fixture);
    let first = context.digest(&identity)?;
    let repeated = context.digest(&identity)?;
    assert!(std::ptr::eq(first, repeated));
    assert_eq!(first.value(), registered.value());
    let mut unregistered = identity;
    unregistered.feature_projection.insert("new-feature".to_string());
    assert!(matches!(
        context.digest(&unregistered),
        Err(SemanticProjectionError::Missing { .. })
    ));
    Ok(())
}

/// Construct a selected source record from compiler support's own producer contract.
fn support_leaf(digit: char) -> Result<RustSemanticDigest, SemanticProjectionError> {
    let files = BTreeMap::from([("src/lib.rs".to_string(), digest(digit))]);
    digest_rust_source_inputs(&RustSemanticInputs {
        activation: activation(&BTreeSet::new(), false),
        contract: RUST_SEMANTIC_INPUT_CONTRACT,
        contract_version: 1,
        package: "incan_stdlib",
        version: "0.6.0",
        crate_name: "incan_stdlib",
        crate_kind: NativeSourceCrateKind::Rlib,
        edition: "2021",
        entrypoint: "src/lib.rs",
        source: RustSemanticSource::CompilerSupport {
            support: NativeCompilerSupport::Stdlib,
        },
        files: &files,
        configuration: &BTreeMap::new(),
        features: &BTreeSet::new(),
        default_features: false,
        dependencies: &[],
        selections: &[],
    })
}

/// Add an original checked provider edge and its source requirement using one consistent alias.
fn add_edge(parent: &mut Fixture, target: &ProviderIdentity, relative_path: &str) {
    use crate::library_manifest::{ProviderDependencyKind, ProviderDependencyMetadata};
    parent
        .manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .push(ProviderDependencyMetadata {
            kind: ProviderDependencyKind::PublicPackage,
            dependency_key: "dependency".to_string(),
            provider_name: target.name.clone(),
            provider_version: target.version.clone(),
            artifact_digest: target.digest.clone(),
            relative_artifact_path: relative_path.to_string(),
            requested_features: BTreeSet::new(),
            default_features: false,
            optional: false,
        });
    parent.definition.requirements.push(NativeSourceRequirement {
        role: NativeRequirementRole::Normal,
        alias: "dependency".to_string(),
        package: Some(target.name.clone()),
        version_requirement: Some(target.version.clone()),
        features: Vec::new(),
        default_features: false,
        optional: false,
        source: NativeRequirementSource::ProviderEdge {
            edge_kind: ProviderDependencyKind::PublicPackage,
            dependency_key: "dependency".to_string(),
        },
    });
}

/// Project one actual descriptor/definition pair against the supplied original selected identity.
fn project_edge(
    parent: &Fixture,
    selected: &ProviderIdentity,
    target: &ProviderSemanticDigest,
) -> Result<ProviderSemanticDigest, SemanticProjectionError> {
    let definition_digest = definition_binding_digest(&parent.definition)?;
    let edges = [ProviderSemanticEdge {
        descriptor_index: 0,
        selected_identity: selected,
        target,
    }];
    let activation_digest = activation_context_digest(&parent.input().activation)?;
    let requirements = [ProviderRequirementBinding {
        activation_digest: &activation_digest,
        definition_digest: &definition_digest,
        requirement_index: 0,
        selected: ProviderRequirementSelection::ProviderEdge { descriptor_index: 0 },
    }];
    parent.project_with(&mut SemanticProjectionBuilder::new(), &requirements, &edges, &[])
}

/// Public delivery coordinates disappear only after exact child association; changed child semantics remain visible.
#[test]
fn provider_edges_relocate_but_selected_dependencies_change_digest() -> TestResult {
    let child = Fixture::new("child");
    let mut relocated_child = Fixture::new("child");
    relocated_child.identity.digest = digest('e');
    let child_digest = child.project(&mut SemanticProjectionBuilder::new())?;
    let relocated_digest = relocated_child.project(&mut SemanticProjectionBuilder::new())?;
    let mut first = Fixture::new("parent");
    let mut second = Fixture::new("parent");
    add_edge(&mut first, &child.identity, "../first/child");
    add_edge(&mut second, &relocated_child.identity, "../../relocated/child");
    let baseline = project_edge(&first, &child.identity, &child_digest)?;
    assert_eq!(
        baseline.value(),
        project_edge(&second, &relocated_child.identity, &relocated_digest)?.value()
    );
    relocated_child.definition.authored_source_digest = digest('6');
    relocated_child
        .manifest
        .contract_metadata
        .provider
        .semantic_source_digest = Some(digest('6'));
    relocated_child.identity.digest = digest('9');
    second.manifest.contract_metadata.provider.provider_dependencies[0].artifact_digest = digest('9');
    let changed = relocated_child.project(&mut SemanticProjectionBuilder::new())?;
    assert_ne!(
        baseline.value(),
        project_edge(&second, &relocated_child.identity, &changed)?.value()
    );
    assert!(matches!(
        first.project(&mut SemanticProjectionBuilder::new()),
        Err(SemanticProjectionError::Missing { .. })
    ));
    Ok(())
}

/// Same physical digest with different active features cannot substitute for an original selected target handle.
#[test]
fn edge_requires_full_original_feature_identity_and_same_version() -> TestResult {
    let child = Fixture::new("child");
    let mut different = Fixture::new("child");
    different.identity.feature_projection.insert("extra".to_string());
    let different_digest = different.project(&mut SemanticProjectionBuilder::new())?;
    let mut parent = Fixture::new("parent");
    add_edge(&mut parent, &child.identity, "../child");
    assert!(matches!(
        project_edge(&parent, &child.identity, &different_digest),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    let mut incompatible = child.project(&mut SemanticProjectionBuilder::new())?;
    incompatible.projection_version = 2;
    assert!(matches!(
        project_edge(&parent, &child.identity, &incompatible),
        Err(SemanticProjectionError::Unsupported { .. })
    ));
    Ok(())
}

/// Standalone and nested native-union owners use the exact typed origin mapping, not physical string rewriting.
#[test]
fn native_union_origins_relocate_and_missing_origins_refuse() -> TestResult {
    use crate::library_manifest::NativeUnionExport;
    let child = Fixture::new("child");
    let mut relocated = Fixture::new("child");
    relocated.identity.digest = digest('e');
    let selected = child.project(&mut SemanticProjectionBuilder::new())?;
    let relocated_selected = relocated.project(&mut SemanticProjectionBuilder::new())?;
    let union = |identity: &ProviderIdentity| NativeUnionExport {
        owner: NativeUnionOwnerExport::SelectedArtifact(identity.clone()),
        rust_name: "ForeignUnion".to_string(),
        members: vec![TypeRef::Named {
            name: "int".to_string(),
            origin: None,
        }],
        local_nominals: BTreeMap::new(),
        checked_projection: None,
    };
    let mut first = Fixture::new("parent");
    first
        .manifest
        .contract_metadata
        .native_unions
        .push(union(&child.identity));
    let mut second = Fixture::new("parent");
    second
        .manifest
        .contract_metadata
        .native_unions
        .push(union(&relocated.identity));
    assert!(matches!(
        first.project(&mut SemanticProjectionBuilder::new()),
        Err(SemanticProjectionError::Missing { .. })
    ));
    let a = first.project_with(&mut SemanticProjectionBuilder::new(), &[], &[], &[&selected])?;
    let b = second.project_with(&mut SemanticProjectionBuilder::new(), &[], &[], &[&relocated_selected])?;
    assert_eq!(a.value(), b.value());
    first.manifest.contract_metadata.native_unions[0]
        .members
        .push(TypeRef::NativeUnion(union(&relocated.identity)));
    assert!(matches!(
        first.project_with(&mut SemanticProjectionBuilder::new(), &[], &[], &[&selected]),
        Err(SemanticProjectionError::Missing { .. })
    ));
    Ok(())
}

/// Selected support changes require a fresh context; absence, extra records and legacy absence are explicit errors.
#[test]
fn compiler_support_context_and_coverage_are_explicit() -> TestResult {
    let mut fixture = Fixture::new("provider");
    assert!(matches!(
        SemanticProjectionBuilder::new().register(&fixture.input()),
        Err(SemanticProjectionError::Missing { .. })
    ));
    let baseline = fixture.project(&mut SemanticProjectionBuilder::new())?;
    let definition_digest = definition_binding_digest(&fixture.definition)?;
    let source = support_leaf('7')?;
    let support = [ProviderSupportBinding {
        definition_digest: &definition_digest,
        support_index: 0,
        selected: &source,
    }];
    let mut input = fixture.input();
    input.compiler_support = &support;
    assert_ne!(
        baseline.value(),
        SemanticProjectionBuilder::new().register(&input)?.value()
    );
    let mut context = SemanticProjectionBuilder::new();
    fixture.project(&mut context)?;
    assert!(matches!(
        context.register(&input),
        Err(SemanticProjectionError::Conflict { .. })
    ));
    let duplicate = [
        ProviderSupportBinding {
            definition_digest: &definition_digest,
            support_index: 0,
            selected: &source,
        },
        ProviderSupportBinding {
            definition_digest: &definition_digest,
            support_index: 0,
            selected: &source,
        },
    ];
    input.compiler_support = &duplicate;
    assert!(matches!(
        SemanticProjectionBuilder::new().register(&input),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    fixture.definition.schema_version = 3;
    fixture.definition.source_members = Some(vec![fixture.definition.entrypoint.clone()]);
    assert_eq!(
        baseline.value(),
        fixture.project(&mut SemanticProjectionBuilder::new())?.value(),
        "physical code-member evidence must not create a second semantic identity path"
    );
    fixture.definition.schema_version = 1;
    fixture.definition.compiler_support = None;
    fixture.definition.source_members = None;
    assert!(matches!(
        SemanticProjectionBuilder::new().register(&fixture.input()),
        Err(SemanticProjectionError::Missing { .. })
    ));
    Ok(())
}

/// An original source unit's declared dependency slot must bind its exact definition and selected source result.
#[test]
fn rust_source_dependencies_require_complete_original_slot_bindings() -> TestResult {
    let files = BTreeMap::from([("src/lib.rs".to_string(), digest('1'))]);
    let config = BTreeMap::from([("build-script".to_string(), "absent".to_string())]);
    let request = registry_request();
    let dependencies = [RustSemanticDependency {
        request: &request,
        build: false,
        target_condition: Some("cfg(unix)"),
    }];
    let features = BTreeSet::new();
    let mut input = RustSemanticInputs {
        activation: activation(&features, true),
        contract: RUST_SEMANTIC_INPUT_CONTRACT,
        contract_version: 1,
        package: "parent",
        version: "1.0.0",
        crate_name: "parent",
        crate_kind: NativeSourceCrateKind::Rlib,
        edition: "2021",
        entrypoint: "src/lib.rs",
        source: RustSemanticSource::Path,
        files: &files,
        configuration: &config,
        features: &features,
        default_features: true,
        dependencies: &dependencies,
        selections: &[],
    };
    assert!(matches!(
        digest_rust_source_inputs(&input),
        Err(SemanticProjectionError::Missing { .. })
    ));
    let binding_digest = rust_definition_binding_digest(&input)?;
    let activation_digest = activation_context_digest(&input.activation)?;
    let first = rust_leaf('2', BTreeSet::new())?;
    let changed = rust_leaf('3', BTreeSet::new())?;
    let selections = [RustSemanticSelection {
        activation_digest: &activation_digest,
        definition_digest: &binding_digest,
        requirement_index: 0,
        outcome: RustSemanticOutcome::Selected(&first),
    }];
    input.selections = &selections;
    let baseline = digest_rust_source_inputs(&input)?;
    let new_selections = [RustSemanticSelection {
        activation_digest: &activation_digest,
        definition_digest: &binding_digest,
        requirement_index: 0,
        outcome: RustSemanticOutcome::Selected(&changed),
    }];
    input.selections = &new_selections;
    assert_ne!(baseline.value, digest_rust_source_inputs(&input)?.value);
    let wrong = [RustSemanticSelection {
        activation_digest: &activation_digest,
        definition_digest: &binding_digest,
        requirement_index: 1,
        outcome: RustSemanticOutcome::Selected(&first),
    }];
    input.selections = &wrong;
    assert!(matches!(
        digest_rust_source_inputs(&input),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    Ok(())
}

/// Unknown producer contracts and malformed source catalogs cannot masquerade as existing selected source evidence.
#[test]
fn rust_source_contract_versions_and_file_evidence_refuse() -> TestResult {
    let features = BTreeSet::new();
    let files = BTreeMap::from([("src/lib.rs".to_string(), digest('1'))]);
    let mut input = RustSemanticInputs {
        activation: activation(&features, false),
        contract: "native-receipt",
        contract_version: 1,
        package: "package",
        version: "1.0.0",
        crate_name: "package",
        crate_kind: NativeSourceCrateKind::Rlib,
        edition: "2021",
        entrypoint: "src/lib.rs",
        source: RustSemanticSource::Path,
        files: &files,
        configuration: &BTreeMap::new(),
        features: &features,
        default_features: false,
        dependencies: &[],
        selections: &[],
    };
    assert!(matches!(
        digest_rust_source_inputs(&input),
        Err(SemanticProjectionError::Unsupported { .. })
    ));
    input.contract = RUST_SEMANTIC_INPUT_CONTRACT;
    input.contract_version = 9;
    assert!(matches!(
        digest_rust_source_inputs(&input),
        Err(SemanticProjectionError::Unsupported { .. })
    ));
    input.contract_version = 1;
    input.entrypoint = "missing.rs";
    assert!(matches!(
        digest_rust_source_inputs(&input),
        Err(SemanticProjectionError::Missing { .. })
    ));
    let escaped = BTreeMap::from([("../lib.rs".to_string(), digest('1'))]);
    input.files = &escaped;
    input.entrypoint = "../lib.rs";
    assert!(matches!(
        digest_rust_source_inputs(&input),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    Ok(())
}

/// Every Rust declaration has a domain-bound outcome, while inactive slots require no invented child identity.
#[test]
fn inactive_rust_slots_preserve_declarations_and_bind_effective_domain() -> TestResult {
    let required = registry_request();
    let mut optional = required.clone();
    optional.alias = "optional".to_string();
    optional.optional = true;
    let mut target_only = required.clone();
    target_only.alias = "windows".to_string();
    let mut development = required.clone();
    development.alias = "development".to_string();
    development.role = NativeRequirementRole::Dev;
    let dependencies = [
        RustSemanticDependency {
            request: &required,
            build: false,
            target_condition: None,
        },
        RustSemanticDependency {
            request: &optional,
            build: false,
            target_condition: None,
        },
        RustSemanticDependency {
            request: &target_only,
            build: false,
            target_condition: Some("cfg(windows)"),
        },
        RustSemanticDependency {
            request: &development,
            build: false,
            target_condition: None,
        },
    ];
    let files = BTreeMap::from([("src/lib.rs".to_string(), digest('2'))]);
    let features = BTreeSet::new();
    let mut input = RustSemanticInputs {
        activation: activation(&features, false),
        contract: RUST_SEMANTIC_INPUT_CONTRACT,
        contract_version: 1,
        package: "parent",
        version: "1.0.0",
        crate_name: "parent",
        crate_kind: NativeSourceCrateKind::Rlib,
        edition: "2021",
        entrypoint: "src/lib.rs",
        source: RustSemanticSource::Path,
        files: &files,
        configuration: &BTreeMap::new(),
        features: &features,
        default_features: false,
        dependencies: &dependencies,
        selections: &[],
    };
    let definition_digest = rust_definition_binding_digest(&input)?;
    let domain = activation_context_digest(&input.activation)?;
    let selected = rust_leaf('5', BTreeSet::new())?;
    let row = |requirement_index, outcome| RustSemanticSelection {
        definition_digest: &definition_digest,
        activation_digest: &domain,
        requirement_index,
        outcome,
    };
    let selections = [
        row(0, RustSemanticOutcome::Selected(&selected)),
        row(
            1,
            RustSemanticOutcome::Inactive(SemanticInactiveReason::OptionalNotEnabled),
        ),
        row(
            2,
            RustSemanticOutcome::Inactive(SemanticInactiveReason::TargetConditionFalse),
        ),
        row(
            3,
            RustSemanticOutcome::Inactive(SemanticInactiveReason::DevelopmentExcluded),
        ),
    ];
    input.selections = &selections;
    let baseline = digest_rust_source_inputs(&input)?;
    assert!(!baseline.value.is_empty());
    input.selections = &selections[..3];
    assert!(matches!(
        digest_rust_source_inputs(&input),
        Err(SemanticProjectionError::Missing { .. })
    ));
    input.selections = &selections;
    input.activation.target = "x86_64-pc-windows-msvc";
    assert!(matches!(
        digest_rust_source_inputs(&input),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    input.activation.target = "aarch64-apple-darwin";
    let wrong = [
        row(
            0,
            RustSemanticOutcome::Inactive(SemanticInactiveReason::OptionalNotEnabled),
        ),
        row(
            1,
            RustSemanticOutcome::Inactive(SemanticInactiveReason::OptionalNotEnabled),
        ),
        row(
            2,
            RustSemanticOutcome::Inactive(SemanticInactiveReason::TargetConditionFalse),
        ),
        row(
            3,
            RustSemanticOutcome::Inactive(SemanticInactiveReason::DevelopmentExcluded),
        ),
    ];
    input.selections = &wrong;
    assert!(matches!(
        digest_rust_source_inputs(&input),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    Ok(())
}

/// A normal provider retains an excluded dev declaration; a test domain requires its own checked outcome.
#[test]
fn provider_dev_outcomes_cannot_be_omitted_or_reused_in_a_test_domain() -> TestResult {
    let mut fixture = Fixture::new("provider");
    let mut request = registry_request();
    request.role = NativeRequirementRole::Dev;
    fixture.definition.requirements.push(request);
    let binding = definition_binding_digest(&fixture.definition)?;
    let domain = activation_context_digest(&fixture.input().activation)?;
    let requirements = [ProviderRequirementBinding {
        definition_digest: &binding,
        activation_digest: &domain,
        requirement_index: 0,
        selected: ProviderRequirementSelection::Inactive(SemanticInactiveReason::DevelopmentExcluded),
    }];
    let normal = fixture.project_with(&mut SemanticProjectionBuilder::new(), &requirements, &[], &[])?;
    assert!(matches!(
        fixture.project(&mut SemanticProjectionBuilder::new()),
        Err(SemanticProjectionError::Missing { .. })
    ));
    let source = support_leaf('4')?;
    let support = [ProviderSupportBinding {
        definition_digest: &binding,
        support_index: 0,
        selected: &source,
    }];
    let mut input = fixture.input();
    input.compiler_support = &support;
    input.requirements = &requirements;
    input.activation.purpose = SemanticActivationPurpose::Test;
    assert!(matches!(
        SemanticProjectionBuilder::new().register(&input),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    let test_domain = activation_context_digest(&input.activation)?;
    let still_excluded = [ProviderRequirementBinding {
        definition_digest: &binding,
        activation_digest: &test_domain,
        requirement_index: 0,
        selected: ProviderRequirementSelection::Inactive(SemanticInactiveReason::DevelopmentExcluded),
    }];
    input.requirements = &still_excluded;
    assert!(matches!(
        SemanticProjectionBuilder::new().register(&input),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    let dependency = rust_leaf('6', BTreeSet::new())?;
    let selected = [ProviderRequirementBinding {
        definition_digest: &binding,
        activation_digest: &test_domain,
        requirement_index: 0,
        selected: ProviderRequirementSelection::Rust(&dependency),
    }];
    input.requirements = &selected;
    let test = SemanticProjectionBuilder::new().register(&input)?;
    assert_ne!(normal.value(), test.value());
    Ok(())
}

/// Public requests permit unified extra features; private SDK edges retain exact selected features and fixed flags.
#[test]
fn provider_edge_features_follow_public_subset_and_private_exact_contracts() -> TestResult {
    let mut child = Fixture::new("child");
    child.identity.feature_projection = BTreeSet::from(["needed".to_string(), "extra".to_string()]);
    let selected = child.project(&mut SemanticProjectionBuilder::new())?;
    let mut parent = Fixture::new("parent");
    add_edge(&mut parent, &child.identity, "../child");
    parent.manifest.contract_metadata.provider.provider_dependencies[0]
        .requested_features
        .insert("needed".to_string());
    project_edge(&parent, &child.identity, &selected)?;
    parent.manifest.contract_metadata.provider.provider_dependencies[0]
        .requested_features
        .insert("missing".to_string());
    assert!(matches!(
        project_edge(&parent, &child.identity, &selected),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    parent.manifest.contract_metadata.provider.provider_dependencies[0]
        .requested_features
        .remove("missing");
    parent.manifest.contract_metadata.provider.provider_dependencies[0].kind =
        ProviderDependencyKind::PrivateImplementation;
    parent.definition.requirements[0].source = NativeRequirementSource::ProviderEdge {
        edge_kind: ProviderDependencyKind::PrivateImplementation,
        dependency_key: "dependency".to_string(),
    };
    assert!(matches!(
        project_edge(&parent, &child.identity, &selected),
        Err(SemanticProjectionError::Invalid { .. })
    ));
    parent.manifest.contract_metadata.provider.provider_dependencies[0]
        .requested_features
        .clone_from(&child.identity.feature_projection);
    project_edge(&parent, &child.identity, &selected)?;
    for defaults in [true, false] {
        let descriptor = &mut parent.manifest.contract_metadata.provider.provider_dependencies[0];
        descriptor.default_features = defaults;
        descriptor.optional = !defaults;
        assert!(matches!(
            project_edge(&parent, &child.identity, &selected),
            Err(SemanticProjectionError::Invalid { .. })
        ));
    }
    Ok(())
}

/// An absent checked build unit discharges only build slots and cannot survive a changed activation domain.
#[test]
fn absent_build_unit_preserves_declaration_and_refuses_nonbuild_or_present_units() -> TestResult {
    let request = registry_request();
    let files = BTreeMap::from([("src/lib.rs".to_string(), digest('2'))]);
    let features = BTreeSet::new();
    let configuration = BTreeMap::new();
    for (build_dependency, build_unit_present, accepted) in
        [(true, false, true), (false, false, false), (true, true, false)]
    {
        let dependencies = [RustSemanticDependency {
            request: &request,
            build: build_dependency,
            target_condition: None,
        }];
        let mut context = activation(&features, false);
        context.build_unit_present = build_unit_present;
        let mut input = RustSemanticInputs {
            activation: context,
            contract: RUST_SEMANTIC_INPUT_CONTRACT,
            contract_version: 1,
            package: "parent",
            version: "1.0.0",
            crate_name: "parent",
            crate_kind: NativeSourceCrateKind::Rlib,
            edition: "2021",
            entrypoint: "src/lib.rs",
            source: RustSemanticSource::Path,
            files: &files,
            configuration: &configuration,
            features: &features,
            default_features: false,
            dependencies: &dependencies,
            selections: &[],
        };
        let definition = rust_definition_binding_digest(&input)?;
        let domain = activation_context_digest(&context)?;
        let selections = [RustSemanticSelection {
            definition_digest: &definition,
            activation_digest: &domain,
            requirement_index: 0,
            outcome: RustSemanticOutcome::Inactive(SemanticInactiveReason::BuildUnitAbsent),
        }];
        input.selections = &selections;
        let result = digest_rust_source_inputs(&input);
        if accepted {
            result?;
            input.selections = &[];
            assert!(matches!(
                digest_rust_source_inputs(&input),
                Err(SemanticProjectionError::Missing { .. })
            ));
            input.selections = &selections;
            input.activation.build_unit_present = true;
            assert_ne!(activation_context_digest(&input.activation)?, domain);
            assert!(matches!(
                digest_rust_source_inputs(&input),
                Err(SemanticProjectionError::Invalid { .. })
            ));
        } else {
            assert!(matches!(result, Err(SemanticProjectionError::Invalid { .. })));
        }
    }
    Ok(())
}
