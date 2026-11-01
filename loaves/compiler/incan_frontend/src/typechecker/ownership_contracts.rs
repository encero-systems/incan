//! Canonical RFC 164 contracts, source access checks and staged executable-body admission.
//!
//! A direct input annotation is admitted only with its complete supported body proof. Returned dependencies,
//! deferred boundaries and unsupported storage shapes remain refused rather than acquiring repair copies.

use incan_lang::lang::types::ownership::{self, OwnershipMode};
use incan_semantics_core::SymbolOrigin;

mod admission;
mod checked;
pub(super) mod flow;
pub(super) mod projection;
mod report;
mod source;

pub(super) use admission::direct_function_boundary;
pub(super) use source::{CallableSource, Unavailable};

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

/// One successfully checked call retains the exact contract owner and instantiated parameter values. A call in
/// declaration/default evaluation has no callable body owner, but still cannot cross a record profile unaware of it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct InputCall {
    pub owner: Option<incan_semantics_core::CanonicalSymbolId>,
    pub callee: incan_semantics_core::CanonicalSymbolId,
    pub parameters: Vec<crate::symbols::CallableParam>,
    pub receiver: Option<crate::ast::Receiver>,
}

impl TypeChecker {
    /// Capture explicit input flow while the checked lexical frame and body-owned expression facts are active.
    /// Statement output owns the result; it is never rediscovered by lowering or published as a valid empty graph.
    pub(super) fn record_explicit_input_flow(&mut self, callable: CallableSource<'_>, span: Span) {
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
            && !self
                .type_info
                .protocols
                .explicit_input_calls
                .values()
                .any(|call| call.owner.as_ref() == Some(&identity))
        {
            return;
        }
        let output = match source::build(self, callable, span, &identity) {
            Ok(graph) => match flow::analyze(&graph) {
                Ok(analysis) => {
                    self.errors.extend(report::conflicts(&graph, &analysis));
                    InputFlow::Checked { graph, analysis }
                }
                Err(fault) => InputFlow::Invalid { graph, fault },
            },
            Err(reason) => InputFlow::Unavailable(reason),
        };
        self.type_info
            .protocols
            .explicit_input_flow
            .insert(identity.clone(), output);
        if let CallableSource::Function(function) = callable
            && let Err(reason) = self.type_info.direct_input_admission(&identity, function)
        {
            self.errors.push(CompileError::type_error(
                format!("Explicit ownership body cannot execute yet: {reason}"), span,
            ).with_stable_code("INCAN-T0123")
                .with_note("The entire owning body must retain supported access, storage and cleanup facts; a valid annotation alone is insufficient"));
        }
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
        let identity = checker
            .type_info
            .declarations
            .function_bindings_by_span
            .get(&declaration.span)
            .and_then(|binding| binding.identity.as_ref())
            .ok_or("missing selected source declaration")?;
        let projected = super::source::build(&checker, CallableSource::Function(function), declaration.span, identity);
        let recorded = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get(identity)
            .ok_or_else(|| format!("source analysis was not retained with its checked body\n{source}\nerrors: {:?}\ncalls: {:?}\ncontracts: {:?}",
                checker.errors, checker.type_info.protocols.explicit_input_calls, checker.type_info.declarations.ownership_contracts))?;
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

    /// A builtin collection-length read observes a transferred owner just as a primitive read does. Literal
    /// construction cannot hide that transfer, and source-defined length hooks retain their separate refusal.
    #[test]
    fn collection_length_observes_transferred_storage() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def consume(values: Owned[list[int]]) -> int:\n    return len(values)\ndef main() -> None:\n    values = [1, 2, 3]\n    consume(values)\n    assert len(values) == 3\n";
        let checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{:?}",
            checker.errors
        );
        for source in [
            "def collect(value: Borrowed[str]) -> None:\n    values = [value]\n",
            "def collect(values: Borrowed[list[int]]) -> None:\n    copied = [*values]\n",
        ] {
            let checker = checked(source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                "unsupported collection admitted: {source}"
            );
        }
        Ok(())
    }

    /// Owned local destinations retain their exact initializer event; a later read of the same place cannot stand
    /// in for that event, and a second destination cannot acquire the first destination's initializer authority.
    #[test]
    fn owned_local_initializers_retain_exact_source_events() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def inspect(value: Owned[int]) -> int:\n    let held: Owned[int] = (value)\n    assert value == 1\n    return held\n";
        let graph = source_graph(source)?.map_err(|error| format!("{error:?}"))?;
        assert_eq!(graph.owned_locals.len(), 1);
        assert!(
            flow::analyze(&graph)
                .map_err(|error| format!("{error:?}"))?
                .conflicts
                .is_empty()
        );
        let (destination, initializer) = graph
            .owned_locals
            .first_key_value()
            .ok_or("missing owned initializer")?;
        assert_eq!(destination.declaration_name, "held");
        let stored = initializer.source.as_ref().ok_or("missing stored source")?;
        let index = initializer.access_node.ok_or("missing initializer event")?;
        let other = graph.nodes.iter().enumerate().find_map(|(candidate, node)| {
            (candidate != index && matches!(&node.event, flow::Event::Access { place, access: flow::Access::Read, .. } if place == stored)).then_some(candidate)
        }).ok_or("missing later read of the same place")?;
        let mut wrong = graph.clone();
        wrong
            .owned_locals
            .get_mut(destination)
            .ok_or("missing destination")?
            .access_node = Some(other);
        assert!(flow::analyze(&wrong).is_err());
        let mut absent = graph.clone();
        absent
            .owned_locals
            .get_mut(destination)
            .ok_or("missing destination")?
            .access_node = None;
        assert!(flow::analyze(&absent).is_err());
        let mut outside = graph.clone();
        outside
            .owned_locals
            .get_mut(destination)
            .ok_or("missing destination")?
            .access_node = Some(graph.nodes.len());
        assert!(flow::analyze(&outside).is_err());
        let mut missing_owner = graph.clone();
        missing_owner
            .permissions
            .remove(&flow::Root::Binding(destination.clone()));
        assert!(flow::analyze(&missing_owner).is_err());
        Ok(())
    }

    /// Registry-selected primitive observers retain reads after transfer; user functions and nominal hooks do not
    /// acquire the builtin's access contract merely by being named print or len.
    #[test]
    fn primitive_observers_keep_checked_reads() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "def inspect(value: Borrowed[str]) -> None:\n    print(value)\n    assert len(value) > 0, \"empty\"\n";
        let graph = source_graph(source)?.map_err(|error| format!("{error:?}"))?;
        assert!(
            flow::analyze(&graph)
                .map_err(|error| format!("{error:?}"))?
                .conflicts
                .is_empty()
        );
        let source = "def inspect(value: Owned[str]) -> None:\n    consume(value)\n    assert len(value) > 0\ndef consume(value: Owned[str]) -> None:\n    pass\n";
        let graph = source_graph(source)?.map_err(|error| format!("{error:?}"))?;
        assert!(
            !flow::analyze(&graph)
                .map_err(|error| format!("{error:?}"))?
                .conflicts
                .is_empty()
        );
        let checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126"))
        );
        for source in [
            "def inspect(value: Borrowed[str]) -> None:\n    user_print(value)\ndef user_print(value: str) -> None:\n    pass\n",
            "def inspect(value: Borrowed[Measure]) -> int:\n    return len(value)\nclass Measure:\n    def __len__(self) -> int:\n        return 1\n",
        ] {
            assert!(
                source_graph(source)?.is_err(),
                "unprojected observer admitted: {source}"
            );
        }
        let shadowed = checked(
            "def inspect(value: Borrowed[str]) -> None:\n    print(value)\ndef print(value: str) -> None:\n    pass\n",
        )?;
        assert!(
            shadowed
                .errors
                .iter()
                .any(|error| error.stable_code() != Some("INCAN-T0123")),
            "protected builtin shadowing must remain rejected"
        );
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
            "def show(value: Borrowed[int]) -> int:\n    return abs(value)\n",
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

    /// Transfers follow actual Copy evidence, including derives and composite carriers. An exclusive reference
    /// never becomes Copy merely because the legacy caller-adaptation helper groups all reference types together.
    #[test]
    fn explicit_transfer_uses_copy_evidence() -> Result<(), Box<dyn std::error::Error>> {
        for (source, conflict) in [
            (
                "def exclusive(value: Owned[&mut int]) -> &mut int:\n    first = value\n    return value\n",
                true,
            ),
            (
                "def shared(value: Owned[&int]) -> &int:\n    first = value\n    return value\n",
                false,
            ),
            (
                "def derived(value: Owned[Point]) -> Point:\n    first = value\n    return value\n@derive(Copy)\nmodel Point:\n    x: int\n",
                false,
            ),
            (
                "def optional(value: Owned[Option[int]]) -> Option[int]:\n    first = value\n    return value\n",
                false,
            ),
        ] {
            let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
            let analysis = flow::analyze(&graph).map_err(|error| format!("{source}\n{error:?}"))?;
            assert_eq!(!analysis.conflicts.is_empty(), conflict, "{source}\n{analysis:?}");
        }
        Ok(())
    }

    /// Canonical destinations retain the exact reborrow chain; lowering must not rediscover aliases by name.
    #[test]
    fn local_loan_destinations_keep_checked_sources() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def read(mut value: Owned[int]) -> int:\n    mut parent: BorrowedMut[int] = value\n    let child: Borrowed[int] = parent\n    return child\n";
        let mut graph = source_graph(source)?.map_err(|error| format!("{error:?}"))?;
        let parent = graph
            .local_loans
            .iter()
            .find(|(local, _)| local.declaration_name == "parent")
            .map(|(_, loan)| *loan)
            .ok_or("missing parent destination")?;
        let (child, child_loan) = graph
            .local_loans
            .iter()
            .find(|(local, _)| local.declaration_name == "child")
            .map(|(local, loan)| (local.clone(), *loan))
            .ok_or("missing child destination")?;
        assert_eq!(graph.local_loans.len(), 2);
        assert_eq!(graph.loans[child_loan.0].parent, Some(parent));
        assert!(
            flow::analyze(&graph)
                .map_err(|error| format!("{error:?}"))?
                .conflicts
                .is_empty()
        );
        let mut dual = graph.clone();
        dual.permissions.insert(
            flow::Root::Binding(child.clone()),
            flow::Permission {
                exclusive: false,
                writable: false,
                transferable: true,
            },
        );
        assert!(flow::analyze(&dual).is_err());
        let mut duplicate = graph.clone();
        let mut other = child.clone();
        other.declaration_name = "other".into();
        duplicate.local_loans.insert(other, child_loan);
        assert!(flow::analyze(&duplicate).is_err());
        graph.local_loans.insert(child.clone(), flow::LoanId(graph.loans.len()));
        assert!(flow::analyze(&graph).is_err());
        graph.local_loans.insert(child.clone(), child_loan);
        let mut invalid = child;
        invalid.kind = incan_semantics_core::SemanticSourceTargetKind::Parameter;
        graph.local_loans.insert(invalid, child_loan);
        assert!(flow::analyze(&graph).is_err());
        Ok(())
    }

    /// A selected method closes Self in its ownership contract using the same receiver as its parameter types.
    #[test]
    fn method_input_payload_retains_checked_self_substitution() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "class Reader:\n    def view(self, other: Borrowed[Self]) -> None:\n        pass\ndef run() -> None:\n    reader = Reader()\n    reader.view(reader)\n",
        )?;
        assert!(
            !checker.errors.is_empty(),
            "explicit execution gate unexpectedly opened"
        );
        assert!(
            checker
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{:?}",
            checker.errors
        );
        assert_eq!(checker.type_info.protocols.explicit_input_calls.len(), 1);
        Ok(())
    }

    /// Methods retain their own receiver and parameter identities; local loans must not disappear merely because
    /// their callable lives inside a model rather than at module scope.
    #[test]
    fn method_flow_preserves_receiver_permissions() -> Result<(), Box<dyn std::error::Error>> {
        for (source, conflict) in [
            (
                "model Counter:\n    value: int\n    def change(mut self, amount: Borrowed[int]) -> int:\n        mut view: BorrowedMut[int] = self.value\n        view += amount\n        return self.value\n",
                false,
            ),
            (
                "model Counter:\n    value: int\n    def change(self, amount: Borrowed[int]) -> int:\n        mut view: BorrowedMut[int] = self.value\n        view += amount\n        return self.value\n",
                true,
            ),
            (
                "model Counter:\n    value: int\n    def read(self) -> int:\n        let view: Borrowed[int] = self.value\n        return view\n",
                false,
            ),
        ] {
            let checker = checked(source)?;
            assert_eq!(checker.type_info.protocols.explicit_input_flow.len(), 1, "{source}");
            let output = checker
                .type_info
                .protocols
                .explicit_input_flow
                .values()
                .next()
                .ok_or("missing method flow")?;
            let InputFlow::Checked { analysis, .. } = output else {
                return Err(format!("method flow unavailable: {output:?}\n{source}").into());
            };
            assert_eq!(!analysis.conflicts.is_empty(), conflict, "{source}\n{analysis:?}");
            assert_eq!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0126")),
                conflict
            );
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123"))
            );
        }
        let checker = checked(
            "model First:\n    value: int\n    def read(self) -> int:\n        let view: Borrowed[int] = self.value\n        return view\nmodel Second:\n    value: int\n    def read(self) -> int:\n        let view: Borrowed[int] = self.value\n        return view\n",
        )?;
        assert_eq!(checker.type_info.protocols.explicit_input_flow.len(), 2);
        let mut receivers = std::collections::BTreeSet::new();
        for output in checker.type_info.protocols.explicit_input_flow.values() {
            let InputFlow::Checked { graph, analysis } = output else {
                return Err(format!("missing distinct owner flow: {output:?}").into());
            };
            assert!(analysis.conflicts.is_empty());
            receivers.extend(
                graph
                    .permissions
                    .keys()
                    .filter(|root| matches!(root, super::flow::Root::Binding(identity) if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Receiver)),
            );
        }
        assert_eq!(
            receivers.len(),
            2,
            "same-spelled methods must retain distinct receivers"
        );
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

    /// Direct input annotations constrain values and admit only the supported body profile. Alias declarations
    /// retain their broader stage refusal, and shared access never permits source mutation.
    #[test]
    fn explicit_inputs_have_checked_value_and_body_profiles() -> Result<(), Box<dyn std::error::Error>> {
        for (source, admitted) in [
            ("def add(value: Borrowed[int]) -> int:\n    return value + 1\n", true),
            (
                "type View = Borrowed[int]\ndef add(value: View) -> int:\n    return value + 1\n",
                false,
            ),
            (
                "model Page:\n    title: str\ndef read(page: Borrowed[Page]) -> int:\n    return len(page.title)\n",
                true,
            ),
            (
                "def increment(mut value: BorrowedMut[int]) -> None:\n    value = value + 1\n",
                true,
            ),
            ("def owned(value: Owned[int]) -> int:\n    return value + 1\n", true),
            (
                "def add(Borrowed: int, value: Borrowed[int]) -> int:\n    return value + Borrowed\n",
                true,
            ),
        ] {
            let checker = checked(source)?;
            assert_eq!(checker.errors.is_empty(), admitted, "{source}\n{:?}", checker.errors);
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

    /// Argument type compatibility sees the contracted value type while retaining the selected declaration's
    /// ownership request. Different overload slots cannot borrow another candidate's mode or generic binders.
    #[test]
    fn explicit_call_arguments_use_selected_value_types() -> Result<(), Box<dyn std::error::Error>> {
        for (source, admitted) in [
            (
                "def use(value: int) -> int:\n    return read(value)\ndef read(value: Borrowed[int]) -> int:\n    return value\n",
                true,
            ),
            (
                "def use(value: int) -> int:\n    return read(value=value)\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef read(prefix: str, value: Borrowed[int]) -> int:\n    return value\n",
                true,
            ),
            (
                "def use(value: str) -> str:\n    return take(value)\ndef take[T](value: Owned[T]) -> T:\n    return value\n",
                false,
            ),
            (
                "type View = Borrowed[int]\ndef use(value: int) -> int:\n    return read(value)\ndef read(value: View) -> int:\n    return value\n",
                false,
            ),
        ] {
            let checker = checked(source)?;
            assert_eq!(checker.errors.is_empty(), admitted, "{source}\n{:?}", checker.errors);
            assert!(
                checker
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0123")),
                "{source}\n{:?}",
                checker.errors
            );
        }
        let checker = checked(
            "def use(value: int) -> int:\n    return read(value)\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef read(value: Owned[int]) -> int:\n    return value\n",
        )?;
        assert!(checker.errors.iter().any(|error| error.message.contains("ambiguous")));
        assert!(
            checker.type_info.protocols.explicit_input_calls.is_empty(),
            "failed overload selection must discard candidate ownership facts"
        );
        Ok(())
    }

    /// A selected overload retains its own instantiated parameter slots without rewriting either declaration.
    /// The ordinary caller owns its checked access profile even without an explicit input annotation of its own.
    #[test]
    fn explicit_call_facts_preserve_canonical_slots() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "def use(value: str) -> str:\n    return read(1, value)\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef read(prefix: int, value: Owned[str]) -> str:\n    return value\n",
        )?;
        assert!(
            checker
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{:?}",
            checker.errors
        );
        assert_eq!(checker.type_info.protocols.explicit_input_calls.len(), 1);
        let call = checker
            .type_info
            .protocols
            .explicit_input_calls
            .values()
            .next()
            .ok_or("missing explicit call")?;
        assert_eq!(call.parameters.len(), 2);
        assert_eq!(call.parameters[1].ty, ResolvedType::Str);
        let contract = checker
            .type_info
            .declarations
            .ownership_contracts
            .get(&call.callee)
            .ok_or("selected contract missing")?;
        assert_eq!(contract.modes.len(), 1);
        assert_eq!(contract.modes[0].component.parameter, Some(1));
        assert_eq!(contract.modes[0].mode, OwnershipMode::Owned);
        let declaration = checker
            .type_info
            .declarations
            .function_bindings_by_span
            .get(&call.callee.declaration_span)
            .ok_or("selected header missing")?;
        assert!(
            matches!(declaration.params[1].ty, ResolvedType::Generic(_, _)),
            "publication must retain the mode"
        );
        let owner = call.owner.as_ref().ok_or("callable owner missing")?;
        assert_eq!(owner.declaration_name, "use");
        assert!(matches!(
            checker.type_info.protocols.explicit_input_flow.get(owner),
            Some(InputFlow::Checked { .. })
        ));
        Ok(())
    }

    /// Call-site requests apply even to read-only bodies and ordinary callers, including named argument order.
    #[test]
    fn explicit_calls_check_source_access_and_transfer() -> Result<(), Box<dyn std::error::Error>> {
        for (source, conflicts) in [
            (
                "def use(mut value: int) -> int:\n    set(value)\n    return value\ndef set(mut value: BorrowedMut[int]) -> None:\n    value = 2\n",
                false,
            ),
            (
                "def use(value: str) -> None:\n    read(value, value)\ndef read(first: Borrowed[str], second: Borrowed[str]) -> None:\n    pass\n",
                false,
            ),
            (
                "def use(value: str) -> str:\n    take(value)\n    return value\ndef take(value: Owned[str]) -> None:\n    pass\n",
                true,
            ),
            (
                "def use(value: int) -> int:\n    take(value)\n    return value\ndef take(value: Owned[int]) -> None:\n    pass\n",
                false,
            ),
            (
                "def use(value: int) -> None:\n    read(value)\ndef read(value: BorrowedMut[int]) -> None:\n    pass\n",
                true,
            ),
            (
                "def use(mut value: int) -> None:\n    read(value, value)\ndef read(first: BorrowedMut[int], second: BorrowedMut[int]) -> None:\n    pass\n",
                true,
            ),
            (
                "def use(value: str) -> None:\n    read(value, value)\ndef read(first: Borrowed[str], second: Owned[str]) -> None:\n    pass\n",
                true,
            ),
            (
                "def use(value: str) -> None:\n    read(second=take(value), first=value)\ndef read(first: Borrowed[str], second: int) -> None:\n    pass\ndef take(value: Owned[str]) -> int:\n    return 1\n",
                true,
            ),
            (
                "def use(value: str) -> None:\n    read(first=value, second=take(value))\ndef read(first: Borrowed[str], second: int) -> None:\n    pass\ndef take(value: Owned[str]) -> int:\n    return 1\n",
                true,
            ),
            (
                "def use(mut value: Pair) -> None:\n    read(value.left, value.right)\ndef read(first: BorrowedMut[int], second: BorrowedMut[int]) -> None:\n    pass\nmodel Pair:\n    left: int\n    right: int\n",
                false,
            ),
            (
                "def use(mut value: Pair) -> None:\n    read(value, value.left)\ndef read(first: BorrowedMut[Pair], second: Borrowed[int]) -> None:\n    pass\nmodel Pair:\n    left: int\n    right: int\n",
                true,
            ),
            (
                "def use(mut value: BorrowedMut[int]) -> int:\n    read(value)\n    value = 3\n    return value\ndef read(value: Borrowed[int]) -> None:\n    pass\n",
                false,
            ),
            (
                "def use(value: Borrowed[str]) -> None:\n    take(value)\ndef take(value: Owned[str]) -> None:\n    pass\n",
                true,
            ),
            (
                "def use(value: str) -> int:\n    return choose(second=value == \"x\", first=take(value))\ndef choose(first: Borrowed[int], second: bool) -> int:\n    return first\ndef take(value: Owned[str]) -> int:\n    return 1\n",
                true,
            ),
            (
                "def use(reader: Reader, value: str) -> int:\n    return reader.choose(second=value == \"x\", first=take(value))\ndef take(value: Owned[str]) -> int:\n    return 1\nmodel Reader:\n    marker: int\n    def choose(self, first: Borrowed[int], second: bool) -> int:\n        return first\n",
                false,
            ),
        ] {
            let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
            let analysis = flow::analyze(&graph).map_err(|error| format!("{error:?}"))?;
            assert_eq!(!analysis.conflicts.is_empty(), conflicts, "{source}\n{analysis:?}");
        }
        Ok(())
    }

    /// A synthetic protocol lookup at an existing expression span must not steal or invent the source call's
    /// explicit ownership contract when its temporary lookup artifacts are removed.
    #[test]
    fn explicit_calls_follow_call_site_save_and_restore() -> Result<(), Box<dyn std::error::Error>> {
        let mut checker = checked(
            "def use(value: int) -> int:\n    return read(value)\ndef read(value: Borrowed[int]) -> int:\n    return value\n",
        )?;
        let (span, original) = checker
            .type_info
            .protocols
            .explicit_input_calls
            .iter()
            .next()
            .map(|(span, call)| (*span, call.clone()))
            .ok_or("missing call")?;
        let facts = checker.type_info.take_call_site_facts(span);
        assert!(checker.type_info.protocols.explicit_input_calls.is_empty());
        checker
            .type_info
            .protocols
            .explicit_input_calls
            .insert(span, original.clone());
        checker.type_info.clear_body_ir_call_site_facts(span);
        assert!(checker.type_info.protocols.explicit_input_calls.is_empty());
        checker.type_info.restore_call_site_facts(span, facts);
        assert_eq!(
            checker.type_info.protocols.explicit_input_calls.get(&span),
            Some(&original)
        );
        let empty_span = crate::ast::Span::new(0, 0);
        let empty = checker.type_info.take_call_site_facts(empty_span);
        checker
            .type_info
            .protocols
            .explicit_input_calls
            .insert(empty_span, original);
        checker.type_info.restore_call_site_facts(empty_span, empty);
        assert!(
            !checker
                .type_info
                .protocols
                .explicit_input_calls
                .contains_key(&empty_span)
        );
        Ok(())
    }

    /// Explicit method parameters retain the exact selected overload, and their receiver contributes a loan in
    /// source evaluation order rather than gaining access from the method's observed implementation.
    #[test]
    fn explicit_method_calls_retain_receiver_access() -> Result<(), Box<dyn std::error::Error>> {
        for (source, conflict) in [
            (
                "def use(reader: Reader, value: str) -> None:\n    reader.read(value)\nclass Reader:\n    def read(self, value: Borrowed[str]) -> None:\n        pass\n",
                false,
            ),
            (
                "def use(mut reader: Reader, value: str) -> None:\n    reader.read(value)\nclass Reader:\n    def read(mut self, value: Borrowed[str]) -> None:\n        pass\n",
                false,
            ),
            (
                "def use(reader: Reader, value: str) -> None:\n    reader.read(value)\nclass Reader:\n    def read(mut self, value: Borrowed[str]) -> None:\n        pass\n",
                true,
            ),
            (
                "def use(mut reader: Reader) -> None:\n    reader.read(reader.value)\nclass Reader:\n    pub value: str\n    def read(mut self, value: Borrowed[str]) -> None:\n        pass\n",
                true,
            ),
            (
                "def use(reader: Reader, value: str) -> str:\n    reader.read(1, value)\n    return value\nclass Reader:\n    def read(self, value: Borrowed[int]) -> None:\n        pass\n    def read(self, prefix: int, value: Owned[str]) -> None:\n        pass\n",
                true,
            ),
        ] {
            let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
            let analysis = flow::analyze(&graph).map_err(|error| format!("{error:?}"))?;
            assert_eq!(!analysis.conflicts.is_empty(), conflict, "{source}\n{analysis:?}");
        }
        Ok(())
    }

    /// A method examined only for an argument diagnostic is not a selected ownership target.
    #[test]
    fn rejected_method_does_not_retain_input_access() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "def use(reader: Reader, value: str) -> None:\n    reader.read(value)\nclass Reader:\n    def read(self, value: Borrowed[int]) -> None:\n        pass\n",
        )?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() != Some("INCAN-T0123"))
        );
        assert!(
            checker.type_info.protocols.explicit_input_calls.is_empty(),
            "diagnostic fallback must not retain a rejected method's ownership contract"
        );
        Ok(())
    }

    /// A fresh computed argument owns bounded storage for its invocation, while parentheses around a place retain
    /// that place's permission. Computing a temporary must retain transfers made by nested argument evaluation.
    #[test]
    fn borrowed_arguments_can_use_bounded_temporary_owners() -> Result<(), Box<dyn std::error::Error>> {
        for (source, conflict) in [
            (
                "def use() -> None:\n    read(\"hello\")\ndef read(value: Borrowed[str]) -> None:\n    pass\n",
                false,
            ),
            (
                "def use() -> None:\n    read(1 + 2)\ndef read(value: Borrowed[int]) -> None:\n    pass\n",
                false,
            ),
            (
                "def use() -> None:\n    change(1)\ndef change(mut value: BorrowedMut[int]) -> None:\n    value = 2\n",
                false,
            ),
            (
                "def use(value: int) -> None:\n    change((value))\ndef change(mut value: BorrowedMut[int]) -> None:\n    value = 2\n",
                true,
            ),
            (
                "def use(value: str) -> str:\n    read(take(value))\n    return value\ndef read(value: Borrowed[str]) -> None:\n    pass\ndef take(value: Owned[str]) -> str:\n    return value\n",
                true,
            ),
        ] {
            let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
            let analysis = flow::analyze(&graph).map_err(|error| format!("{error:?}"))?;
            assert_eq!(!analysis.conflicts.is_empty(), conflict, "{source}\n{analysis:?}");
        }
        Ok(())
    }

    /// The body owner is a canonical input, not a span-table lookup. A dependency module with an equal source range
    /// cannot turn the current callable's borrowed parameter into an ordinary transferable one.
    #[test]
    fn flow_owner_is_independent_of_shared_span_tables() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def read(value: Borrowed[int]) -> int:\n    return value\n";
        let mut checker = checked(source)?;
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let declaration = program.declarations.first().ok_or("missing declaration")?;
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return Err("expected function".into());
        };
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .next()
            .cloned()
            .ok_or("missing owner")?;
        let binding = checker
            .type_info
            .declarations
            .function_bindings_by_span
            .get_mut(&declaration.span)
            .ok_or("missing binding")?;
        let mut other = owner.clone();
        other.origin = incan_semantics_core::SymbolOrigin::Module(vec!["other".into()]);
        binding.identity = Some(other);
        let graph = super::source::build(&checker, CallableSource::Function(function), declaration.span, &owner)
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(graph.permissions.len(), 1);
        assert!(
            graph
                .permissions
                .values()
                .all(|permission| !permission.transferable && !permission.exclusive)
        );
        Ok(())
    }

    /// A retained map entry alone cannot authenticate an annotation, destination, graph owner or analysis result.
    #[test]
    fn body_projection_requires_complete_canonical_local_evidence() -> Result<(), Box<dyn std::error::Error>> {
        let checker =
            checked("def read(value: Borrowed[int]) -> int:\n    let view: Borrowed[int] = value\n    return view\n")?;
        let (local, mode) = checker
            .type_info
            .protocols
            .explicit_local_modes
            .iter()
            .next()
            .ok_or("missing local")?;
        let owner = &mode.owner;
        let proof = checker.type_info.checked_input_body(owner)?.ok_or("missing proof")?;
        assert_eq!(proof.local_mode(local), Some(OwnershipMode::Borrowed));
        let mut other = local.clone();
        other.declaration_name = "foreign".into();
        assert_eq!(proof.local_mode(&other), None);
        for corruption in 0..8 {
            let mut facts = checker.type_info.clone();
            match corruption {
                0 => {
                    facts.protocols.explicit_input_flow.remove(owner);
                }
                1 => {
                    facts.protocols.explicit_local_modes.remove(local);
                }
                2 => {
                    facts
                        .protocols
                        .explicit_local_modes
                        .get_mut(local)
                        .ok_or("missing mode")?
                        .owner
                        .declaration_name = "foreign".into();
                }
                3 => {
                    facts
                        .protocols
                        .explicit_local_modes
                        .get_mut(local)
                        .ok_or("missing mode")?
                        .value_type = ResolvedType::Str;
                }
                4 => {
                    facts
                        .references
                        .resolved_write_identities
                        .remove(&(local.declaration_span, local.declaration_name.clone()));
                }
                _ => {
                    let InputFlow::Checked { graph, analysis } = facts
                        .protocols
                        .explicit_input_flow
                        .get_mut(owner)
                        .ok_or("missing flow")?
                    else {
                        return Err("expected checked flow".into());
                    };
                    match corruption {
                        5 => graph.local_loans.clear(),
                        6 => analysis.live_before.clear(),
                        _ => graph.owner.declaration_name = "foreign".into(),
                    }
                }
            }
            assert!(
                facts.checked_input_body(owner).is_err(),
                "accepted corrupted evidence {corruption}"
            );
        }
        Ok(())
    }

    /// Fixed named calls retain declaration-order evaluation and distinct written positions, slots and source loans.
    #[test]
    fn invocation_projection_retains_slots_and_loans() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "def choose(first: Borrowed[int], second: Borrowed[int]) -> int:\n    return first + second\ndef use(left: int, right: int) -> int:\n    return choose(second=right, first=left)\n",
        )?;
        let (owner, output) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "use")
            .ok_or("missing caller flow")?;
        let InputFlow::Checked { graph, analysis } = output else {
            return Err("caller flow unavailable".into());
        };
        assert!(analysis.conflicts.is_empty());
        let (span, invocation) = graph.invocations.iter().next().ok_or("missing invocation")?;
        assert_eq!(
            invocation
                .arguments
                .iter()
                .map(|argument| argument.slot)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(
            invocation
                .arguments
                .iter()
                .map(|argument| argument.written_position)
                .collect::<Vec<_>>(),
            [1, 0]
        );
        assert_ne!(invocation.arguments[0].loan, invocation.arguments[1].loan);
        assert!(checker.type_info.checked_input_body(owner)?.is_some());
        for corruption in 0..6 {
            let mut facts = checker.type_info.clone();
            let InputFlow::Checked { graph, .. } = facts
                .protocols
                .explicit_input_flow
                .get_mut(owner)
                .ok_or("missing flow")?
            else {
                return Err("expected checked flow".into());
            };
            let invocation = graph.invocations.get_mut(span).ok_or("missing invocation")?;
            match corruption {
                0 => invocation.callee.declaration_name = "foreign".into(),
                1 => invocation.arguments[0].slot = invocation.arguments[1].slot,
                2 => invocation.arguments[0].loan = Some(flow::LoanId(usize::MAX)),
                3 => {
                    invocation.arguments[0].access = incan_semantics_core::IncanInputAccess::Owned;
                    invocation.arguments[0].loan = None;
                }
                4 => invocation.arguments[0].source = None,
                _ => {
                    facts
                        .protocols
                        .explicit_input_calls
                        .get_mut(span)
                        .ok_or("missing call facts")?
                        .owner = None;
                }
            }
            assert!(
                facts.checked_input_body(owner).is_err(),
                "accepted corrupted invocation {corruption}"
            );
        }
        Ok(())
    }

    /// Owned calls transfer even when the callee only compares the value; a later caller use is a source conflict.
    #[test]
    fn readonly_owned_callee_still_consumes_the_argument() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "def consume(value: Owned[str]) -> bool:\n    return value == \"x\"\ndef use(value: str) -> str:\n    result = consume(value)\n    return value\n",
        )?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{:?}",
            checker.errors
        );
        let (_, output) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "use")
            .ok_or("missing caller flow")?;
        let InputFlow::Checked { analysis, .. } = output else {
            return Err("caller flow unavailable".into());
        };
        assert!(
            analysis
                .conflicts
                .iter()
                .any(|conflict| matches!(conflict.kind, flow::ConflictKind::Moved(_)))
        );
        Ok(())
    }

    /// Ordinary bodies need no ownership proof; source conflicts and unsupported flows cannot produce one.
    #[test]
    fn body_projection_distinguishes_absent_and_refused_flows() -> Result<(), Box<dyn std::error::Error>> {
        for (source, ordinary) in [
            ("def read(value: int) -> int:\n    return value\n", true),
            (
                "def read(mut value: Owned[int]) -> int:\n    let view: Borrowed[int] = value\n    value = 2\n    return view\n",
                false,
            ),
            (
                "def read(value: Borrowed[int]) -> Borrowed[int] from value:\n    return value\n",
                false,
            ),
        ] {
            let checker = checked(source)?;
            let owner = checker
                .type_info
                .declarations
                .function_bindings_by_span
                .values()
                .find_map(|binding| binding.identity.as_ref())
                .ok_or("missing callable")?;
            let projected = checker.type_info.checked_input_body(owner);
            if ordinary {
                assert!(projected?.is_none());
            } else {
                assert!(projected.is_err());
            }
        }
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

    /// Reannotation, method bodies and non-Copy local values cannot borrow the Copy local-declaration admission.
    #[test]
    fn local_alias_admission_preserves_other_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def wrong() -> int:\n    mut value = 1\n    value: BorrowedMut[int] = 2\n    return value\n",
            "class Counter:\n    value: int\n    def read(self) -> int:\n        let local: Borrowed[int] = self.value\n        return local\n",
            "def read(value: Borrowed[str]) -> int:\n    let local: Borrowed[str] = value\n    return len(local)\n",
            "def read(value: Owned[int]) -> int:\n    let local: Owned[int] = value\n    return local\n",
        ] {
            let checker = checked(source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                "{source}: {:?}",
                checker.errors
            );
        }
        Ok(())
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
