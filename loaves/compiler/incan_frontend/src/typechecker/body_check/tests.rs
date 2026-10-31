//! Complete per-body cold comparisons with the original shared-output checker.

use super::{AnchorFault, BodyLocal, CheckedBodyView, DiagnosticPositions, TypeChecker};
use crate::ast::Span;

impl TypeChecker {
    /// Declaration-owned defaults never become statement-body inputs, and the separate owner keeps the checked
    /// expression even when the body is refused. Constant and static rules are exercised by the cold matrix.
    pub(in crate::typechecker) fn assert_declaration_expression_owners() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model Item:\n    value: int = 5\nclass Other:\n    value: int = 6\nconst seed: int = 3\nstatic total: int = 4\ndef first(value: int = seed + 1) -> int:\n    return value\ndef refused(value: int = 2) -> int:\n    return \"wrong\"\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("expected the invalid return to be refused")?;
        assert_eq!(errors.len(), 1, "nested owners retain each diagnostic once");
        assert_eq!(checker.checked_declarations.len(), 6);
        for declaration in &program.declarations {
            let defaults = match &declaration.node {
                crate::ast::Declaration::Function(function) => function
                    .params
                    .iter()
                    .filter_map(|param| param.node.default.as_ref())
                    .collect::<Vec<_>>(),
                crate::ast::Declaration::Model(model) => model
                    .fields
                    .iter()
                    .filter_map(|field| field.node.default.as_ref())
                    .collect::<Vec<_>>(),
                crate::ast::Declaration::Class(class) => class
                    .fields
                    .iter()
                    .filter_map(|field| field.node.default.as_ref())
                    .collect::<Vec<_>>(),
                _ => continue,
            };
            let owner = checker
                .checked_declarations
                .iter()
                .find(|owner| owner.local.owner == declaration.span)
                .ok_or("missing checked declaration")?;
            for default in defaults {
                assert!(
                    owner.local.expressions.expr_types.contains_key(&default.span),
                    "the declaration retains its checked default even when its statement body is refused"
                );
                assert!(
                    checker.checked_bodies.iter().all(|body| !body
                        .local
                        .expressions
                        .expr_types
                        .contains_key(&default.span)),
                    "a statement output must not own its declaration's default expression"
                );
            }
        }
        for declaration in &program.declarations {
            let crate::ast::Declaration::Function(function) = &declaration.node else {
                continue;
            };
            let statement = checker
                .checked_body(declaration.span)
                .ok_or("missing settled statement view")?;
            let declaration_view = checker
                .checked_declaration(declaration.span)
                .ok_or("missing settled declaration view")?;
            assert_eq!(statement.owner(), declaration.span);
            assert_eq!(statement.is_refused(), function.name == "refused");
            assert!(
                !declaration_view.is_refused(),
                "a body refusal belongs to its statement result"
            );
        }
        for body in &checker.checked_bodies {
            assert!(
                checker.checked_declarations.iter().all(|owner| {
                    owner
                        .local
                        .nested
                        .keys()
                        .all(|span| !body.local.nested.contains_key(span))
                }),
                "nested body declarations have one output owner"
            );
        }
        assert_eq!(
            checker.check_work.callable_defaults, 2,
            "each default expression is checked once"
        );
        assert_eq!(checker.check_work.declaration_local_checks, 6);
        assert_eq!(checker.check_work.declaration_finalizations, 6);
        assert_eq!(checker.check_work.field_defaults, 2);
        assert_eq!(checker.check_work.body_local_checks, 2);
        assert_eq!(checker.check_work.body_finalizations, 2);
        eprintln!(
            "declaration-owners\towners={}\tbodies={}\tdefault_checks={}\tfield_default_checks={}\tdefaults_in_body=0\trefusals={}",
            checker.check_work.declaration_local_checks,
            checker.check_work.body_local_checks,
            checker.check_work.callable_defaults,
            checker.check_work.field_defaults,
            errors.len()
        );
        Ok(())
    }

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
            for local in self.body_enclosing_observations.clone() {
                local.append(self);
            }
            for input in self.body_checks.clone() {
                input.local().clone().append(self);
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
                "def touch(mut items: list[int]) -> None:\n    {body:<24}\ndef first() -> None:\n    items = [1]\n    touch(items)\ndef second() -> None:\n    items = [2]\n    touch(items)\ndef sibling(value: int = 3) -> int:\n    return value\n"
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
        assert_eq!(base.check_work.callable_defaults, 1);
        for (body, expected_finalizations, expected_reuses) in
            [("println(2)", 1, 3), ("touch(items)", 1, 3), ("items.append(2)", 3, 1)]
        {
            let edited = parse(&program_text(body))?;
            let mut candidate = TypeChecker::new();
            candidate.prepare_body_edit_program(&edited);
            let mut positions = DiagnosticPositions::new(&edited, candidate.symbols.declaration_origin());
            // This fixture has no imports, decorators or signature influences. The default-bearing sibling's
            // exact syntax and the complete G1 projection must be equal before its declaration output is reused.
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
                    let declaration_value = base
                        .checked_declarations
                        .iter()
                        .find(|checked| checked.local.owner == declaration.span)
                        .ok_or("missing sibling declaration value")?;
                    candidate
                        .reuse_checked_declaration(std::sync::Arc::clone(declaration_value), &mut positions)
                        .map_err(|fault| format!("{fault:?}"))?;
                    let checked = base
                        .checked_bodies
                        .iter()
                        .find(|checked| checked.local.owner == declaration.span)
                        .ok_or("missing sibling value")?;
                    candidate
                        .reuse_checked_body(std::sync::Arc::clone(checked), &mut positions)
                        .map_err(|fault| format!("{fault:?}"))?;
                }
            }
            let errors = candidate.assert_body_edit_result(
                &edited,
                &base,
                original.declarations[0].span,
                expected_finalizations,
                expected_reuses,
            )?;
            let work = candidate.check_work();
            eprintln!(
                "body-edit\tbody={body}\tlocal_checks={}\tfinalizations={}\treused_views={}\tdeclaration_reuses={}\tdefault_checks={}\tcomponents={}\tmembers={}\terrors={}\tdifferences=0",
                work.body_local_checks,
                work.body_finalizations,
                work.body_view_reuses,
                work.declaration_view_reuses,
                work.callable_defaults,
                work.mutation_components,
                work.mutation_member_visits,
                errors
            );
        }
        Ok(())
    }
    /// Settle the edited records and compare every final fact and diagnostic with an independently checked program.
    fn assert_body_edit_result(
        &mut self,
        edited: &crate::ast::Program,
        base: &Self,
        changed: Span,
        expected_finalizations: usize,
        expected_reuses: usize,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        self.settle_body_checks(edited);
        self.record_program_lowering_facts(edited);
        let result = self.finish_program_check(edited);
        let mut legacy = TypeChecker::new();
        let fresh = legacy.legacy_check_program(edited);
        assert_eq!(result, fresh, "complete ordered diagnostics after edit");
        self.observe_body_local(Span::default());
        legacy.observe_body_local(Span::default());
        assert_eq!(
            self.body_local_observations.last(),
            legacy.body_local_observations.last(),
            "all final fact families after edit"
        );
        assert_eq!(
            self.type_info.semantic_fact_store(&[]).render_snapshot(),
            legacy.type_info.semantic_fact_store(&[]).render_snapshot(),
            "complete semantic facts after edit"
        );
        assert_eq!(
            self.type_info.declarations.nested_declaration_identities,
            legacy.type_info.declarations.nested_declaration_identities,
            "unused lexical declarations survive reuse"
        );
        let work = self.check_work();

        assert_eq!(work.body_local_checks, 1);
        assert_eq!(work.body_finalizations, expected_finalizations);
        assert_eq!(work.body_view_reuses, expected_reuses);
        assert_eq!(work.declaration_view_reuses, 3);
        assert_eq!(work.callable_defaults, 0);
        assert_eq!(
            work.mutation_arguments,
            legacy.check_work().mutation_arguments,
            "cached obligations receive the current traversal binding"
        );
        for previous in base
            .checked_bodies
            .iter()
            .filter(|checked| checked.local.owner != changed)
        {
            let current = self
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
        Ok(result.as_ref().err().map_or(0, Vec::len))
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

/// Stored diagnostic families of one settled owner, compared as values.
fn stored_values(view: &CheckedBodyView) -> impl PartialEq + std::fmt::Debug + '_ {
    (
        &view.diagnostics,
        &view.warnings,
        &view.mutation,
        &view.dictionary,
        &view.open,
    )
}

/// A checked revision: its syntax, the settled checker and its fresh published diagnostics.
struct CheckedRevision {
    source: String,
    program: crate::ast::Program,
    checker: TypeChecker,
    errors: Vec<crate::diagnostics::CompileError>,
}

impl CheckedRevision {
    /// Parse and check one revision through the production entry point.
    fn check(source: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        let errors = checker.check_program(&program).err().ok_or("expected refusals")?;
        let work = checker.check_work();
        assert!(
            work.stored_diagnostics >= errors.len(),
            "every refusal is a stored value"
        );
        assert_eq!(work.detached_anchors, 0, "every stored range is anchored to syntax");
        Ok(Self {
            source: source.to_owned(),
            program,
            checker,
            errors,
        })
    }

    /// Bind stored values of any revision to this revision's syntax.
    fn positions(&self) -> DiagnosticPositions<'_> {
        DiagnosticPositions::new(&self.program, self.checker.symbols.declaration_origin())
    }

    /// The settled view of the named callable's statements.
    fn body(&self, name: &str) -> Result<&CheckedBodyView, Box<dyn std::error::Error>> {
        let declaration = self
            .program
            .declarations
            .iter()
            .find(|declaration| {
                matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == name)
            })
            .ok_or("missing declaration")?;
        Ok(self
            .checker
            .checked_body(declaration.span)
            .ok_or("missing settled body")?)
    }

    /// The `file:line:` prefix of a rendered diagnostic in this revision.
    fn line_of(&self, error: &crate::diagnostics::CompileError) -> Option<usize> {
        let rendered = crate::diagnostics::format_error("case.incn", &self.source, error);
        let (_, after) = rendered.split_once("case.incn:")?;
        after.split(':').next()?.parse().ok()
    }
}

impl TypeChecker {
    /// BE-34 on settled production storage. A comment above a refused body, or above its refused statement, leaves
    /// every stored diagnostic value equal: local refusals, an advisory with a related range, a deferred mutation
    /// decision and a dictionary decision. The earlier revision's stored values render the edited revision's fresh
    /// diagnostics, so the reported line moves. A changed or removed owner refuses the binding instead.
    pub(in crate::typechecker) fn assert_stored_diagnostics_follow_comment_edits()
    -> Result<(), Box<dyn std::error::Error>> {
        let program = |above: &str, inside: &str, value: &str| {
            format!(
                "{above}def refused() -> int:\n{inside}    return {value}\ndef touch(mut items: list[int]) -> None:\n    items.append(2)\ndef caller() -> None:\n    items = [1]\n    touch(items)\ndef dead() -> int:\n    return 1\n    println(\"never\")\n    println(\"again\")\ndef numbers() -> Generator[int]:\n    yield 1\ndef pick(streams: dict[str, Generator[int]], key: str) -> Option[Generator[int]]:\n    return streams.get(key)\n"
            )
        };
        let base = CheckedRevision::check(&program("", "", "\"wrong\""))?;
        let refused = base.body("refused")?;
        let stored = refused.diagnostics.first().ok_or("missing stored refusal")?;
        assert_eq!(base.line_of(&base.errors[0]), Some(2));
        assert!(
            !base.body("caller")?.mutation.is_empty(),
            "the caller stores its mutation decision"
        );
        assert!(
            !base.body("pick")?.dictionary.is_empty(),
            "the lookup stores its dictionary decision"
        );
        assert!(
            !base.body("dead")?.warnings.is_empty(),
            "the dead tail stores its advisory"
        );

        // ---- Comment-only edits keep every stored value ----
        let edits = [
            ("# A comment above the refused body.\n", ""),
            ("", "    # A comment above the refused statement.\n"),
        ];
        for (above, inside) in edits {
            let edited = CheckedRevision::check(&program(above, inside, "\"wrong\""))?;
            for name in ["refused", "touch", "caller", "dead", "numbers", "pick"] {
                assert_eq!(
                    stored_values(base.body(name)?),
                    stored_values(edited.body(name)?),
                    "{name}: stored diagnostic values are equal across a comment-only edit"
                );
            }
            let mut positions = edited.positions();
            let rendered = positions.render(stored).map_err(|fault| format!("{fault:?}"))?;
            assert_eq!(
                rendered, edited.errors[0],
                "the stored value renders the fresh diagnostic"
            );
            assert_eq!(
                edited.line_of(&rendered),
                Some(3),
                "the rendered line moves with the edit"
            );
            for view in edited.checker.checked_bodies.iter() {
                for (_, value) in view.mutation.iter().chain(&view.dictionary) {
                    let decision = positions.render(value).map_err(|fault| format!("{fault:?}"))?;
                    assert!(
                        edited.errors.contains(&decision),
                        "a stored decision renders a fresh refusal"
                    );
                }
                for value in &view.warnings {
                    let advisory = positions.render(value).map_err(|fault| format!("{fault:?}"))?;
                    assert!(
                        edited.checker.warnings.contains(&advisory),
                        "a stored advisory renders fresh"
                    );
                }
            }
            eprintln!(
                "be34-stored\tedit={}\tstored={}\tnode_relative={}\tdetached={}\toriginal_line=2\trendered_line=3\tstored_values_equal=true",
                if above.is_empty() { "statement" } else { "body" },
                edited.checker.check_work().stored_diagnostics,
                edited.checker.check_work().node_relative_anchors,
                edited.checker.check_work().detached_anchors
            );
        }

        // ---- Semantic edits refuse the binding ----
        let changed = CheckedRevision::check(&program("", "", "\"different\""))?;
        assert_ne!(stored_values(refused), stored_values(changed.body("refused")?));
        assert_eq!(
            changed.positions().render(stored).err(),
            Some(AnchorFault::ChangedOwner)
        );
        let renamed = CheckedRevision::check(&program("", "", "\"wrong\"").replacen("def refused", "def moved", 1))?;
        assert_eq!(
            renamed.positions().render(stored).err(),
            Some(AnchorFault::MissingOwner)
        );
        Ok(())
    }
}
