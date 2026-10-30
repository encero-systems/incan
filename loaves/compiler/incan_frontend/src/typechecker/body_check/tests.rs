//! Complete per-body cold comparisons with the original shared-output checker.

use super::{BodyLocal, TypeChecker};
use crate::ast::Span;

impl TypeChecker {
    /// Capture all local fact families and ordered pending decisions after a body. These verification snapshots never
    /// become producer inputs; only unordered Debug containers are sorted and every ordered element is retained.
    pub(in crate::typechecker) fn observe_body_local(&mut self, span: Span) {
        // Reconstruct only the legacy observation container in this test adapter. It never becomes an input to
        // checking or settlement and performs no semantic operations.
        if !self.legacy_body_checking && !self.body_checks.is_empty() {
            let enclosing = BodyLocal::take(self);
            let nested = self.type_info.declarations.nested_declaration_identities.clone();
            let errors = self.errors.clone();
            let warnings = self.warnings.clone();
            enclosing.clone().install(self);
            for input in self.body_checks.clone() {
                let mut local = input.local().clone();
                local.diagnostics.clear();
                local.warnings.clear();
                local.append(self);
            }
            self.capture_body_observation(span);
            enclosing.install(self);
            self.type_info.declarations.nested_declaration_identities = nested;
            self.errors = errors;
            self.warnings = warnings;
            return;
        }
        self.capture_body_observation(span);
    }

    /// Capture exact checked containers without changing the checker or filtering any field.
    fn capture_body_observation(&mut self, span: Span) {
        let value = format!(
            "{:?}",
            (
                &self.type_info.expressions,
                &self.type_info.references,
                &self.type_info.calls,
                &self.type_info.protocols,
                &self.type_info.rust,
                &self.type_info.traits,
                &self.type_info.c_abi,
                (
                    &self.type_info.body_ir_iterator_sum_items,
                    &self.type_info.body_ir_newtypes,
                ),
                &self.mut_params,
                &self.pending_uncopyable_dict_lookups,
                &self.errors,
                &self.warnings,
            )
        );
        self.body_local_observations.push((span, canonical_debug(&value)));
    }
}

/// Sort unordered Debug containers while preserving variants, sequences, scalars and quoted bytes. This exact test
/// comparator is never used for production identity, storage or semantic projection.
fn canonical_debug(value: &str) -> String {
    debug_container(&mut value.chars().peekable(), None, false)
}

/// Consume one delimited Debug container, sorting maps, sets and named fields while retaining every payload value.
fn debug_container(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, close: Option<char>, sort: bool) -> String {
    let mut parts = Vec::new();
    let mut part = String::new();
    while let Some(character) = chars.next() {
        match character {
            '"' | '\'' => {
                part.push(character);
                while let Some(quoted) = chars.next() {
                    part.push(quoted);
                    if quoted == '\\' {
                        if let Some(escaped) = chars.next() {
                            part.push(escaped);
                        }
                    } else if quoted == character {
                        break;
                    }
                }
            }
            '{' | '[' | '(' => {
                let ending = match character {
                    '{' => '}',
                    '[' => ']',
                    _ => ')',
                };
                part.push(character);
                part.push_str(&debug_container(chars, Some(ending), character == '{'));
                part.push(ending);
            }
            ',' if close.is_some() => parts.push(std::mem::take(&mut part).trim().to_owned()),
            ending if Some(ending) == close => break,
            _ => part.push(character),
        }
    }
    if !part.is_empty() {
        parts.push(part.trim().to_owned());
    }
    if sort {
        parts.sort();
    }
    parts.join(",")
}

impl TypeChecker {
    /// Extend the existing cold oracle with edit reuse: applicability is proved from exact unchanged ASTs and G1
    /// declaration projections at fixed coordinates. The harness calls the production local/finalization adapters;
    /// it is not an incremental engine, a persistent cache, or proof of position remapping.
    pub(in crate::typechecker) fn assert_body_edit_reuse() -> Result<(), Box<dyn std::error::Error>> {
        let program_text = |body: &str| {
            format!(
                "def touch(mut items: list[int]) -> None:\n    {body:<24}\ndef first() -> None:\n    items = [1]\n    touch(items)\ndef second() -> None:\n    items = [2]\n    touch(items)\ndef sibling() -> int:\n    return 3\n"
            )
        };
        let parse = |source: &str| -> Result<crate::ast::Program, Box<dyn std::error::Error>> {
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            Ok(crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?)
        };
        let original = parse(&program_text("println(1)"))?;
        let mut envelope = TypeChecker::new();
        envelope.prepare_body_edit_program(&original);
        let environment = canonical_debug(&format!(
            "{:?}",
            (
                &envelope.type_info.declarations,
                envelope.symbols.all_symbols(),
                &envelope.type_aliases
            )
        ));
        let mut base = TypeChecker::new();
        assert!(base.check_program(&original).is_ok());
        assert_eq!(base.checked_bodies.len(), 4);
        for (body, expected_finalizations, expected_reuses) in
            [("println(2)", 1, 3), ("touch(items)", 1, 3), ("items.append(2)", 3, 1)]
        {
            let edited = parse(&program_text(body))?;
            let mut candidate = TypeChecker::new();
            candidate.prepare_body_edit_program(&edited);
            // This fixture has no imports, decorators, defaults or signature influences. Its complete G1
            // declaration projection must still be byte-equal before a sibling is admitted for reuse.
            assert_eq!(
                canonical_debug(&format!(
                    "{:?}",
                    (
                        &candidate.type_info.declarations,
                        candidate.symbols.all_symbols(),
                        &candidate.type_aliases
                    )
                )),
                environment
            );
            candidate.validate_source_type_names = true;
            for (index, declaration) in edited.declarations.iter().enumerate() {
                if index == 0 {
                    candidate.check_environment_declaration(declaration);
                } else {
                    assert_eq!(
                        declaration, &original.declarations[index],
                        "exact local syntax/coordinate precondition"
                    );
                    let checked = base
                        .checked_bodies
                        .iter()
                        .find(|checked| checked.local.owner == declaration.span)
                        .ok_or("missing sibling value")?;
                    candidate.reuse_checked_body(std::sync::Arc::clone(checked));
                }
            }
            candidate.settle_body_checks();
            candidate.record_program_lowering_facts(&edited);
            let result = candidate.finish_program_check(&edited);
            let mut legacy = TypeChecker::new();
            let fresh = legacy.legacy_check_program(&edited);
            assert_eq!(result, fresh, "complete ordered diagnostics after edit");
            candidate.observe_body_local(Span::default());
            legacy.observe_body_local(Span::default());
            assert_eq!(
                candidate.body_local_observations.last(),
                legacy.body_local_observations.last(),
                "all final fact families after edit"
            );
            assert_eq!(
                candidate.type_info.semantic_fact_store(&[]).render_snapshot(),
                legacy.type_info.semantic_fact_store(&[]).render_snapshot(),
                "complete semantic facts after edit"
            );
            assert_eq!(
                candidate.type_info.declarations.nested_declaration_identities,
                legacy.type_info.declarations.nested_declaration_identities,
                "unused lexical declarations survive reuse"
            );
            let work = candidate.check_work();

            assert_eq!(work.body_local_checks, 1);
            assert_eq!(work.body_finalizations, expected_finalizations);
            assert_eq!(work.body_view_reuses, expected_reuses);
            assert_eq!(work.callable_defaults, 0);
            assert_eq!(
                work.mutation_arguments,
                legacy.check_work().mutation_arguments,
                "cached obligations receive the current traversal binding"
            );
            for previous in base
                .checked_bodies
                .iter()
                .filter(|checked| checked.local.owner != original.declarations[0].span)
            {
                let current = candidate
                    .checked_bodies
                    .iter()
                    .find(|checked| checked.local.owner == previous.local.owner)
                    .ok_or("missing final sibling")?;
                let subscribed = !previous.effect_reads.is_empty();
                assert_eq!(
                    std::sync::Arc::ptr_eq(previous, current),
                    expected_reuses == 3 || !subscribed
                );
                assert!(
                    std::sync::Arc::ptr_eq(&previous.local, &current.local),
                    "caller local facts are reused even when its effect decision changes"
                );
            }
            eprintln!(
                "body-edit\tbody={body}\tlocal_checks={}\tfinalizations={}\treused_views={}\tcomponents={}\tmembers={}\terrors={}\tdifferences=0",
                work.body_local_checks,
                work.body_finalizations,
                work.body_view_reuses,
                work.mutation_components,
                work.mutation_member_visits,
                result.as_ref().err().map_or(0, Vec::len)
            );
        }
        Ok(())
    }
    /// Prepare exactly the production G1 envelope before a test driver admits any reused body.
    fn prepare_body_edit_program(&mut self, program: &crate::ast::Program) {
        self.reset_program_check(program, false);
        self.collect_program_check_declarations(program);
        self.record_program_header_facts();
        self.record_source_declaration_identities(program);
        self.visibility_point = super::super::VisibilityPoint::Collected;
        self.prepare_program_body_facts();
    }
}
