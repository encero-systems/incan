//! The RFC 120 identity-conformance corpus: the provider, facade and matrix sources, the lexical, member and
//! path matrix rows with their verifiers and coverage validation, the `let`/`mut` shadowing, generic-binder and
//! builtin-rebinding rows, and the release-artifact projection check.

use super::*;

const IDENTITY_PROVIDER_SRC: &str = r#"
pub def imported_lexical(value: int) -> int:
    return value

pub def aliased_lexical(value: int) -> int:
    return value

pub def relayed_lexical(value: int) -> int:
    return value

pub model ImportedMember:
    pub value: int

    def read(self) -> int:
        return self.value

pub model AliasedMember:
    pub value: int

    def read(self) -> int:
        return self.value

pub model RelayedMember:
    pub value: int

    def read(self) -> int:
        return self.value

pub def imported_path(value: int) -> int:
    return value

pub def aliased_path(value: int) -> int:
    return value

"#;

const IDENTITY_FACADE_SRC: &str = r#"
pub from identity_provider import RelayedMember, relayed_lexical
"#;

pub(super) const IDENTITY_MATRIX_SRC: &str = r#"
from identity_provider import ImportedMember, imported_lexical
from identity_provider import AliasedMember as MemberAlias, aliased_lexical as lexical_alias
from identity_facade import RelayedMember as MemberReexport, relayed_lexical as lexical_reexport
import identity_provider
import identity_provider as provider_alias

def local_lexical(value: int) -> int:
    return value

model LocalMember:
    value: int

    def read(self) -> int:
        return self.value

def local_lexical_function_scope() -> int:
    return local_lexical(1)

def local_lexical_block_scope() -> int:
    if true:
        return local_lexical(1)
    return 0

def imported_lexical_function_scope() -> int:
    return imported_lexical(3)

def imported_lexical_block_scope() -> int:
    if true:
        return imported_lexical(3)
    return 0

def aliased_lexical_function_scope() -> int:
    return lexical_alias(5)

def aliased_lexical_block_scope() -> int:
    if true:
        return lexical_alias(5)
    return 0

def reexported_lexical_function_scope() -> int:
    return lexical_reexport(7)

def reexported_lexical_block_scope() -> int:
    if true:
        return lexical_reexport(7)
    return 0

def local_member_function_scope() -> int:
    local = LocalMember(value=9)
    return local.read()

def local_member_block_scope() -> int:
    local = LocalMember(value=10)
    if true:
        return local.read()
    return 0

def imported_member_function_scope() -> int:
    imported = ImportedMember(value=11)
    return imported.read()

def imported_member_block_scope() -> int:
    imported = ImportedMember(value=12)
    if true:
        return imported.read()
    return 0

def aliased_member_function_scope() -> int:
    aliased = MemberAlias(value=13)
    return aliased.read()

def aliased_member_block_scope() -> int:
    aliased = MemberAlias(value=14)
    if true:
        return aliased.read()
    return 0

def reexported_member_function_scope() -> int:
    relayed = MemberReexport(value=15)
    return relayed.read()

def reexported_member_block_scope() -> int:
    relayed = MemberReexport(value=16)
    if true:
        return relayed.read()
    return 0

def imported_path_function_scope() -> int:
    return identity_provider.imported_path(17)

def imported_path_block_scope() -> int:
    if true:
        return identity_provider.imported_path(17)
    return 0

def aliased_path_function_scope() -> int:
    return provider_alias.aliased_path(19)

def aliased_path_block_scope() -> int:
    if true:
        return provider_alias.aliased_path(19)
    return 0

"#;

/// Every matrix cell the replacement route executes across the module boundary, with the value it must return.
///
/// The returned values are deliberately distinct so a call that reached the wrong declaration is a wrong number
/// rather than a coincidence: were `lexical_alias` resolved to `imported_lexical`, this row would return 3 where 5
/// is required, and both are real declarations that execute.
///
/// Member cells are absent on purpose and are declared in [`IDENTITY_MATRIX_DEFERRED`] instead.
pub(super) const IDENTITY_MATRIX_ENTRYPOINTS: &[IdentityGraphEntrypoint] = &[
    IdentityGraphEntrypoint {
        function: "local_lexical_function_scope",
        expected: 1,
    },
    IdentityGraphEntrypoint {
        function: "local_lexical_block_scope",
        expected: 1,
    },
    IdentityGraphEntrypoint {
        function: "imported_lexical_function_scope",
        expected: 3,
    },
    IdentityGraphEntrypoint {
        function: "imported_lexical_block_scope",
        expected: 3,
    },
    IdentityGraphEntrypoint {
        function: "aliased_lexical_function_scope",
        expected: 5,
    },
    IdentityGraphEntrypoint {
        function: "aliased_lexical_block_scope",
        expected: 5,
    },
    IdentityGraphEntrypoint {
        function: "reexported_lexical_function_scope",
        expected: 7,
    },
    IdentityGraphEntrypoint {
        function: "reexported_lexical_block_scope",
        expected: 7,
    },
    IdentityGraphEntrypoint {
        function: "imported_path_function_scope",
        expected: 17,
    },
    IdentityGraphEntrypoint {
        function: "imported_path_block_scope",
        expected: 17,
    },
    IdentityGraphEntrypoint {
        function: "aliased_path_function_scope",
        expected: 19,
    },
    IdentityGraphEntrypoint {
        function: "aliased_path_block_scope",
        expected: 19,
    },
];

/// Matrix cells the replacement route refuses today, each bound to the issue that owns closing the gap.
///
/// Every member cell in the matrix reads a field through a model method, and the direct profile retains a model
/// declaration only when it has no methods (`is_direct_replacement_plain_model`), so the constructor is refused
/// before the method is ever reached. That is a language-matrix gap owned by #1291, not an import one: the same
/// refusal occurs for `LocalMember`, which crosses no module boundary at all. #1260 and #1261 supply what these
/// cells still need after that lands -- the imported, aliased, and re-exported identities they construct through.
///
/// The runner requires each of these to actually be refused. A deferral that quietly starts working fails this row
/// rather than passing unnoticed, so the day #1291 lands, someone has to come back and promote these cells.
pub(super) const IDENTITY_MATRIX_DEFERRED: &[IdentityGraphDeferral] = &[
    IdentityGraphDeferral {
        function: "local_member_function_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "local_member_block_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "imported_member_function_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "imported_member_block_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "aliased_member_function_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "aliased_member_block_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "reexported_member_function_scope",
        owning_issue: 1291,
    },
    IdentityGraphDeferral {
        function: "reexported_member_block_scope",
        owning_issue: 1291,
    },
];

pub(super) const IDENTITY_MATRIX_MODULES: &[IdentitySourceModule] = &[
    IdentitySourceModule {
        name: "identity_provider",
        path: &["identity_provider"],
        source: IDENTITY_PROVIDER_SRC,
        dependencies: &[],
    },
    IdentitySourceModule {
        name: "identity_facade",
        path: &["identity_facade"],
        source: IDENTITY_FACADE_SRC,
        dependencies: &["identity_provider"],
    },
    IdentitySourceModule {
        name: "identity_matrix",
        path: &["identity_matrix"],
        source: IDENTITY_MATRIX_SRC,
        dependencies: &["identity_provider", "identity_facade"],
    },
];

struct LexicalIdentityRow<'a> {
    label: &'a str,
    binding: IdentityBindingForm,
    target_module: &'a str,
    target_name: &'a str,
    root_binding: &'a str,
    call: &'a str,
    function_body: &'a str,
    block_body: &'a str,
}

#[derive(Clone, Copy)]
struct DeclarationSpanSelector<'a> {
    anchor: &'a str,
    occurrence: usize,
}

struct MemberIdentityRow<'a> {
    label: &'a str,
    binding: IdentityBindingForm,
    target_module: &'a str,
    owner_name: &'a str,
    owner: DeclarationSpanSelector<'a>,
    member: DeclarationSpanSelector<'a>,
    root_binding: &'a str,
    receiver_call: &'a str,
    function_body: &'a str,
    block_body: &'a str,
}

struct PathIdentityRow<'a> {
    label: &'a str,
    binding: IdentityBindingForm,
    expected_module_path: &'a [&'a str],
    expected_module_name: &'a str,
    target_name: &'a str,
    module_binding: &'a str,
    call: &'a str,
    function_body: &'a str,
    block_body: &'a str,
}

fn require_same_identity(
    label: &str,
    expected: &CanonicalSymbolId,
    actual: &CanonicalSymbolId,
    evidence: &mut Vec<String>,
) -> Result<(), String> {
    if actual != expected {
        return Err(format!(
            "{label} reconstructed or selected the wrong identity: expected {}, got {}",
            expected.render_compact(),
            actual.render_compact()
        ));
    }
    evidence.push(format!("{label}: {}", actual.render_compact()));
    Ok(())
}

fn require_body_consumer(
    graph: &CheckedIdentityGraph,
    body: &str,
    expected: &CanonicalSymbolId,
    evidence: &mut Vec<String>,
    label: &str,
) -> Result<(), String> {
    let identities = graph.body_consumer_identities("identity_matrix", body)?;
    if !identities.iter().any(|identity| identity == expected) {
        return Err(format!(
            "{label} Body IR lost {}, retaining {identities:?}",
            expected.render_compact()
        ));
    }
    evidence.push(format!("{label}: {}", expected.render_compact()));
    Ok(())
}

fn verify_lexical_matrix_row(
    graph: &CheckedIdentityGraph,
    row: &LexicalIdentityRow<'_>,
    assertions: &mut IdentityAssertions,
) -> Result<(), String> {
    let target = graph.declaration_identity(
        row.target_module,
        row.target_name,
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let module_binding = graph.hir_identity("identity_matrix", row.root_binding)?;
    require_same_identity(
        &format!("{} lexical/module", row.label),
        &target,
        &module_binding,
        &mut assertions.checked_relations,
    )?;
    assertions
        .hir_consumers
        .push(format!("{} lexical/module HIR: {}", row.label, target.render_compact()));

    let function_reference = graph.resolved_identity("identity_matrix", row.call, 0)?;
    require_same_identity(
        &format!("{} lexical/function", row.label),
        &target,
        &function_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.function_body,
        &target,
        &mut assertions.body_ir_consumers,
        &format!("{} lexical/function", row.label),
    )?;

    let block_reference = graph.resolved_identity("identity_matrix", row.call, 1)?;
    require_same_identity(
        &format!("{} lexical/block", row.label),
        &target,
        &block_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.block_body,
        &target,
        &mut assertions.body_ir_consumers,
        &format!("{} lexical/block", row.label),
    )?;
    let target_projection = graph.require_emitted_projection(row.target_module, &target)?;
    let root_projection = graph.require_emitted_projection("identity_matrix", &target)?;
    let projection_evidence = format!("{target_projection}; {root_projection}");
    assertions.legacy_projections.push(target_projection);
    assertions.legacy_projections.push(root_projection);
    assertions.coverage_cells.extend([
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Lexical,
            scope: IdentityScope::Module,
            checked_identity: target.render_compact(),
            hir_identity: Some(module_binding.render_compact()),
            body_ir_identity: None,
            emitted_projection: Some(projection_evidence.clone()),
        },
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Lexical,
            scope: IdentityScope::Function,
            checked_identity: function_reference.render_compact(),
            hir_identity: None,
            body_ir_identity: Some(target.render_compact()),
            emitted_projection: Some(projection_evidence.clone()),
        },
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Lexical,
            scope: IdentityScope::Block,
            checked_identity: block_reference.render_compact(),
            hir_identity: None,
            body_ir_identity: Some(target.render_compact()),
            emitted_projection: Some(projection_evidence),
        },
    ]);
    Ok(())
}

fn verify_member_matrix_row(
    graph: &CheckedIdentityGraph,
    row: &MemberIdentityRow<'_>,
    assertions: &mut IdentityAssertions,
) -> Result<(), String> {
    let owner = graph.declaration_identity_at_source_anchor(
        row.target_module,
        row.owner.anchor,
        row.owner.occurrence,
        SemanticSourceTargetKind::Model,
        SymbolNamespace::OrdinaryLexical,
    )?;
    if owner.declaration_name != row.owner_name {
        return Err(format!(
            "{} owner span selected `{}` instead of `{}`",
            row.label, owner.declaration_name, row.owner_name
        ));
    }
    let declared_member = graph.declaration_identity_at_source_anchor(
        row.target_module,
        row.member.anchor,
        row.member.occurrence,
        SemanticSourceTargetKind::Method,
        SymbolNamespace::Member,
    )?;
    if declared_member.declaration_name != "read"
        || declared_member.kind != SemanticSourceTargetKind::Method
        || declared_member.namespace != SymbolNamespace::Member
    {
        return Err(format!(
            "{} member declaration selected a non-method identity: {}",
            row.label,
            declared_member.render_compact()
        ));
    }
    if declared_member.declaration_span.start < owner.declaration_span.start
        || declared_member.declaration_span.end > owner.declaration_span.end
    {
        return Err(format!(
            "{} member declaration {} is outside owner {}",
            row.label,
            declared_member.render_compact(),
            owner.render_compact()
        ));
    }
    let module_binding = graph.hir_identity("identity_matrix", row.root_binding)?;
    require_same_identity(
        &format!("{} member/module owner", row.label),
        &owner,
        &module_binding,
        &mut assertions.checked_relations,
    )?;
    assertions.hir_consumers.push(format!(
        "{} member/module HIR owner={} declared-member={}",
        row.label,
        owner.render_compact(),
        declared_member.render_compact()
    ));

    let function_reference = graph.resolved_identity("identity_matrix", row.receiver_call, 0)?;
    require_same_identity(
        &format!("{} member/function", row.label),
        &declared_member,
        &function_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.function_body,
        &declared_member,
        &mut assertions.body_ir_consumers,
        &format!("{} member/function", row.label),
    )?;

    let block_reference = graph.resolved_identity("identity_matrix", row.receiver_call, 1)?;
    require_same_identity(
        &format!("{} member/block", row.label),
        &declared_member,
        &block_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.block_body,
        &declared_member,
        &mut assertions.body_ir_consumers,
        &format!("{} member/block", row.label),
    )?;
    let target_projection = graph.require_emitted_projection(row.target_module, &declared_member)?;
    let root_projection = graph.require_emitted_projection("identity_matrix", &declared_member)?;
    let projection_evidence = format!("{target_projection}; {root_projection}");
    assertions.legacy_projections.push(target_projection);
    assertions.legacy_projections.push(root_projection);
    assertions.coverage_cells.extend([
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Member,
            scope: IdentityScope::Owner,
            checked_identity: declared_member.render_compact(),
            hir_identity: None,
            body_ir_identity: None,
            emitted_projection: Some(projection_evidence.clone()),
        },
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Member,
            scope: IdentityScope::Function,
            checked_identity: function_reference.render_compact(),
            hir_identity: None,
            body_ir_identity: Some(declared_member.render_compact()),
            emitted_projection: Some(projection_evidence.clone()),
        },
        IdentityCoverageCell {
            binding: row.binding,
            namespace: IdentityNamespace::Member,
            scope: IdentityScope::Block,
            checked_identity: block_reference.render_compact(),
            hir_identity: None,
            body_ir_identity: Some(declared_member.render_compact()),
            emitted_projection: Some(projection_evidence),
        },
    ]);
    Ok(())
}

fn verify_path_matrix_row(
    graph: &CheckedIdentityGraph,
    row: &PathIdentityRow<'_>,
    assertions: &mut IdentityAssertions,
) -> Result<(), String> {
    let module_binding = graph.hir_identity("identity_matrix", row.module_binding)?;
    let expected_module = CanonicalSymbolId {
        namespace: SymbolNamespace::ModulePath,
        origin: SymbolOrigin::Module(
            row.expected_module_path
                .iter()
                .map(|segment| (*segment).to_string())
                .collect(),
        ),
        declaration_name: row.expected_module_name.to_string(),
        kind: SemanticSourceTargetKind::Module,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, 0),
    };
    require_same_identity(
        &format!("{} path/module", row.label),
        &expected_module,
        &module_binding,
        &mut assertions.checked_relations,
    )?;
    assertions.hir_consumers.push(format!(
        "{} path/module HIR: {}",
        row.label,
        module_binding.render_compact()
    ));
    assertions.coverage_cells.push(IdentityCoverageCell {
        binding: row.binding,
        namespace: IdentityNamespace::ModulePath,
        scope: IdentityScope::Module,
        checked_identity: module_binding.render_compact(),
        hir_identity: Some(module_binding.render_compact()),
        body_ir_identity: None,
        emitted_projection: None,
    });

    let target = graph.declaration_identity(
        "identity_provider",
        row.target_name,
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let function_reference = graph.resolved_identity("identity_matrix", row.call, 0)?;
    require_same_identity(
        &format!("{} path/function", row.label),
        &target,
        &function_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.function_body,
        &target,
        &mut assertions.body_ir_consumers,
        &format!("{} path/function", row.label),
    )?;

    let block_reference = graph.resolved_identity("identity_matrix", row.call, 1)?;
    require_same_identity(
        &format!("{} path/block", row.label),
        &target,
        &block_reference,
        &mut assertions.checked_relations,
    )?;
    require_body_consumer(
        graph,
        row.block_body,
        &target,
        &mut assertions.body_ir_consumers,
        &format!("{} path/block", row.label),
    )?;
    assertions
        .legacy_projections
        .push(graph.require_emitted_projection("identity_provider", &target)?);
    assertions
        .legacy_projections
        .push(graph.require_emitted_projection("identity_matrix", &target)?);
    Ok(())
}

pub(super) fn verify_wrong_path_target_selection(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    let mut assertions = IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations: Vec::new(),
        hir_consumers: Vec::new(),
        body_ir_consumers: Vec::new(),
        legacy_projections: Vec::new(),
        artifact_observations: Vec::new(),
    };
    verify_path_matrix_row(
        graph,
        &PathIdentityRow {
            label: "wrong-target negative",
            binding: IdentityBindingForm::Import,
            expected_module_path: &["identity_facade"],
            expected_module_name: "identity_facade",
            target_name: "imported_path",
            module_binding: "identity_provider",
            call: "identity_provider.imported_path(17)",
            function_body: "imported_path_function_scope",
            block_body: "imported_path_block_scope",
        },
        &mut assertions,
    )?;
    Err("wrong-target module-path selection was incorrectly accepted".to_string())
}

fn validate_identity_matrix_coverage(cells: &[IdentityCoverageCell]) -> Result<(), String> {
    let mut expected = BTreeSet::new();
    for binding in [
        IdentityBindingForm::Local,
        IdentityBindingForm::Import,
        IdentityBindingForm::Alias,
        IdentityBindingForm::ReExport,
    ] {
        for scope in [IdentityScope::Module, IdentityScope::Function, IdentityScope::Block] {
            expected.insert((binding, IdentityNamespace::Lexical, scope));
        }
        for scope in [IdentityScope::Owner, IdentityScope::Function, IdentityScope::Block] {
            expected.insert((binding, IdentityNamespace::Member, scope));
        }
    }
    for binding in [IdentityBindingForm::Import, IdentityBindingForm::Alias] {
        expected.insert((binding, IdentityNamespace::ModulePath, IdentityScope::Module));
    }
    let actual = cells
        .iter()
        .map(|cell| (cell.binding, cell.namespace, cell.scope))
        .collect::<BTreeSet<_>>();
    if actual != expected || cells.len() != expected.len() {
        return Err(format!(
            "RFC 120 typed coverage differs from the semantically valid contract: missing={:?}, unexpected={:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        ));
    }
    Ok(())
}

pub(super) fn verify_wrong_owner_member_selection(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    let mut assertions = IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations: Vec::new(),
        hir_consumers: Vec::new(),
        body_ir_consumers: Vec::new(),
        legacy_projections: Vec::new(),
        artifact_observations: Vec::new(),
    };
    verify_member_matrix_row(
        graph,
        &MemberIdentityRow {
            label: "wrong-owner negative",
            binding: IdentityBindingForm::Import,
            target_module: "identity_provider",
            owner_name: "ImportedMember",
            owner: DeclarationSpanSelector {
                anchor: "pub model ImportedMember:",
                occurrence: 0,
            },
            // Occurrence 1 is `AliasedMember.read`, deliberately outside the selected owner declaration span.
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 1,
            },
            root_binding: "ImportedMember",
            receiver_call: "imported.read()",
            function_body: "imported_member_function_scope",
            block_body: "imported_member_block_scope",
        },
        &mut assertions,
    )?;
    Err("wrong-owner member selection was incorrectly accepted".to_string())
}

pub(super) fn verify_identity_matrix(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    let mut assertions = IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations: Vec::new(),
        hir_consumers: Vec::new(),
        body_ir_consumers: Vec::new(),
        legacy_projections: Vec::new(),
        artifact_observations: Vec::new(),
    };
    for row in [
        LexicalIdentityRow {
            label: "local",
            binding: IdentityBindingForm::Local,
            target_module: "identity_matrix",
            target_name: "local_lexical",
            root_binding: "local_lexical",
            call: "local_lexical(1)",
            function_body: "local_lexical_function_scope",
            block_body: "local_lexical_block_scope",
        },
        LexicalIdentityRow {
            label: "import",
            binding: IdentityBindingForm::Import,
            target_module: "identity_provider",
            target_name: "imported_lexical",
            root_binding: "imported_lexical",
            call: "imported_lexical(3)",
            function_body: "imported_lexical_function_scope",
            block_body: "imported_lexical_block_scope",
        },
        LexicalIdentityRow {
            label: "alias",
            binding: IdentityBindingForm::Alias,
            target_module: "identity_provider",
            target_name: "aliased_lexical",
            root_binding: "lexical_alias",
            call: "lexical_alias(5)",
            function_body: "aliased_lexical_function_scope",
            block_body: "aliased_lexical_block_scope",
        },
        LexicalIdentityRow {
            label: "re-export",
            binding: IdentityBindingForm::ReExport,
            target_module: "identity_provider",
            target_name: "relayed_lexical",
            root_binding: "lexical_reexport",
            call: "lexical_reexport(7)",
            function_body: "reexported_lexical_function_scope",
            block_body: "reexported_lexical_block_scope",
        },
    ] {
        verify_lexical_matrix_row(graph, &row, &mut assertions)?;
    }
    for row in [
        MemberIdentityRow {
            label: "local",
            binding: IdentityBindingForm::Local,
            target_module: "identity_matrix",
            owner_name: "LocalMember",
            owner: DeclarationSpanSelector {
                anchor: "model LocalMember:",
                occurrence: 0,
            },
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 0,
            },
            root_binding: "LocalMember",
            receiver_call: "local.read()",
            function_body: "local_member_function_scope",
            block_body: "local_member_block_scope",
        },
        MemberIdentityRow {
            label: "import",
            binding: IdentityBindingForm::Import,
            target_module: "identity_provider",
            owner_name: "ImportedMember",
            owner: DeclarationSpanSelector {
                anchor: "pub model ImportedMember:",
                occurrence: 0,
            },
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 0,
            },
            root_binding: "ImportedMember",
            receiver_call: "imported.read()",
            function_body: "imported_member_function_scope",
            block_body: "imported_member_block_scope",
        },
        MemberIdentityRow {
            label: "alias",
            binding: IdentityBindingForm::Alias,
            target_module: "identity_provider",
            owner_name: "AliasedMember",
            owner: DeclarationSpanSelector {
                anchor: "pub model AliasedMember:",
                occurrence: 0,
            },
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 1,
            },
            root_binding: "MemberAlias",
            receiver_call: "aliased.read()",
            function_body: "aliased_member_function_scope",
            block_body: "aliased_member_block_scope",
        },
        MemberIdentityRow {
            label: "re-export",
            binding: IdentityBindingForm::ReExport,
            target_module: "identity_provider",
            owner_name: "RelayedMember",
            owner: DeclarationSpanSelector {
                anchor: "pub model RelayedMember:",
                occurrence: 0,
            },
            member: DeclarationSpanSelector {
                anchor: "def read(self) -> int:",
                occurrence: 2,
            },
            root_binding: "MemberReexport",
            receiver_call: "relayed.read()",
            function_body: "reexported_member_function_scope",
            block_body: "reexported_member_block_scope",
        },
    ] {
        verify_member_matrix_row(graph, &row, &mut assertions)?;
    }
    for row in [
        PathIdentityRow {
            label: "import",
            binding: IdentityBindingForm::Import,
            expected_module_path: &["identity_provider"],
            expected_module_name: "identity_provider",
            target_name: "imported_path",
            module_binding: "identity_provider",
            call: "identity_provider.imported_path(17)",
            function_body: "imported_path_function_scope",
            block_body: "imported_path_block_scope",
        },
        PathIdentityRow {
            label: "alias",
            binding: IdentityBindingForm::Alias,
            expected_module_path: &["identity_provider"],
            expected_module_name: "identity_provider",
            target_name: "aliased_path",
            module_binding: "provider_alias",
            call: "provider_alias.aliased_path(19)",
            function_body: "aliased_path_function_scope",
            block_body: "aliased_path_block_scope",
        },
    ] {
        verify_path_matrix_row(graph, &row, &mut assertions)?;
    }
    validate_identity_matrix_coverage(&assertions.coverage_cells)?;
    Ok(assertions)
}

pub(super) const LET_SHADOW_SRC: &str = r#"
def shadow_let() -> int:
    mut total = 0
    x = 1
    if true:
        let x = 2
        total += x
    return total + x
"#;

pub(super) const LET_SHADOW_MODULES: &[IdentitySourceModule] = &[IdentitySourceModule {
    name: "identity_let_shadow",
    path: &["identity_let_shadow"],
    source: LET_SHADOW_SRC,
    dependencies: &[],
}];

pub(super) const MUT_SHADOW_SRC: &str = r#"
def shadow_mut() -> int:
    mut total = 0
    mut x = 4
    if true:
        mut x = 7
        total += x
    return total + x
"#;

pub(super) const MUT_SHADOW_MODULES: &[IdentitySourceModule] = &[IdentitySourceModule {
    name: "identity_mut_shadow",
    path: &["identity_mut_shadow"],
    source: MUT_SHADOW_SRC,
    dependencies: &[],
}];

pub(super) const GENERIC_BINDER_SRC: &str = r#"
def generic_identity[T](value: T) -> T:
    return value

def generic_entry() -> int:
    return generic_identity[int](42)
"#;

pub(super) const GENERIC_BINDER_MODULES: &[IdentitySourceModule] = &[IdentitySourceModule {
    name: "identity_generic",
    path: &["identity_generic"],
    source: GENERIC_BINDER_SRC,
    dependencies: &[],
}];

pub(super) const BUILTIN_REBINDING_SRC: &str = r#"
def len(value: int) -> int:
    return value + 1

def builtin_entry() -> int:
    return len(4) + std.builtins.len([1, 2, 3])
"#;

pub(super) const BUILTIN_REBINDING_MODULES: &[IdentitySourceModule] = &[IdentitySourceModule {
    name: "identity_builtin",
    path: &["identity_builtin"],
    source: BUILTIN_REBINDING_SRC,
    dependencies: &[],
}];

pub(super) fn no_replacement_arguments() -> Vec<ReplacementValue> {
    Vec::new()
}

pub(super) fn expected_three() -> ReplacementValue {
    ReplacementValue::Int(3)
}

pub(super) fn expected_eleven() -> ReplacementValue {
    ReplacementValue::Int(11)
}

pub(super) fn expected_forty_two() -> ReplacementValue {
    ReplacementValue::Int(42)
}

pub(super) fn expected_eight() -> ReplacementValue {
    ReplacementValue::Int(8)
}

fn verify_shadow_binding(
    graph: &CheckedIdentityGraph,
    module: &str,
    body: &str,
    declaration_name: &str,
) -> Result<IdentityAssertions, String> {
    // Binding tokens introduce declarations and are intentionally absent from the checked-reference map. Read the
    // identities selected by the two use sites, then require Body IR to carry those same declaration identities.
    let inner_read = graph.resolved_identity(module, "x", 2)?;
    let outer_read = graph.resolved_identity(module, "x", 3)?;
    if outer_read == inner_read {
        return Err("same-spelled shadow declarations collapsed to one canonical identity".to_string());
    }
    let mut checked_relations = Vec::new();
    checked_relations.push(format!(
        "shadow declarations distinct: {} != {}",
        outer_read.render_compact(),
        inner_read.render_compact()
    ));

    let locals = graph.body_local_identities(module, body, "x")?;
    if locals.len() != 2
        || !locals.iter().any(|identity| identity == &outer_read)
        || !locals.iter().any(|identity| identity == &inner_read)
    {
        return Err(format!(
            "Body IR did not retain both canonical shadow locals, got {locals:?}"
        ));
    }
    let function = graph.declaration_identity(
        module,
        declaration_name,
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let hir_function = graph.hir_identity(module, declaration_name)?;
    require_same_identity("shadow entry HIR", &function, &hir_function, &mut checked_relations)?;
    Ok(IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations,
        hir_consumers: vec![format!("shadow entry HIR: {}", hir_function.render_compact())],
        body_ir_consumers: locals
            .iter()
            .map(|identity| format!("shadow local: {}", identity.render_compact()))
            .collect(),
        legacy_projections: vec![graph.require_emitted_projection(module, &function)?],
        artifact_observations: Vec::new(),
    })
}

pub(super) fn verify_let_shadow(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    verify_shadow_binding(graph, "identity_let_shadow", "shadow_let", "shadow_let")
}

pub(super) fn verify_mut_shadow(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    verify_shadow_binding(graph, "identity_mut_shadow", "shadow_mut", "shadow_mut")
}

pub(super) fn verify_generic_binder(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    // The binder token introduces a declaration, so it deliberately does not appear in the checker-owned reference
    // map. Its annotations are references to that declaration and carry the canonical GenericBinder identity into
    // downstream consumers.
    let parameter = graph.resolved_identity("identity_generic", "T", 1)?;
    let return_type = graph.resolved_identity("identity_generic", "T", 2)?;
    if parameter.kind != SemanticSourceTargetKind::GenericBinder {
        return Err(format!(
            "generic parameter annotation did not retain its GenericBinder identity: {}",
            parameter.render_compact()
        ));
    }
    let mut checked_relations = Vec::new();
    require_same_identity(
        "generic binder annotations",
        &parameter,
        &return_type,
        &mut checked_relations,
    )?;
    let concrete_int = graph.resolved_identity("identity_generic", "int", 1)?;
    if concrete_int == parameter || concrete_int.kind == SemanticSourceTargetKind::GenericBinder {
        return Err(format!(
            "generic binder collapsed into concrete `int`: binder={}, concrete={}",
            parameter.render_compact(),
            concrete_int.render_compact()
        ));
    }
    checked_relations.push(format!(
        "generic binder/concrete distinct: {} != {}",
        parameter.render_compact(),
        concrete_int.render_compact()
    ));
    let generic_function = graph.declaration_identity(
        "identity_generic",
        "generic_identity",
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let call = graph.resolved_identity("identity_generic", "generic_identity[int](42)", 0)?;
    require_same_identity(
        "generic callable selection",
        &generic_function,
        &call,
        &mut checked_relations,
    )?;
    let body_consumers = graph.body_consumer_identities("identity_generic", "generic_entry")?;
    if !body_consumers.iter().any(|identity| identity == &generic_function) {
        return Err("generic call target did not survive into replacement-facing Body IR".to_string());
    }
    let hir_function = graph.hir_identity("identity_generic", "generic_identity")?;
    require_same_identity(
        "generic function HIR",
        &generic_function,
        &hir_function,
        &mut checked_relations,
    )?;
    Ok(IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations,
        hir_consumers: vec![format!("generic function HIR: {}", hir_function.render_compact())],
        body_ir_consumers: body_consumers
            .iter()
            .map(|identity| format!("generic Body IR consumer: {}", identity.render_compact()))
            .collect(),
        legacy_projections: vec![graph.require_emitted_projection("identity_generic", &generic_function)?],
        artifact_observations: Vec::new(),
    })
}

pub(super) fn verify_builtin_rebinding(graph: &CheckedIdentityGraph) -> Result<IdentityAssertions, String> {
    let local = graph.declaration_identity(
        "identity_builtin",
        "len",
        SemanticSourceTargetKind::Function,
        SymbolNamespace::OrdinaryLexical,
    )?;
    let local_call = graph.resolved_identity("identity_builtin", "len(4)", 0)?;
    let builtin_call = graph.resolved_identity("identity_builtin", "std.builtins.len([1, 2, 3])", 0)?;
    if builtin_call.kind != SemanticSourceTargetKind::Builtin
        || builtin_call.namespace != SymbolNamespace::OrdinaryLexical
        || builtin_call == local
    {
        return Err(format!(
            "builtin qualification did not stay distinct from the ordinary lexical binding: local={}, builtin={}",
            local.render_compact(),
            builtin_call.render_compact()
        ));
    }
    let mut checked_relations = Vec::new();
    require_same_identity(
        "ordinary builtin-name rebinding",
        &local,
        &local_call,
        &mut checked_relations,
    )?;
    checked_relations.push(format!(
        "ordinary/builtin distinct: {} != {}",
        local.render_compact(),
        builtin_call.render_compact()
    ));
    let body_consumers = graph.body_consumer_identities("identity_builtin", "builtin_entry")?;
    if !body_consumers.iter().any(|identity| identity == &local)
        || !body_consumers.iter().any(|identity| identity == &builtin_call)
    {
        return Err(format!(
            "Body IR did not retain both local and builtin targets: {body_consumers:?}"
        ));
    }
    let hir_local = graph.hir_identity("identity_builtin", "len")?;
    require_same_identity(
        "ordinary builtin-name binding HIR",
        &local,
        &hir_local,
        &mut checked_relations,
    )?;
    Ok(IdentityAssertions {
        coverage_cells: Vec::new(),
        checked_relations,
        hir_consumers: vec![format!("local len HIR: {}", hir_local.render_compact())],
        body_ir_consumers: body_consumers
            .iter()
            .map(|identity| format!("builtin row Body IR consumer: {}", identity.render_compact()))
            .collect(),
        legacy_projections: vec![graph.require_emitted_projection("identity_builtin", &local)?],
        artifact_observations: Vec::new(),
    })
}

pub(super) fn verify_release_artifact() -> Result<ReleaseArtifactAssertions, String> {
    static EVIDENCE: OnceLock<Result<ReleaseArtifactAssertions, String>> = OnceLock::new();
    EVIDENCE
        .get_or_init(|| {
            let evidence = emitted_symbol_artifact::verify_pinned_release_artifact()
                .map_err(|error| format!("pinned release artifact verification failed: {error}"))?;
            if evidence.recovered_identities.len() != 4
                || !evidence.saw_generic_u64_specialization
                || !evidence.saw_non_incan_host_symbol
            {
                return Err(format!(
                    "pinned release artifact returned incomplete evidence: {evidence:?}"
                ));
            }
            Ok(ReleaseArtifactAssertions {
                assertions: IdentityAssertions {
                    coverage_cells: Vec::new(),
                    checked_relations: Vec::new(),
                    hir_consumers: Vec::new(),
                    body_ir_consumers: Vec::new(),
                    legacy_projections: evidence
                        .recovered_identities
                        .iter()
                        .map(|identity| format!("recovered incan-v1: {}", identity.render_compact()))
                        .collect(),
                    artifact_observations: vec![
                        format!("rustc {} optimized v0 artifact", emitted_symbol_artifact::SELECTED_RUST),
                        "generic specialization u64 recovered".to_string(),
                        "host_bridge classified as non-Incan".to_string(),
                        format!(
                            "artifact bytes baseline={} projected={}",
                            evidence.baseline_bytes, evidence.projected_bytes
                        ),
                        format!(
                            "identifier bytes baseline={} projected={}",
                            evidence.baseline_identifier_bytes, evidence.projected_identifier_bytes
                        ),
                    ],
                },
                fixture_input_identity: evidence.fixture_input_identity,
                artifact_content_identity: evidence.artifact_content_identity,
                recovered_observation_identity: evidence.recovered_observation_identity,
            })
        })
        .clone()
}
