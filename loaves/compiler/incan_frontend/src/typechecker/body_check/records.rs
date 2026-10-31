//! Selected owner projections for the Incan checker-record codec. This is a bootstrap conversion, not a store.

mod diagnostics;
mod types;
use diagnostics::{project_diagnostics, read_diagnostic};
mod unsupported;

use std::collections::BTreeMap;
use std::sync::Arc;

use super::super::canonical_nodes::{NodeBindingError, SemanticNodeAnchor, SemanticNodeBindings};
use super::super::diagnostic_values::DiagnosticPositions;
use super::{BodyLocal, CheckedBodyView, LocalOwnerKind, TypeChecker};
use crate::ast::{Program, Span};

use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::stable_identity::StableDeclarationId;
use types::{next, parameter_fields, read_parameters, read_type, type_fields};

/// The native boundary carries only field primitives. The Incan schema owns their versioned encoding.
pub(crate) type Fields = Vec<Vec<String>>;

/// Current coordinates and canonical activation handles, keyed by persisted semantic identity. No type, call or
/// diagnostic payload is retained here. The test sink may build these from a separate fresh checker oracle.
struct Positions {
    nodes: SemanticNodeBindings,
    identities: BTreeMap<String, CanonicalSymbolId>,
}

impl Positions {
    /// Select exactly one owner's syntax and current declaration/binding handles; never take semantic body tables.
    fn new(checker: &TypeChecker, program: &Program, owner: Span, declaration: bool) -> Result<Self, NodeBindingError> {
        let nodes = if declaration {
            SemanticNodeBindings::declaration(checker, program, owner)?
        } else {
            SemanticNodeBindings::new(checker, program, owner)?
        };
        let mut identities = checker
            .type_info
            .declarations
            .source_identities
            .iter()
            .map(|(canonical, stable)| (stable.render_compact(), canonical.clone()))
            .collect::<BTreeMap<_, _>>();
        for index in 0..checker.symbols.all_symbols().len() {
            let Some(canonical) = checker.symbols.identity_of(index) else {
                continue;
            };
            if canonical.origin != incan_semantics_core::SymbolOrigin::Builtin {
                continue;
            }
            if let Some(stable) = StableDeclarationId::from_canonical(canonical, None, None) {
                identities.insert(stable.render_compact(), canonical.clone());
            }
        }
        let nested = checker
            .checked_body(owner)
            .into_iter()
            .chain(checker.checked_declaration(owner))
            .filter(|view| !declaration || view.local.kind == LocalOwnerKind::Declaration)
            .flat_map(|view| view.local.nested.values());
        let raw = checker
            .body_checks
            .iter()
            .map(|input| input.local())
            .filter(|local| local.owner == owner && (!declaration || local.kind == LocalOwnerKind::Declaration))
            .flat_map(|local| local.nested.values());
        for (canonical, stable) in nodes.binding_identities(nested.chain(raw))? {
            identities.insert(stable.render_compact(), canonical);
        }
        Ok(Self { nodes, identities })
    }

    /// Only a checked declaration or an explicitly admitted builtin can become a persistent reference.
    fn identity(&self, canonical: &CanonicalSymbolId) -> Result<String, NodeBindingError> {
        if let Some((stable, _)) = self.identities.iter().find(|(_, current)| *current == canonical) {
            return Ok(stable.clone());
        }
        if canonical.origin == incan_semantics_core::SymbolOrigin::Builtin {
            return StableDeclarationId::from_canonical(canonical, None, None)
                .map(|stable| stable.render_compact())
                .ok_or(NodeBindingError::MissingOwner);
        }
        Err(NodeBindingError::Unsupported("unadmitted declaration reference"))
    }

    /// Current scope indices and spans are supplied by this activation's binding map, never parsed from a key.
    fn canonical(&self, identity: &str) -> Result<CanonicalSymbolId, NodeBindingError> {
        self.identities
            .get(identity)
            .cloned()
            .ok_or(NodeBindingError::MissingNode)
    }

    /// Bind a structural path only under the already authenticated selected owner/root.
    fn span(&self, path: &str) -> Result<Span, NodeBindingError> {
        self.nodes.resolve(&SemanticNodeAnchor {
            owner: self.nodes.owner().clone(),
            root: self.nodes.root().to_owned(),
            path: path.to_owned(),
        })
    }
}

/// Project one declaration or settled body into portable field primitives; every unsupported nonempty native field
/// refuses the profile. The caller owns the result and can destroy every original native value after this returns.
pub(crate) fn project(
    checker: &TypeChecker,
    program: &Program,
    owner: Span,
    declaration: bool,
) -> Result<Fields, NodeBindingError> {
    let positions = Positions::new(checker, program, owner, declaration)?;
    let view = if declaration {
        checker.checked_declaration(owner)
    } else {
        checker.checked_body(owner)
    }
    .ok_or(NodeBindingError::MissingOwner)?;
    validate(view)?;
    let mut fields = local_fields(&positions, &view.local)?;
    if declaration {
        project_signature(checker, program, owner, &positions, &mut fields)?;
    }
    project_diagnostics(checker, program, &positions, view, &mut fields)?;
    project_final_types(checker, view, &positions, &mut fields)?;
    fields.push(vec![
        "status".into(),
        String::new(),
        "refused".into(),
        view.is_refused().to_string(),
    ]);
    fields.sort();
    fields.insert(
        0,
        vec![
            "header".into(),
            positions.nodes.owner().render_compact(),
            positions.nodes.root().to_owned(),
            if declaration { "declaration" } else { "final" }.into(),
            "incan.checker-free-functions/1".into(),
        ],
    );
    Ok(fields)
}

/// Rebuild a selected owner solely from canonical fields and current position/identity bindings. The independently
/// checked current oracle supplies coordinates/handles only; no native fact payload is copied from it.
fn restore(
    positions: &Positions,
    program: &Program,
    owner: Span,
    fields: &[Vec<String>],
) -> Result<NativeOwner, NodeBindingError> {
    let header = fields
        .first()
        .filter(|header| header.len() == 5)
        .ok_or(NodeBindingError::MissingOwner)?;
    if header[0] != "header" || header[4] != "incan.checker-free-functions/1" {
        return Err(NodeBindingError::Unsupported("record profile"));
    }
    if header[1] != positions.nodes.owner().render_compact() {
        return Err(NodeBindingError::WrongOwner);
    }
    if header[2] != positions.nodes.root() {
        return Err(NodeBindingError::ChangedBody);
    }
    let kind = match header[3].as_str() {
        "declaration" => LocalOwnerKind::Declaration,
        "final" | "local" => LocalOwnerKind::Body,
        _ => return Err(NodeBindingError::Unsupported("record stage")),
    };
    let mut rebound = Rebinding {
        local: BodyLocal {
            owner,
            kind,
            ..BodyLocal::default()
        },
        ..Rebinding::default()
    };
    let mut previous: Option<&[String]> = None;
    for row in &fields[1..] {
        if row.len() < 4 || previous.is_some_and(|previous| previous[..3] >= row[..3]) {
            return Err(NodeBindingError::InvalidField(
                "canonical ordering or field width".into(),
            ));
        }
        previous = Some(row);
        rebound.field(program, positions, header, row).map_err(|error| {
            if error == NodeBindingError::MissingNode {
                NodeBindingError::InvalidField(format!("family={} node={} selector={}", row[0], row[1], row[2]))
            } else {
                error
            }
        })?;
    }
    rebound.finish(&header[3])
}

/// Rebinding owns decoded native scratch only for this activation; persistent bytes and source lifetimes stay outside.
#[derive(Default)]
struct Rebinding {
    local: BodyLocal,
    diagnostics: BTreeMap<String, super::super::diagnostic_values::AnchoredDiagnostic>,
    warnings: BTreeMap<String, super::super::diagnostic_values::AnchoredDiagnostic>,
    bodies: Vec<CanonicalSymbolId>,
    status: Option<bool>,
    binding: Option<super::super::type_info::FunctionBindingInfo>,
    defaults: BTreeMap<Span, (Span, String)>,
    final_types: FinalTypes,
}

impl Rebinding {
    /// Dispatch exactly one admitted field grammar. Unrecognized selectors never fall back to native source facts.
    fn field(
        &mut self,
        program: &Program,
        positions: &Positions,
        header: &[String],
        row: &[String],
    ) -> Result<(), NodeBindingError> {
        match row[0].as_str() {
            "status"
                if header[3] == "local"
                    && row[1].is_empty()
                    && row[2] == "pending"
                    && row.len() == 4
                    && row[3] == "true" =>
            {
                self.status = Some(true)
            }
            "status" if row[1].is_empty() && row[2] == "refused" && row.len() == 4 => {
                self.status = Some(row[3].parse::<bool>().map_err(|_| NodeBindingError::MissingNode)?)
            }
            "effect" if row[1].is_empty() && row[3] == "checked" && row.len() == 4 => {
                self.bodies.push(positions.canonical(&row[2])?)
            }
            "signature" if row[2] == "declaration" => self.signature(positions, header, row)?,
            "default" if row.len() == 4 && row[3] == header[1] => {
                let span = positions.span(&row[1])?;
                if self.defaults.insert(span, (self.local.owner, row[2].clone())).is_some() {
                    return Err(NodeBindingError::MissingNode);
                }
            }
            "final-type" if header[3] != "local" => self.final_type(positions, row)?,
            "native-call"
                if header[3] != "local"
                    && row[1].is_empty()
                    && row[2] == "empty"
                    && row.len() == 4
                    && row[3] == "true" => {}
            "diagnostic" if header[3] != "local" => self.diagnostic(program, positions, row)?,
            _ => restore_local(positions, &mut self.local, row)?,
        }
        Ok(())
    }

    /// Restore declaration signatures from their canonical parameter/type primitives, not from the current binding.
    fn signature(&mut self, positions: &Positions, header: &[String], row: &[String]) -> Result<(), NodeBindingError> {
        if self.binding.is_some()
            || header[3] != "declaration"
            || row[1] != positions.nodes.anchor(self.local.owner)?.path
        {
            return Err(NodeBindingError::MissingNode);
        }
        let mut values = row[3..].iter();
        let is_async = next(&mut values)?
            .parse::<bool>()
            .map_err(|_| NodeBindingError::MissingNode)?;
        let params = read_parameters(&mut values)?;
        let return_type = read_type(&mut values)?;
        if values.next().is_some() {
            return Err(NodeBindingError::MissingNode);
        }
        self.binding = Some(super::super::type_info::FunctionBindingInfo {
            is_async,
            params,
            return_type,
            identity: Some(positions.canonical(&header[1])?),
        });
        Ok(())
    }

    /// Finalized own expression/binding values are separate from pending local checks and their original snapshots.
    fn final_type(&mut self, positions: &Positions, row: &[String]) -> Result<(), NodeBindingError> {
        let mut values = row[3..].iter();
        match row[2].as_str() {
            "expression" => {
                self.final_types
                    .expressions
                    .insert(positions.span(&row[1])?, read_type(&mut values)?);
            }
            "binding" if !row[1].is_empty() => {
                let canonical = positions.canonical(&row[1])?;
                self.final_types.bindings.insert(canonical, read_type(&mut values)?);
            }
            "empty" if row[1].is_empty() && row.len() == 4 && row[3] == "true" => {
                let _ = next(&mut values)?;
            }
            _ => return Err(NodeBindingError::MissingNode),
        }
        if values.next().is_some() {
            return Err(NodeBindingError::MissingNode);
        }
        Ok(())
    }

    /// Retain the legacy diagnostic order independently of canonical field sorting by structural path.
    fn diagnostic(&mut self, program: &Program, positions: &Positions, row: &[String]) -> Result<(), NodeBindingError> {
        let value = read_diagnostic(program, positions, row)?;
        let map = match row[2].split_once(':').map(|(kind, _)| kind) {
            Some("error") => &mut self.diagnostics,
            Some("warning") => &mut self.warnings,
            _ => return Err(NodeBindingError::MissingNode),
        };
        if map.insert(row[2].clone(), value).is_some() {
            return Err(NodeBindingError::MissingNode);
        }
        Ok(())
    }

    /// Finish as either a pending local value or an independently settled owner; no local fact is promoted by decoding.
    fn finish(mut self, stage: &str) -> Result<NativeOwner, NodeBindingError> {
        self.local.mutation = super::super::mut_arguments::MutBodyLocal::from_plain_bodies(self.bodies);
        if stage == "local" {
            if self.status != Some(true)
                || !self.diagnostics.is_empty()
                || !self.warnings.is_empty()
                || self.binding.is_some()
                || !self.defaults.is_empty()
            {
                return Err(NodeBindingError::MissingNode);
            }
            return Ok(NativeOwner::Local(self.local));
        }
        let diagnostics = ordered_diagnostics(self.diagnostics, "error")?;
        let warnings = ordered_diagnostics(self.warnings, "warning")?;
        let view = CheckedBodyView {
            local: Arc::new(self.local),
            effect_reads: Default::default(),
            diagnostics,
            warnings,
            mutation: Vec::new(),
            dictionary: Vec::new(),
            open: Vec::new(),
        };
        if self.status != Some(view.is_refused()) || (stage == "declaration") != self.binding.is_some() {
            return Err(NodeBindingError::MissingNode);
        }
        Ok(NativeOwner::Settled(RestoredOwner {
            view,
            binding: self.binding,
            defaults: self.defaults,
            final_types: self.final_types,
        }))
    }
}

/// Require contiguous canonical diagnostic indices before dropping their invocation-independent order keys.
fn ordered_diagnostics(
    values: BTreeMap<String, super::super::diagnostic_values::AnchoredDiagnostic>,
    kind: &str,
) -> Result<Vec<super::super::diagnostic_values::AnchoredDiagnostic>, NodeBindingError> {
    values
        .into_iter()
        .enumerate()
        .map(|(index, (key, value))| {
            if key != format!("{kind}:{index:08}") {
                return Err(NodeBindingError::MissingNode);
            }
            Ok(value)
        })
        .collect()
}

/// Select complete supported expression maps. Their native field names are schema selectors, not memory offsets.
fn project_expressions(positions: &Positions, local: &BodyLocal, fields: &mut Fields) -> Result<(), NodeBindingError> {
    let expressions = &local.expressions;
    for (selector, values) in [
        ("expr", &expressions.expr_types),
        ("body-expr", &expressions.body_ir_expr_types),
        ("assignment", &expressions.assignment_binding_types),
        ("destination", &expressions.value_destination_types),
    ] {
        for (span, ty) in values {
            let mut row = vec!["type".into(), positions.nodes.anchor(*span)?.path, selector.into()];
            type_fields(ty, &mut row)?;
            fields.push(row);
        }
    }
    for (span, kind) in &expressions.ident_kinds {
        let tag = match kind {
            super::super::type_info::IdentKind::Value => "value",
            _ => return Err(NodeBindingError::Unsupported("identifier kind")),
        };
        fields.push(vec![
            "identifier".into(),
            positions.nodes.anchor(*span)?.path,
            "kind".into(),
            tag.into(),
        ]);
    }
    for (span, target) in &expressions.source_targets {
        let mut row = vec![
            "target".into(),
            positions.nodes.anchor(*span)?.path,
            "source".into(),
            target.name.clone(),
            target.kind.clone(),
        ];
        row.extend(target.module_path.iter().cloned());
        fields.push(row);
    }
    Ok(())
}

/// References keep their stable target and a structural source role; parameter writes stay declaration-owned.
fn project_references(positions: &Positions, local: &BodyLocal, fields: &mut Fields) -> Result<(), NodeBindingError> {
    for (span, canonical) in &local.references.resolved_identities {
        fields.push(vec![
            "reference".into(),
            positions.nodes.anchor(*span)?.path,
            "read".into(),
            positions.identity(canonical)?,
        ]);
    }
    for ((span, name), canonical) in &local.references.resolved_write_identities {
        fields.push(vec![
            "write".into(),
            positions.nodes.anchor(*span)?.path,
            name.clone(),
            positions.identity(canonical)?,
        ]);
    }
    for ((span, name), ty) in &local.references.resolved_write_types {
        let mut row = vec!["parameter".into(), positions.nodes.anchor(*span)?.path, name.clone()];
        type_fields(ty, &mut row)?;
        fields.push(row);
    }
    Ok(())
}

/// Calls retain the selected builtin and checked parameter/type arguments, not a re-resolved source name.
fn project_calls(positions: &Positions, local: &BodyLocal, fields: &mut Fields) -> Result<(), NodeBindingError> {
    for (span, builtin) in &local.calls.resolved_builtin_calls {
        fields.push(vec![
            "builtin".into(),
            positions.nodes.anchor(*span)?.path,
            "call".into(),
            incan_lang::lang::builtins::as_str(*builtin).into(),
        ]);
    }
    for (selector, values) in [
        ("call", &local.calls.call_site_callable_params),
        ("body-call", &local.calls.body_ir_call_parameter_types),
    ] {
        for (span, parameters) in values {
            let mut row = vec!["signature".into(), positions.nodes.anchor(*span)?.path, selector.into()];
            parameter_fields(parameters, &mut row)?;
            fields.push(row);
        }
    }
    for (selector, values) in [
        ("monomorphs", &local.calls.call_site_monomorph_type_args),
        ("type-arguments", &local.calls.body_ir_call_type_arguments),
    ] {
        for (span, arguments) in values {
            let mut row = vec![
                "signature".into(),
                positions.nodes.anchor(*span)?.path,
                selector.into(),
                arguments.len().to_string(),
            ];
            for ty in arguments {
                type_fields(ty, &mut row)?;
            }
            fields.push(row);
        }
    }
    if !local.calls.body_ir_call_result_types.is_empty() || !local.calls.body_ir_native_argument_types.is_empty() {
        return Err(NodeBindingError::Unsupported("native call refinements"));
    }
    Ok(())
}

/// Every unrepresented family is explicitly refused before a canonical value can escape.
fn validate(view: &CheckedBodyView) -> Result<(), NodeBindingError> {
    if !view.effect_reads.is_empty()
        || !view.mutation.is_empty()
        || !view.dictionary.is_empty()
        || !view.open.is_empty()
        || !view.local.iterator_sum_items.is_empty()
        || !view.local.dictionary.is_empty()
        || !view.local.diagnostic_slots.is_empty()
        || !view.local.warning_slots.is_empty()
        || view.local.order.mutation != 0
        || view.local.order.dictionary != 0
    {
        return Err(NodeBindingError::Unsupported("deferred effect or diagnostic ordering"));
    }
    Ok(())
}

/// Recreate every supported local map directly from the selected field's typed primitives.
fn restore_local(positions: &Positions, local: &mut BodyLocal, row: &[String]) -> Result<(), NodeBindingError> {
    let span = positions.span(&row[1])?;
    let mut values = row[3..].iter();
    match row[0].as_str() {
        "type" => {
            let ty = read_type(&mut values)?;
            let map = match row[2].as_str() {
                "expr" => &mut local.expressions.expr_types,
                "body-expr" => &mut local.expressions.body_ir_expr_types,
                "assignment" => &mut local.expressions.assignment_binding_types,
                "destination" => &mut local.expressions.value_destination_types,
                _ => return Err(NodeBindingError::MissingNode),
            };
            map.insert(span, ty);
        }
        "parameter" => {
            local
                .references
                .resolved_write_types
                .insert((span, row[2].clone()), read_type(&mut values)?);
        }
        "reference" if row[2] == "read" => {
            local
                .references
                .resolved_identities
                .insert(span, positions.canonical(next(&mut values)?)?);
        }
        "write" => {
            local
                .references
                .resolved_write_identities
                .insert((span, row[2].clone()), positions.canonical(next(&mut values)?)?);
        }
        "binding" if row[2] == "declaration" => {
            local.nested.insert(span, positions.canonical(next(&mut values)?)?);
        }
        "identifier" if row[2] == "kind" && next(&mut values)? == "value" => {
            local
                .expressions
                .ident_kinds
                .insert(span, super::super::type_info::IdentKind::Value);
        }
        "builtin" if row[2] == "call" => {
            let builtin =
                incan_lang::lang::builtins::from_str(next(&mut values)?).ok_or(NodeBindingError::MissingNode)?;
            local.calls.resolved_builtin_calls.insert(span, builtin);
        }
        "signature" if matches!(row[2].as_str(), "monomorphs" | "type-arguments") => {
            let count = next(&mut values)?
                .parse::<usize>()
                .map_err(|_| NodeBindingError::MissingNode)?;
            if count > values.len() {
                return Err(NodeBindingError::MissingNode);
            }
            let arguments = (0..count)
                .map(|_| read_type(&mut values))
                .collect::<Result<Vec<_>, _>>()?;
            let map = if row[2] == "monomorphs" {
                &mut local.calls.call_site_monomorph_type_args
            } else {
                &mut local.calls.body_ir_call_type_arguments
            };
            map.insert(span, arguments);
        }
        "signature" => {
            let parameters = read_parameters(&mut values)?;
            let map = match row[2].as_str() {
                "call" => &mut local.calls.call_site_callable_params,
                "body-call" => &mut local.calls.body_ir_call_parameter_types,
                _ => return Err(NodeBindingError::MissingNode),
            };
            map.insert(span, parameters);
        }
        "target" if row[2] == "source" => {
            let name = next(&mut values)?.to_owned();
            let kind = next(&mut values)?.to_owned();
            let module_path = values.by_ref().cloned().collect();
            local.expressions.source_targets.insert(
                span,
                super::super::type_info::SourceTargetInfo {
                    name,
                    kind,
                    module_path,
                },
            );
        }
        _ => return Err(NodeBindingError::Unsupported("canonical field")),
    }
    if values.next().is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

/// Extract the exact authored declaration range from its current activation handle.
fn span_of(canonical: &CanonicalSymbolId) -> Span {
    Span {
        start: canonical.declaration_span.start,
        end: canonical.declaration_span.end,
        expansion: canonical.declaration_span.expansion,
    }
}

/// Test-sink entry: check a fixture, select one owner, return only its canonical primitives and drop the checker.
/// This intentionally measures no production reuse; a future record producer calls `project` at its own boundary.
pub(crate) fn fixture_projection(source: &str, name: &str, declaration: bool) -> Result<Fields, NodeBindingError> {
    let (program, checker, owner) = fixture(source, name)?;
    project(&checker, &program, owner, declaration)
}

/// Test oracle: independently check the current revision, retain its coordinate bindings, reconstruct the supplied
/// fields and compare every native field. The cold checker supplies expected values only, never reconstructed ones.
pub(crate) fn verify_fixture(source: &str, name: &str, fields: Fields) -> Result<Vec<String>, NodeBindingError> {
    let (program, checker, owner) = fixture(source, name)?;
    let declaration = fields
        .first()
        .is_some_and(|header| header.get(3).is_some_and(|role| role == "declaration"));
    let expected = if declaration {
        checker.checked_declaration(owner)
    } else {
        checker.checked_body(owner)
    }
    .cloned()
    .ok_or(NodeBindingError::MissingOwner)?;
    let positions = Positions::new(&checker, &program, owner, declaration)?;
    let expected_binding = checker.declaration_view().function(owner).cloned();
    let expected_types = final_type_values(&checker, &expected);
    drop(checker);
    let NativeOwner::Settled(restored) = restore(&positions, &program, owner, &fields)? else {
        return Err(NodeBindingError::MissingNode);
    };
    compare_native(&expected, &restored.view)?;
    if declaration && restored.binding != expected_binding {
        return Err(NodeBindingError::OracleDifference("declaration signature".into()));
    }
    if declaration && restored.defaults != default_writers(&program, owner)? {
        return Err(NodeBindingError::OracleDifference("selected default writers".into()));
    }
    if expected_types != restored.final_types {
        return Err(NodeBindingError::OracleDifference(
            "finalized own type projections".into(),
        ));
    }
    let mut current = DiagnosticPositions::new(&program, positions.nodes.owner().origin.clone());
    restored
        .view
        .diagnostics
        .iter()
        .chain(&restored.view.warnings)
        .map(|diagnostic| {
            let error = current.render(diagnostic).map_err(|_| NodeBindingError::MissingNode)?;
            Ok(crate::diagnostics::format_error("case.incn", source, &error))
        })
        .collect()
}

/// The complete native value is an independent oracle only; every sorted map, sequence and scalar remains visible.
fn compare_native(expected: &impl std::fmt::Debug, restored: &impl std::fmt::Debug) -> Result<(), NodeBindingError> {
    let before = super::oracle_compare::canonical_debug(&format!("{expected:?}"));
    let after = super::oracle_compare::canonical_debug(&format!("{restored:?}"));
    if before != after {
        return Err(NodeBindingError::OracleDifference(format!(
            "expected {before}; actual {after}"
        )));
    }
    Ok(())
}

/// Default writer membership comes from exact source default nodes; it never supplies a checked type or value.
fn default_writers(program: &Program, owner: Span) -> Result<BTreeMap<Span, (Span, String)>, NodeBindingError> {
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| declaration.span == owner)
        .ok_or(NodeBindingError::MissingOwner)?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err(NodeBindingError::Unsupported("default owner"));
    };
    Ok(function
        .params
        .iter()
        .filter_map(|parameter| {
            parameter
                .node
                .default
                .as_ref()
                .map(|default| (default.span, (owner, parameter.node.name.clone())))
        })
        .collect())
}

/// Independent full checking belongs only to the fixture oracle. Canonical production conversion never invokes it.
fn fixture(source: &str, name: &str) -> Result<(Program, TypeChecker, Span), NodeBindingError> {
    let tokens = crate::lexer::lex(source).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let program = crate::parser::parse(&tokens).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let owner = program.declarations.iter().find(|declaration| matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == name)).map(|declaration| declaration.span).ok_or(NodeBindingError::MissingOwner)?;
    let mut checker = TypeChecker::new();
    let _ = checker.check_program(&program);
    Ok((program, checker, owner))
}

/// Reconstructed semantic native values and default-writer references, separate from the current position bindings.
struct RestoredOwner {
    view: CheckedBodyView,
    binding: Option<super::super::type_info::FunctionBindingInfo>,
    defaults: BTreeMap<Span, (Span, String)>,
    final_types: FinalTypes,
}

/// Declaration-owned source signatures and exact default-writer references are independent from statement outputs.
fn project_signature(
    checker: &TypeChecker,
    program: &Program,
    owner: Span,
    positions: &Positions,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    let binding = checker
        .declaration_view()
        .function(owner)
        .ok_or(NodeBindingError::MissingOwner)?;
    let mut row = vec![
        "signature".into(),
        positions.nodes.anchor(owner)?.path,
        "declaration".into(),
        binding.is_async.to_string(),
    ];
    parameter_fields(&binding.params, &mut row)?;
    type_fields(&binding.return_type, &mut row)?;
    fields.push(row);
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| declaration.span == owner)
        .ok_or(NodeBindingError::MissingOwner)?;
    if let crate::ast::Declaration::Function(function) = &declaration.node {
        for parameter in &function.params {
            if let Some(default) = &parameter.node.default {
                let selected = checker
                    .checked_declaration(owner)
                    .ok_or(NodeBindingError::MissingOwner)?;
                if !selected.expressions().expr_types.contains_key(&default.span) {
                    return Err(NodeBindingError::Unsupported("default writer"));
                }
                fields.push(vec![
                    "default".into(),
                    positions.nodes.anchor(default.span)?.path,
                    parameter.node.name.clone(),
                    positions.nodes.owner().render_compact(),
                ]);
            }
        }
    }
    Ok(())
}

/// Pending local values are never presented as settled checked bodies, even in the test sink.
enum NativeOwner {
    Local(BodyLocal),
    Settled(RestoredOwner),
}

/// Capture the actual pre-settlement statement output, not a relabeled final body. Unsupported pending capabilities
/// refuse explicitly until the codec represents their decisions and references.
pub(crate) fn fixture_local_projection(source: &str, name: &str) -> Result<Fields, NodeBindingError> {
    let (program, checker, owner) = local_fixture(source, name)?;
    let local = checker
        .body_checks
        .iter()
        .map(|input| input.local())
        .find(|local| local.owner == owner && local.kind == LocalOwnerKind::Body)
        .ok_or(NodeBindingError::MissingOwner)?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let mut fields = local_fields(&positions, local)?;
    fields.push(vec!["status".into(), String::new(), "pending".into(), "true".into()]);
    fields.sort();
    fields.insert(
        0,
        vec![
            "header".into(),
            positions.nodes.owner().render_compact(),
            positions.nodes.root().to_owned(),
            "local".into(),
            "incan.checker-free-functions/1".into(),
        ],
    );
    Ok(fields)
}

/// Rebuild a pending local owner and compare every native field to an independent pre-settlement cold oracle.
pub(crate) fn verify_local_fixture(source: &str, name: &str, fields: Fields) -> Result<(), NodeBindingError> {
    let (program, checker, owner) = local_fixture(source, name)?;
    let positions = Positions::new(&checker, &program, owner, false)?;
    let expected = checker
        .body_checks
        .iter()
        .map(|input| input.local())
        .find(|local| local.owner == owner && local.kind == LocalOwnerKind::Body)
        .cloned()
        .ok_or(NodeBindingError::MissingOwner)?;
    drop(checker);
    let NativeOwner::Local(local) = restore(&positions, &program, owner, &fields)? else {
        return Err(NodeBindingError::MissingNode);
    };
    compare_native(&expected, &local)
}

/// Run only existing declaration and statement operations, stopping before settlement. This is a fixture oracle,
/// never a production per-record checking entry or a mechanism for filling missing record fields.
fn local_fixture(source: &str, name: &str) -> Result<(Program, TypeChecker, Span), NodeBindingError> {
    let tokens = crate::lexer::lex(source).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let program = crate::parser::parse(&tokens).map_err(|_| NodeBindingError::Unsupported("fixture syntax"))?;
    let owner = program.declarations.iter().find(|declaration| matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == name)).map(|declaration| declaration.span).ok_or(NodeBindingError::MissingOwner)?;
    let mut checker = TypeChecker::new();
    checker.reset_program_check(&program, false);
    checker.collect_program_check_declarations(&program);
    checker.record_program_header_facts();
    checker.record_source_declaration_identities(&program);
    checker.visibility_point = super::super::VisibilityPoint::Collected;
    checker.prepare_program_body_facts();
    for declaration in &program.declarations {
        checker.check_environment_declaration(declaration);
    }
    Ok((program, checker, owner))
}

/// The initial local profile proves every unsupported collection empty and refuses traversal-indexed decisions.
fn validate_local(local: &BodyLocal) -> Result<(), NodeBindingError> {
    let BodyLocal {
        owner: _,
        kind: _,
        order: _,
        nested: _,
        expressions: _,
        references: _,
        calls: _,
        protocols: _,
        rust: _,
        traits: _,
        c_abi: _,
        mutation: _,
        dictionary: _,
        iterator_sum_items: _,
        diagnostic_slots: _,
        warning_slots: _,
    } = local;
    unsupported::expression_artifacts(&local.expressions)?;
    unsupported::reference_artifacts(&local.references)?;
    unsupported::call_artifacts(&local.calls)?;
    unsupported::protocol_artifacts(&local.protocols)?;
    unsupported::rust_interop_artifacts(&local.rust)?;
    unsupported::trait_artifacts(&local.traits)?;
    unsupported::cabi_interop_artifacts(&local.c_abi)?;
    if !local.iterator_sum_items.is_empty()
        || !local.dictionary.is_empty()
        || !local.diagnostic_slots.is_empty()
        || !local.warning_slots.is_empty()
        || local.order.mutation != 0
        || local.order.dictionary != 0
    {
        return Err(NodeBindingError::Unsupported("pending local decisions"));
    }
    Ok(())
}

/// One conversion path owns all local field primitives, independently of whether settlement has completed.
fn local_fields(positions: &Positions, local: &BodyLocal) -> Result<Fields, NodeBindingError> {
    validate_local(local)?;
    let mut fields = Vec::new();
    project_expressions(positions, local, &mut fields)?;
    project_references(positions, local, &mut fields)?;
    project_calls(positions, local, &mut fields)?;
    for canonical in local.nested.values() {
        fields.push(vec![
            "binding".into(),
            positions.nodes.anchor(span_of(canonical))?.path,
            "declaration".into(),
            positions.identity(canonical)?,
        ]);
    }
    for canonical in local
        .mutation
        .plain_bodies()
        .ok_or(NodeBindingError::Unsupported("mutation facts"))?
    {
        fields.push(vec![
            "effect".into(),
            String::new(),
            positions.identity(canonical)?,
            "checked".into(),
        ]);
    }
    Ok(fields)
}

/// Selected final module refinements for one body's own expression/reference domains. These are distinct from the
/// pending local facts and never include a sibling's payload.
#[derive(Debug, Default, PartialEq)]
struct FinalTypes {
    expressions: BTreeMap<Span, crate::symbols::ResolvedType>,
    bindings: BTreeMap<CanonicalSymbolId, crate::symbols::ResolvedType>,
}

/// Select the same final expression/binding projections as G3's native publication adapter, without its broad copy.
fn final_type_values(checker: &TypeChecker, view: &CheckedBodyView) -> FinalTypes {
    let expressions = view
        .expressions()
        .expr_types
        .keys()
        .filter_map(|span| {
            checker
                .type_info
                .expressions
                .body_ir_expr_types
                .get(span)
                .map(|ty| (*span, ty.clone()))
        })
        .collect();
    let bindings = view
        .references()
        .resolved_write_identities
        .values()
        .chain(view.references().resolved_identities.values())
        .filter_map(|canonical| {
            checker
                .type_info
                .expressions
                .body_ir_binding_types
                .get(canonical)
                .map(|ty| (canonical.clone(), ty.clone()))
        })
        .collect();
    FinalTypes { expressions, bindings }
}

/// Publish the selected finalized empty-list/value types and an explicitly witnessed empty native-call capability.
/// A nonempty native-call field already fails the exhaustive local inventory until its typed codec is supported.
fn project_final_types(
    checker: &TypeChecker,
    view: &CheckedBodyView,
    positions: &Positions,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    let values = final_type_values(checker, view);
    if values.expressions.is_empty() && values.bindings.is_empty() {
        fields.push(vec!["final-type".into(), String::new(), "empty".into(), "true".into()]);
    }
    for (span, ty) in &values.expressions {
        let mut row = vec![
            "final-type".into(),
            positions.nodes.anchor(*span)?.path,
            "expression".into(),
        ];
        type_fields(ty, &mut row)?;
        fields.push(row);
    }
    for (canonical, ty) in &values.bindings {
        let mut row = vec!["final-type".into(), positions.identity(canonical)?, "binding".into()];
        type_fields(ty, &mut row)?;
        fields.push(row);
    }
    fields.push(vec!["native-call".into(), String::new(), "empty".into(), "true".into()]);
    Ok(())
}
