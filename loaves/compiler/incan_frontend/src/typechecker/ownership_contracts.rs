//! Recognition of RFC 164 contracts before their executable ownership stages are enabled.
//!
//! Source parsing and formatting preserve declarations. Every resolved prelude mode and result-source clause is
//! refused before native lowering until checked loan and transfer facts can implement it without repair copies.

use incan_lang::lang::types::ownership::{self, OwnershipMode};
use incan_semantics_core::SymbolOrigin;

mod checked;
mod flow;
mod report;
mod source;

use super::TypeChecker;
use crate::ast::{AssignmentStmt, BindingKind, Expr, Span, Spanned, Type};
use crate::diagnostics::CompileError;
use crate::symbols::{ResolvedType, TypeInfo};

/// One body-owned source analysis. Unsupported projection and malformed internal facts remain distinct from a
/// completed analysis with user-program conflicts; none of these variants alone enables executable lowering.
#[derive(Debug, Clone)]
pub(super) enum InputFlow {
    Checked {
        graph: flow::Graph,
        analysis: flow::Analysis,
    },
    Unavailable(source::Unavailable),
    Invalid {
        graph: flow::Graph,
        fault: flow::InvalidGraph,
    },
}

/// Canonical local annotation facts captured before a later binding can shadow a prelude or alias name.
#[derive(Debug, Clone)]
pub(super) struct LocalMode {
    pub owner: incan_semantics_core::CanonicalSymbolId,
    pub mode: OwnershipMode,
    pub value_type: ResolvedType,
    pub writable: bool,
    pub annotation: Span,
}

impl TypeChecker {
    /// Capture explicit input flow while the checked lexical frame and body-owned expression facts are active.
    /// Statement output owns the result; it is never rediscovered by lowering or published as a valid empty graph.
    pub(super) fn record_explicit_input_flow(&mut self, function: &crate::ast::FunctionDecl, span: Span) {
        let Some(identity) = self.current_body_identity().cloned() else {
            return;
        };
        if !self.type_info.declarations.ownership_contracts.contains_key(&identity)
            && !self
                .type_info
                .protocols
                .explicit_local_modes
                .values()
                .any(|mode| mode.owner == identity)
        {
            return;
        }
        let output = match source::build(self, function, span) {
            Ok(graph) => match flow::analyze(&graph) {
                Ok(analysis) => {
                    self.errors.extend(report::conflicts(&graph, &analysis));
                    InputFlow::Checked { graph, analysis }
                }
                Err(fault) => InputFlow::Invalid { graph, fault },
            },
            Err(reason) => InputFlow::Unavailable(reason),
        };
        self.type_info.protocols.explicit_input_flow.insert(identity, output);
    }

    /// Announce the place-binding migration without changing legacy runtime behavior or inferring a repair clone.
    ///
    /// An annotation does not opt out. Calls, constructed values, computed properties and explicit clone results are
    /// not storage places; reassignment is handled before this new-binding hook.
    pub(super) fn warn_place_binding_migration(&mut self, assignment: &AssignmentStmt) {
        if assignment.binding == BindingKind::Reassign || !self.migration_place(&assignment.value) {
            return;
        }
        let behavior = if assignment.binding == BindingKind::Mutable {
            "a writable borrow; assignment through the local will write through to the source"
        } else {
            "a read-only borrow that retains its source while the local is used"
        };
        self.warnings.push(CompileError::warning(
            format!("Local '{}' binds an existing place; a future ownership release will make this {behavior}", assignment.name),
            assignment.value.span,
        ).with_stable_code("INCAN-T0125")
            .with_note("Runtime behavior is unchanged in this release; a type annotation does not request an independent copy")
            .with_hint("Use an explicit clone if an independent snapshot is intended; otherwise keep the source valid through the local's last use"));
    }

    /// Admit only existing storage roots and known projections, excluding property calls and temporary owners.
    fn migration_place(&self, value: &Spanned<Expr>) -> bool {
        match &value.node {
            Expr::Ident(name) => self.lookup_variable_info_in_scope_chain(name).is_some(),
            Expr::Paren(inner) => self.migration_place(inner),
            Expr::Field(base, field) if self.migration_place(base) => {
                if self
                    .type_info
                    .resolved_identity(value.span)
                    .is_some_and(|identity| identity.kind == incan_semantics_core::SemanticSourceTargetKind::Field)
                {
                    return true;
                }
                let name = match self.type_info.expr_type(base.span) {
                    Some(ResolvedType::Named(name) | ResolvedType::Generic(name, _)) => name,
                    _ => return false,
                };
                match self.lookup_type_info(name) {
                    Some(TypeInfo::Model(model)) => model.fields.contains_key(field),
                    Some(TypeInfo::Class(class)) => class.fields.contains_key(field),
                    _ => false,
                }
            }
            Expr::Index(base, _) if self.migration_place(base) => matches!(self.type_info.expr_type(base.span),
                Some(ResolvedType::Generic(name, _)) if name == "List" || name == "Dict"),
            _ => false,
        }
    }

    /// Resolve a mode only when ordinary lookup selects its canonical builtin declaration.
    pub(super) fn explicit_ownership_mode(&self, name: &str) -> Option<OwnershipMode> {
        let symbol = self
            .symbols
            .lookup(name)
            .or_else(|| ownership::from_str(name).and_then(|mode| self.symbols.lookup(mode.as_str())))?;
        let identity = self.symbols.identity_of(symbol)?;
        if identity.origin != SymbolOrigin::Builtin {
            return None;
        }
        ownership::from_str(&identity.declaration_name)
    }

    /// Keep prelude access contracts from becoming accidental runtime wrapper constructors.
    pub(super) fn refuse_explicit_ownership_constructor(&mut self, name: &str, span: Span) -> bool {
        let Some(mode) = self.explicit_ownership_mode(name) else {
            return false;
        };
        self.errors.push(
            CompileError::type_error(
                format!(
                    "'{}' is an ownership annotation, not a runtime constructor",
                    mode.as_str()
                ),
                span,
            )
            .with_stable_code("INCAN-T0123")
            .with_hint("Write the contract in a type annotation instead of wrapping the value in a call"),
        );
        true
    }

    /// Refuse explicit mode annotations at every nesting depth, including aliases, fields and callable signatures.
    ///
    /// The annotation retains its generic shape; this diagnostic prevents interpreting that shape as a value wrapper
    /// or turning it into a native reference before the contract's executable semantics exist.
    pub(super) fn refuse_unimplemented_ownership_annotation(&mut self, annotation: &Spanned<Type>) {
        match &annotation.node {
            Type::Simple(name) | Type::Generic(name, _) => {
                if let Some(mode) = self.explicit_ownership_mode(name) {
                    let valid_arity = matches!(&annotation.node, Type::Generic(_, arguments) if arguments.len() == 1);
                    let message = if valid_arity {
                        format!(
                            "Explicit ownership contract '{}[T]' is recognized but not executable yet",
                            mode.as_str()
                        )
                    } else {
                        format!(
                            "Ownership contract '{}' requires exactly one type argument",
                            mode.as_str()
                        )
                    };
                    self.errors.push(CompileError::type_error(message, annotation.span)
                        .with_stable_code("INCAN-T0123")
                        .with_note("This compiler refuses the contract rather than compiling it with implicit copy or clone semantics"));
                }
                if let Type::Generic(_, arguments) = &annotation.node {
                    for argument in arguments {
                        self.refuse_unimplemented_ownership_annotation(argument);
                    }
                }
            }
            Type::DottedGeneric(_, arguments) | Type::Tuple(arguments) => {
                for argument in arguments {
                    self.refuse_unimplemented_ownership_annotation(argument);
                }
            }
            Type::Function(parameters, result) => {
                for parameter in parameters {
                    self.refuse_unimplemented_ownership_annotation(parameter);
                }
                self.refuse_unimplemented_ownership_annotation(result);
            }
            Type::Ref(inner) | Type::RefMut(inner) | Type::MutParam(inner) => {
                self.refuse_unimplemented_ownership_annotation(inner);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Project actual checked source so CFG tests cannot bypass canonical root/type lookup with hand-built facts.
    fn source_graph(
        source: &str,
    ) -> Result<Result<flow::Graph, super::source::Unavailable>, Box<dyn std::error::Error>> {
        let checker = checked(source)?;
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let declaration = program.declarations.first().ok_or("missing function")?;
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return Err("expected function".into());
        };
        let projected = super::source::build(&checker, function, declaration.span);
        let identity = checker
            .type_info
            .declarations
            .function_bindings_by_span
            .get(&declaration.span)
            .and_then(|binding| binding.identity.as_ref())
            .ok_or("missing selected source declaration")?;
        let recorded = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get(identity)
            .ok_or("source analysis was not retained with its checked body")?;
        match (recorded, &projected) {
            (InputFlow::Checked { graph, analysis }, Ok(rebuilt)) => {
                assert_eq!(graph.nodes.len(), rebuilt.nodes.len());
                assert_eq!(analysis, &flow::analyze(rebuilt).map_err(|fault| format!("{fault:?}"))?);
            }
            (InputFlow::Unavailable(reason), Err(rebuilt)) => assert_eq!(reason, rebuilt),
            (InputFlow::Invalid { graph, fault }, _) => {
                return Err(format!("invalid body graph ({} nodes): {fault:?}", graph.nodes.len()).into());
            }
            _ => return Err(format!("scoped and rebuilt source flow differ: {recorded:?}\n{projected:?}").into()),
        }
        Ok(projected)
    }

    /// Local modes keep their callable owner through settlement, and an ordinary following function does not
    /// inherit a previous body's ownership profile, including in the legacy cold-oracle comparison route.
    #[test]
    fn local_mode_facts_stay_with_their_callable() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def borrowed() -> int:\n    mut value = 1\n    mut view: BorrowedMut[int] = value\n    view = 2\n    return value\ndef ordinary() -> int:\n    return 3\n";
        for legacy in [false, true] {
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let mut checker = TypeChecker::new();
            checker.legacy_body_checking = legacy;
            let _ = checker.check_program(&program);
            assert_eq!(checker.type_info.protocols.explicit_local_modes.len(), 1);
            assert_eq!(checker.type_info.protocols.explicit_input_flow.len(), 1);
            assert!(
                checker
                    .type_info
                    .protocols
                    .explicit_input_flow
                    .keys()
                    .all(|owner| owner.declaration_name == "borrowed")
            );
        }
        Ok(())
    }

    /// Source branch and back-edge projections preserve consuming uses and distinguish Copy reads from transfers.
    #[test]
    fn source_flow_tracks_inputs_through_branches_and_loops() -> Result<(), Box<dyn std::error::Error>> {
        for (source, conflict) in [
            ("def add(value: Borrowed[int]) -> int:\n    return value + 1\n", false),
            ("def transfer(value: Owned[str]) -> str:\n    return value\n", false),
            ("def copy(value: Borrowed[str]) -> str:\n    return value\n", true),
            (
                "def branch(value: Owned[str], flag: bool) -> str:\n    if flag:\n        taken = value\n    return value\n",
                true,
            ),
            (
                "def repeat(value: Owned[str]) -> None:\n    loop:\n        taken = value\n        continue\n",
                true,
            ),
            (
                "def restore(mut value: Owned[str]) -> str:\n    taken = value\n    value = \"new\"\n    return value\n",
                false,
            ),
            (
                "def local(mut value: Owned[int]) -> int:\n    mut view: BorrowedMut[int] = value\n    view = view + 1\n    return value\n",
                false,
            ),
            (
                "def local_only() -> int:\n    mut value = 1\n    mut view: BorrowedMut[int] = value\n    view = 2\n    return value\n",
                false,
            ),
            (
                "def conflict(mut value: Owned[int]) -> int:\n    let view: Borrowed[int] = value\n    value = 2\n    return view\n",
                true,
            ),
            (
                "def readonly(mut value: Owned[int]) -> int:\n    mut view: Borrowed[int] = value\n    view = 2\n    return value\n",
                true,
            ),
            (
                "def reborrow(mut value: Owned[int]) -> int:\n    mut parent: BorrowedMut[int] = value\n    mut child: BorrowedMut[int] = parent\n    parent = 2\n    return child\n",
                true,
            ),
            (
                "def branch_release(mut value: Owned[int], flag: bool) -> int:\n    let view: Borrowed[int] = value\n    if flag:\n        return view\n    value = 2\n    return value\n",
                false,
            ),
            (
                "def shadow(mut value: Owned[int]) -> int:\n    let view: Borrowed[int] = value\n    Borrowed = 2\n    value = 3\n    return view\n",
                true,
            ),
            (
                "def nested(mut value: BorrowedMut[Customer]) -> int:\n    mut view: BorrowedMut[int] = value.address.count\n    view = 2\n    return value.address.count\nmodel Address:\n    count: int\nmodel Customer:\n    address: Address\n",
                false,
            ),
            (
                "def parent(value: Owned[Customer]) -> int:\n    let view: Borrowed[int] = value.address.count\n    let taken: Owned[Address] = value.address\n    return view\nmodel Address:\n    count: int\nmodel Customer:\n    address: Address\n",
                true,
            ),
            (
                "def sibling(mut value: BorrowedMut[Pair]) -> int:\n    let view: Borrowed[int] = value.left\n    value.right = 2\n    return view\nmodel Pair:\n    left: int\n    right: int\n",
                false,
            ),
            (
                "def overlapping(mut value: BorrowedMut[Pair]) -> int:\n    let view: Borrowed[int] = value.left\n    value.left = 2\n    return view\nmodel Pair:\n    left: int\n    right: int\n",
                true,
            ),
            (
                "def compound(mut value: BorrowedMut[int]) -> int:\n    value += 1\n    return value\n",
                false,
            ),
        ] {
            let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
            let analysis = flow::analyze(&graph).map_err(|error| format!("{source}\n{error:?}"))?;
            assert_eq!(
                !analysis.conflicts.is_empty(),
                conflict,
                "{source}\n{:?}",
                analysis.conflicts
            );
        }
        Ok(())
    }

    /// Calls, escaping results and temporary owners remain unavailable until their exact checked access
    /// and dependency facts exist; a partial graph cannot count as validation of the whole callable.
    #[test]
    fn source_flow_refuses_unprojected_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def show(value: Borrowed[int]) -> None:\n    print(value)\n",
            "def view(value: Borrowed[str]) -> Borrowed[str] from value:\n    return value\n",
            "def local(value: Borrowed[str]) -> None:\n    let view: Reference[str] = value\n",
            "def temporary() -> None:\n    let view: Borrowed[str] = \"temporary\"\n",
        ] {
            assert!(
                source_graph(source)?.is_err(),
                "unsupported dependency admitted: {source}"
            );
        }
        Ok(())
    }

    /// Conflicts are source diagnostics with the actual operation and the dependency explaining its lifetime;
    /// stable secondary anchors must survive independently of message wording or native compilation.
    #[test]
    fn explicit_conflicts_report_origin_and_dependent_use() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def conflicting(mut value: Owned[int]) -> int:\n    let view: Borrowed[int] = value\n    value = 2\n    return view\n";
        let checker = checked(source)?;
        let conflict = checker
            .errors
            .iter()
            .find(|error| error.stable_code() == Some("INCAN-T0126"))
            .ok_or("missing source ownership conflict")?;
        assert_eq!(source.get(conflict.span.start..conflict.span.end), Some("value"));
        let related = conflict.related_spans();
        assert_eq!(related.len(), 2, "{conflict:?}");
        assert_eq!(source.get(related[0].span.start..related[0].span.end), Some("value"));
        assert_eq!(source.get(related[1].span.start..related[1].span.end), Some("view"));
        assert!(related[0].span.start < conflict.span.start);
        assert!(related[1].span.start > conflict.span.start);
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123"))
        );

        let source = "def transfer(value: Owned[str]) -> str:\n    first = value\n    return value\n";
        let checker = checked(source)?;
        let conflict = checker
            .errors
            .iter()
            .find(|error| error.stable_code() == Some("INCAN-T0126"))
            .ok_or("missing moved source conflict")?;
        assert_eq!(source.get(conflict.span.start..conflict.span.end), Some("value"));
        assert_eq!(conflict.related_spans().len(), 1);
        assert!(conflict.related_spans()[0].span.start < conflict.span.start);

        let source = "def readonly(value: Borrowed[int]) -> None:\n    value = 2\n";
        let checker = checked(source)?;
        let conflict = checker
            .errors
            .iter()
            .find(|error| error.stable_code() == Some("INCAN-T0126"))
            .ok_or("missing permission conflict")?;
        assert!(conflict.related_spans().is_empty());
        assert_eq!(conflict.related_declarations().len(), 1);
        assert_eq!(conflict.related_declarations()[0].identity.declaration_name, "value");

        let source = "def branching(mut value: Owned[int], flag: bool) -> int:\n    let view: Borrowed[int] = value\n    if flag:\n        value = 2\n        return view\n    return view\n";
        let checker = checked(source)?;
        let conflict = checker
            .errors
            .iter()
            .find(|error| error.stable_code() == Some("INCAN-T0126"))
            .ok_or("missing branch conflict")?;
        assert_eq!(conflict.related_spans().len(), 2);
        let witness = conflict.related_spans()[1].span;
        assert_eq!(
            witness.start,
            source.find("return view").ok_or("missing branch use")? + 7
        );
        Ok(())
    }

    /// Later callees and overloads expose their exact obligations before any body executes; malformed sources
    /// remain unavailable and are diagnosed by their declaration-owned check, without duplicate source errors.
    #[test]
    fn function_headers_supply_ownership_before_bodies() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def caller() -> None:\n    pass\ndef view(value: Borrowed[Page]) -> Borrowed[str] from value.title:\n    return value.title\ndef view(prefix: str, value: Borrowed[str]) -> Borrowed[str] from value:\n    return value\ndef invalid(value: Borrowed[str]) -> Borrowed[str] from absent:\n    return value\nmodel Page:\n    title: str\n";
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.reset_program_check(&program, false);
        checker.collect_program_check_declarations(&program);
        assert_eq!(checker.check_work.function_declarations, 0);
        let contracts = &checker.type_info.declarations.ownership_contracts;
        assert_eq!(contracts.len(), 2, "{contracts:?}");
        for (index, declaration) in program.declarations[1..3].iter().enumerate() {
            let binding = checker
                .type_info
                .declarations
                .function_bindings_by_span
                .get(&declaration.span)
                .ok_or("missing overload header")?;
            let identity = binding.identity.as_ref().ok_or("missing exact overload identity")?;
            let contract = contracts.get(identity).ok_or("missing overload ownership")?;
            let dependency = contract.result_source.as_ref().ok_or("missing source relationship")?;
            assert_eq!(dependency.slot, crate::ownership_metadata::SourceSlot::Parameter(index));
            assert_eq!(dependency.fields.len(), usize::from(index == 0));
        }
        assert!(!checker.errors.iter().any(|error| error.message.contains("absent")));
        let checked = checked(source)?;
        assert_eq!(
            checked
                .errors
                .iter()
                .filter(|error| error.message.contains("Result source must name"))
                .count(),
            1,
            "{:?}",
            checked.errors,
        );
        Ok(())
    }

    /// Direct input annotations constrain access instead of becoming generic value wrappers. Body typing can use
    /// arithmetic and stored fields, but this does not enable execution or permit a shared input to be mutated.
    #[test]
    fn explicit_inputs_have_value_types_without_granting_execution() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def add(value: Borrowed[int]) -> int:\n    return value + 1\n",
            "type View = Borrowed[int]\ndef add(value: View) -> int:\n    return value + 1\n",
            "model Page:\n    title: str\ndef read(page: Borrowed[Page]) -> int:\n    return len(page.title)\n",
            "def increment(mut value: BorrowedMut[int]) -> None:\n    value = value + 1\n",
            "def owned(value: Owned[int]) -> int:\n    return value + 1\n",
            "def add(Borrowed: int, value: Borrowed[int]) -> int:\n    return value + Borrowed\n",
        ] {
            let checker = checked(source)?;
            assert!(!checker.errors.is_empty(), "execution gate unexpectedly open: {source}");
            assert!(
                checker
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0123")),
                "unexpected value typing failure: {source}\n{:?}",
                checker.errors
            );
        }
        let checker = checked("def overwrite(mut value: Borrowed[int]) -> None:\n    value = 2\n")?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() != Some("INCAN-T0123")),
            "shared input mutation was not refused: {:?}",
            checker.errors
        );
        Ok(())
    }

    /// Check source directly so ownership diagnostics cannot be supplied by a native compiler failure.
    fn checked(source: &str) -> Result<TypeChecker, Box<dyn std::error::Error>> {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        if let Err(errors) = checker.check_program(&program) {
            checker.errors = errors;
        }
        Ok(checker)
    }

    /// A source declaration shadowing a prelude spelling remains an ordinary nominal type.
    #[test]
    fn shadowed_mode_is_not_an_ownership_contract() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "model Borrowed[T]:\n    value: T\ndef identity(value: Borrowed[int]) -> Borrowed[int]:\n    return value\n",
        )?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        Ok(())
    }

    /// Every explicit form is refused before executable storage can be selected, also when nested.
    #[test]
    fn modes_are_refused_at_every_annotation_depth() -> Result<(), Box<dyn std::error::Error>> {
        for mode in ["Borrowed", "BorrowedMut", "Owned", "Reference"] {
            let checker = checked(&format!("def read(value: Option[{mode}[str]]) -> None:\n    pass\n"))?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                "{mode}: {:?}",
                checker.errors
            );
        }
        Ok(())
    }

    /// Even a single-source signature needs the explicit relationship, and globals cannot fulfill it.
    #[test]
    fn borrowed_result_source_is_mandatory_and_local_to_signature() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def view(value: Borrowed[str]) -> Borrowed[str]:\n    return value\n",
            "def view(value: Borrowed[str]) -> Result[Borrowed[str], str]:\n    return Ok(value)\n",
            "def view(value: Borrowed[str]) -> FrozenList[Borrowed[str]]:\n    return []\n",
            "const other: str = \"outside\"\ndef view(value: str) -> str from other:\n    return value\n",
        ] {
            let checker = checked(source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0124")),
                "{:?}",
                checker.errors
            );
        }
        Ok(())
    }

    /// A valid written source is still refused rather than authorizing old copied-result behavior.
    #[test]
    fn source_relationship_does_not_enable_execution() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked("def view(value: str) -> str from value:\n    return value\n")?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "{:?}",
            checker.errors
        );
        Ok(())
    }

    /// The known local-copy migration warns without rejecting or changing the program.
    #[test]
    fn mutable_stored_string_copy_warns() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "model Page:\n    title: str\ndef main() -> None:\n    page = Page(title=\"old\")\n    mut title = page.title\n    title = \"new\"\n",
        )?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        assert!(
            checker
                .warnings
                .iter()
                .any(|warning| warning.message.contains("future ownership release")),
            "{:?}",
            checker.warnings
        );
        Ok(())
    }
    /// Compatible annotations, scalar fields and read-only locals do not accidentally request independent copies.
    #[test]
    fn place_migration_includes_annotations_scalars_and_readonly() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "model Page:\n    count: int\ndef main() -> None:\n    page = Page(count=1)\n    mut count: int = page.count\n    let saved = page.count\n    copy = int(2)\n    fresh = Page(count=2)\n",
        )?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        let warnings: Vec<_> = checker
            .warnings
            .iter()
            .filter(|warning| warning.stable_code() == Some("INCAN-T0125"))
            .collect();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        Ok(())
    }

    /// Abstract and concrete receiver sources share checked slots; concrete field paths retain their declaration.
    #[test]
    fn receiver_contracts_cover_abstract_and_concrete_methods() -> Result<(), Box<dyn std::error::Error>> {
        for (source, fields) in [
            ("pub trait Reader:\n    def view(self) -> Borrowed[str] from self\n", 0),
            (
                "pub class Page:\n    title: str\n    def view(self) -> Borrowed[str] from self.title:\n        return self.title\n",
                1,
            ),
        ] {
            let checker = checked(source)?;
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let exports = crate::library_exports::collect_checked_public_exports(&program, &checker);
            let methods = match &exports.first().ok_or("missing public owner")?.kind {
                crate::library_exports::CheckedExportKind::Class(owner) => &owner.methods,
                crate::library_exports::CheckedExportKind::Trait(owner) => &owner.methods,
                _ => return Err("wrong owner kind".into()),
            };
            assert!(methods.first().ok_or("missing public method")?.ownership.is_some());
            let contract = checker
                .type_info
                .declarations
                .ownership_contracts
                .values()
                .next()
                .ok_or("missing method contract")?;
            let dependency = contract.result_source.as_ref().ok_or("missing receiver source")?;
            assert_eq!(dependency.slot, crate::ownership_metadata::SourceSlot::Receiver);
            assert_eq!(dependency.fields.len(), fields);
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123"))
            );
        }
        Ok(())
    }

    /// Annotation constructors and aliases cannot silently opt into ordinary generic value behavior.
    #[test]
    fn modes_are_not_wrappers_or_alias_escape_hatches() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def main() -> None:\n    value = Owned(1)\n",
            "type View = Borrowed[str]\ndef view(value: View) -> None:\n    pass\n",
            "def read(value: Owned[int, str]) -> None:\n    pass\n",
        ] {
            let checker = checked(source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                "{:?}",
                checker.errors
            );
        }
        Ok(())
    }
    /// Source relationships use slots and canonical fields, including substituted nested generic owners.
    #[test]
    fn checked_source_retains_nested_field_identities() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "model Address:\n    street: str\nmodel Customer[T]:\n    address: T\ndef view(customer: Borrowed[Customer[Address]]) -> Borrowed[str] from customer.address.street:\n    return customer.address.street\n",
        )?;
        let contract = checker
            .type_info
            .declarations
            .ownership_contracts
            .values()
            .next()
            .ok_or("missing ownership contract")?;
        let source = contract.result_source.as_ref().ok_or("missing checked source")?;
        assert_eq!(source.slot, crate::ownership_metadata::SourceSlot::Parameter(0));
        assert_eq!(
            source
                .fields
                .iter()
                .map(|field| field.declaration_name.as_str())
                .collect::<Vec<_>>(),
            ["address", "street"]
        );
        assert!(
            source
                .fields
                .iter()
                .all(|field| field.kind == incan_semantics_core::SemanticSourceTargetKind::Field)
        );
        assert_eq!(contract.modes.len(), 2);
        Ok(())
    }

    /// An alias-expanded borrowed result still requires a source, and impossible fields have source diagnostics.
    #[test]
    fn checked_alias_and_invalid_field_contracts() -> Result<(), Box<dyn std::error::Error>> {
        let missing =
            checked("type View[T] = Borrowed[T]\ndef view(value: View[str]) -> View[str]:\n    return value\n")?;
        assert!(
            missing
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0124"))
        );
        let invalid = checked(
            "model Page:\n    title: str\ndef view(page: Borrowed[Page]) -> Borrowed[str] from page.absent:\n    return \"\"\n",
        )?;
        assert!(
            invalid
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0124"))
        );
        assert!(invalid.type_info.declarations.ownership_contracts.is_empty());
        Ok(())
    }
}
