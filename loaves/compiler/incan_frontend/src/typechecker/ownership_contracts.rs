//! Canonical RFC 164 contracts, source access checks and staged executable-body admission.
//!
//! A direct input annotation is admitted only with its complete supported body proof. Returned dependencies,
//! deferred boundaries and unsupported storage shapes remain refused rather than acquiring repair copies.

use incan_lang::lang::types::ownership::{self, OwnershipMode};
use incan_semantics_core::SymbolOrigin;

mod admission;
mod builtins;
mod checked;
mod copy;
pub(super) mod flow;
pub(super) mod projection;
mod report;
mod source;

pub(super) use admission::{direct_function_boundary, direct_method_boundary};
pub(crate) use builtins::carried_view_binding;
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
    pub kind: InputCallKind,
    pub owner: Option<incan_semantics_core::CanonicalSymbolId>,
    pub callee: incan_semantics_core::CanonicalSymbolId,
    pub parameters: Vec<crate::symbols::CallableParam>,
    pub receiver: Option<crate::ast::Receiver>,
}

/// Declaration classification is separate from argument access. Ordinary facts are provisional until their body
/// independently requires an input proof, and cannot replace an explicit contract missing from the declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputCallKind {
    /// The selected declaration carries an explicit input contract.
    Explicit,
    /// A synchronous plain declaration retains its existing inferred ABI.
    Ordinary,
}

impl TypeChecker {
    /// The body writer starts with isolated references. Retain the prepared parameter bindings before statement
    /// checking can shadow their names, so source admission and persisted restoration use the same canonical facts.
    pub(super) fn retain_callable_input_bindings(&mut self, callable: CallableSource<'_>) {
        let parameters = match callable {
            CallableSource::Function(function) => &function.params,
            CallableSource::Method(method) => &method.params,
        };
        for parameter in parameters {
            self.record_write_target_identity(parameter.span, &parameter.node.name);
        }
    }

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
                .any(|call| call.owner.as_ref() == Some(&identity) && call.kind == InputCallKind::Explicit)
        {
            self.type_info
                .protocols
                .explicit_input_calls
                .retain(|_, call| call.owner.as_ref() != Some(&identity) || call.kind == InputCallKind::Explicit);
            return;
        }
        self.retain_input_copy_values(&identity, span);
        let (graph, view_errors) = source::build_reporting_views(self, callable, span, &identity);
        let view_refused = !view_errors.is_empty();
        self.errors.extend(view_errors);
        let output = match graph {
            // A return that does not borrow the declared view source leaves the body without a proof.
            Ok(_) if view_refused => InputFlow::Unavailable(Unavailable {
                span,
                reason: "returned view does not borrow its declared source".into(),
            }),
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
        let admission = match callable {
            CallableSource::Function(function) => self.type_info.executable_function_input(&identity, function),
            CallableSource::Method(method) => self.type_info.executable_method_input(&identity, method),
        };
        // A refused returned view already names the offending return; admission would only repeat it.
        if let Err(reason) = admission
            && !view_refused
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

    /// Resource-observer fixtures must reach cleanup through represented call boundaries. An ordinary helper
    /// retains its selected header but still needs non-Copy argument or nominal returned-dependency facts.
    #[test]
    fn resource_probe_wrappers_retain_exact_source_boundary_refusals() -> Result<(), Box<dyn std::error::Error>> {
        for (source, call, reason) in [
            (
                "def observe(label: str) -> None:\n    print(label)\ndef consume(value: Owned[str]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    value = \"initial\"\n    if flag:\n        consume(value)\n    observe(\"join\")\n",
                "\"join\"",
                "ordinary argument requires inferred access or mutable binding facts",
            ),
            (
                "class Holder:\n    pub value: int\ndef make(value: int) -> Holder:\n    return Holder(value=value)\ndef consume(value: Owned[Holder]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    value = make(1)\n    if flag:\n        consume(value)\n",
                "make(1)",
                "call result needs retained dependency facts",
            ),
            (
                "class Holder:\n    pub value: int\ndef make(value: Owned[int]) -> Holder:\n    return Holder(value=value)\ndef consume(value: Owned[Holder]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    value = make(1)\n    if flag:\n        consume(value)\n",
                "make(1)",
                "call result needs retained dependency facts",
            ),
        ] {
            let checker = checked(source)?;
            let state = checker
                .type_info
                .protocols
                .explicit_input_flow
                .iter()
                .find(|(owner, _)| owner.declaration_name == "main")
                .map(|(_, state)| state)
                .ok_or("missing caller source flow")?;
            let InputFlow::Unavailable(unavailable) = state else {
                return Err(format!("expected exact unavailable boundary: {state:?}").into());
            };
            assert_eq!(unavailable.reason, reason);
            assert_eq!(source.get(unavailable.span.start..unavailable.span.end), Some(call));
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123"))
            );
        }
        Ok(())
    }

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
    /// inherit a previous body's ownership profile, including in the legacy cold-baseline comparison route.
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
        for source in [
            "def inspect(value: Borrowed[str], flag: bool) -> None:\n    assert flag, value\n    print(value)\n",
            "def inspect(value: Borrowed[str]) -> None:\n    assert false, f\"failed: {value}\"\n    print(value)\n",
            "def inspect(value: Borrowed[str]) -> None:\n    assert true, \"literal\"\n    assert false, value\n",
            "def inspect(value: Borrowed[str], flag: bool) -> None:\n    assert flag, f\"first: {value}\"\n    assert flag, f\"second: {value}\"\n    return\n    print(value)\n",
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(value: Borrowed[str], mut other: BorrowedMut[str], flag: bool) -> None:\n    assert flag, f\"{value}{replace(other)}\"\n    print(value)\n",
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut other: BorrowedMut[str], flag: bool) -> None:\n    assert flag, replace(other)\n    print(other)\n",
            "def inspect(flag: bool) -> None:\n    assert flag, consume(\"fresh\")\ndef consume(value: Owned[str]) -> str:\n    return \"failed\"\n",
            "def inspect(value: Owned[int], flag: bool) -> None:\n    assert flag, consume(value)\n    print(value)\ndef consume(value: Owned[int]) -> str:\n    return \"failed\"\n",
        ] {
            let checker = checked(source)?;
            assert!(checker.errors.is_empty(), "{source}\n{:?}", checker.errors);
        }
        for binding in ["value = \"stored\"", "let value: Owned[str] = \"stored\""] {
            let source = format!(
                "def inspect(flag: bool) -> None:\n    {binding}\n    assert flag, consume(value)\ndef consume(value: Owned[str]) -> str:\n    return \"failed\"\n"
            );
            let checker = checked(&source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                "conditional stored transfer admitted: {source}"
            );
        }
        let source = "def inspect(value: Owned[str], flag: bool) -> None:\n    assert flag, consume(value)\ndef consume(value: Owned[str]) -> str:\n    return \"failed\"\n";
        let checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "conditional parameter transfer admitted"
        );
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
            (
                "def branch(flag: bool) -> None:\n    value = \"owned\"\n    if flag:\n        consume(value)\n    print(\"after join\")\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                false,
            ),
            (
                "def branch(flag: bool) -> None:\n    let value: Owned[str] = \"owned\"\n    if flag:\n        consume(value)\n    print(\"after join\")\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                false,
            ),
            (
                "def branch(value: Owned[str], flag: bool) -> None:\n    if flag:\n        consume(value)\n    else:\n        return\n    print(\"after join\")\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                false,
            ),
            (
                "def branch(flag: bool) -> None:\n    mut value = \"owned\"\n    if flag:\n        consume(value)\n    value = \"new\"\n    print(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                false,
            ),
            (
                "def branch(flag: bool) -> None:\n    value = \"owned\"\n    if flag:\n        consume(value)\n    print(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                true,
            ),
            // A loop shares its stored iterable for every poll: items are read freely, the iterable is not edited
            // until the loop ends.
            (
                "def total(values: Borrowed[list[int]]) -> int:\n    mut sum = 0\n    for value in values:\n        sum += value\n    return sum\n",
                false,
            ),
            (
                "def reset(mut values: BorrowedMut[list[int]]) -> None:\n    for value in values:\n        values = [value]\n",
                true,
            ),
            (
                "def after(mut values: BorrowedMut[list[int]]) -> None:\n    mut sum = 0\n    for value in values:\n        sum += value\n    values = [sum]\n",
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

    /// Calls, aggregate results and temporary owners remain unavailable until their exact checked access
    /// and dependency facts exist; a partial graph cannot count as validation of the whole callable.
    #[test]
    fn source_flow_refuses_unprojected_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def show(value: Borrowed[int]) -> int:\n    return abs(value)\n",
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

    /// A selected method closes Self in its ownership contract using the same receiver as its parameter types; a
    /// shared receiver and a shared argument may borrow the same owner.
    #[test]
    fn method_input_payload_retains_checked_self_substitution() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "class Reader:\n    def view(self, other: Borrowed[Self]) -> None:\n        pass\ndef run() -> None:\n    reader = Reader()\n    reader.view(reader)\n",
        )?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
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
            // A method frame without conflicts is admitted; a conflicting one has no executable proof.
            assert_eq!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                conflict,
                "{source}: {:?}",
                checker.errors
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

    /// Plain helpers participate only in a body that already owes an input proof. Failed overload candidates
    /// cannot manufacture that obligation or leave a selected ordinary signature behind.
    #[test]
    fn ordinary_helpers_are_scoped_to_required_input_bodies() -> Result<(), Box<dyn std::error::Error>> {
        let ordinary = checked(
            "def add(left: int, right: int) -> int:\n    return left + right\ndef use(value: int) -> int:\n    return add(right=2, left=value)\n",
        )?;
        assert!(ordinary.errors.is_empty(), "{:?}", ordinary.errors);
        assert!(ordinary.type_info.protocols.explicit_input_calls.is_empty());
        assert!(ordinary.type_info.protocols.explicit_input_flow.is_empty());
        for source in [
            "def add(left: int, right: int) -> int:\n    return left + right\ndef use(value: Borrowed[int]) -> int:\n    return add(right=2, left=value)\n",
            "def add(left: int, right: int) -> int:\n    return left + right\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef use(value: int) -> int:\n    first = add(value, 2)\n    return read(first)\n",
        ] {
            let checker = checked(source)?;
            assert!(checker.errors.is_empty(), "{source}\n{:?}", checker.errors);
            assert!(
                checker
                    .type_info
                    .protocols
                    .explicit_input_calls
                    .values()
                    .any(|call| call.callee.declaration_name == "add")
            );
        }
        for (source, accepted) in [
            (
                "def identity[T](value: T) -> T:\n    return value\ndef use(value: Borrowed[int]) -> int:\n    return identity(value)\n",
                false,
            ),
            (
                "def phantom[T](value: int) -> int:\n    return value\ndef use(value: Borrowed[int]) -> int:\n    return phantom[int](value)\n",
                true,
            ),
            (
                "def absent[T](value: int) -> Option[T]:\n    return None\ndef use(value: Borrowed[int]) -> Option[int]:\n    return absent[int](value)\n",
                true,
            ),
        ] {
            let checker = checked(source)?;
            assert_eq!(checker.errors.is_empty(), accepted, "{source}\n{:?}", checker.errors);
            assert!(
                checker
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0123")),
                "{source}\n{:?}",
                checker.errors
            );
        }
        let failed = checked(
            "def choose(value: int) -> int:\n    return value\ndef choose(other: int) -> int:\n    return other\ndef use(value: Borrowed[int]) -> int:\n    return choose(value)\n",
        )?;
        assert!(
            failed.errors.iter().any(|error| error.message.contains("ambiguous")),
            "{:?}",
            failed.errors
        );
        assert!(failed.type_info.protocols.explicit_input_calls.is_empty());
        Ok(())
    }

    /// Declaration classification is authenticated independently of restored access events. Neither replacing an
    /// explicit contract nor borrowing a same-shaped ordinary header may authorize a forged invocation.
    #[test]
    fn ordinary_call_replay_requires_its_exact_header() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def add(left: int, right: int) -> int:\n    return left + right\ndef use(value: Borrowed[int]) -> int:\n    return add(right=2, left=value)\n";
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        let (span, call) = checker
            .type_info
            .protocols
            .explicit_input_calls
            .iter()
            .next()
            .ok_or("missing ordinary call")?;
        assert_eq!(call.kind, InputCallKind::Ordinary);
        let owner = call.owner.as_ref().ok_or("missing caller")?;
        assert!(checker.type_info.checked_input_body(owner)?.is_some());
        for corruption in 0..7 {
            let mut facts = checker.type_info.clone();
            let changed = facts
                .protocols
                .explicit_input_calls
                .get_mut(span)
                .ok_or("missing call")?;
            match corruption {
                0 => changed.kind = InputCallKind::Explicit,
                1 => changed.parameters[0].is_mut = true,
                2 => changed.owner = Some(call.callee.clone()),
                3 => {
                    facts
                        .declarations
                        .function_bindings_by_span
                        .retain(|_, binding| binding.identity.as_ref() != Some(&call.callee));
                    facts
                        .declarations
                        .function_bindings
                        .retain(|_, binding| binding.identity.as_ref() != Some(&call.callee));
                }
                4 => {
                    changed.parameters[0].is_mut = true;
                    for binding in facts
                        .declarations
                        .function_bindings_by_span
                        .values_mut()
                        .chain(facts.declarations.function_bindings.values_mut())
                    {
                        if binding.identity.as_ref() == Some(&call.callee) {
                            binding.params[0].is_mut = true;
                        }
                    }
                }
                5 => {
                    let mut conflict = facts
                        .declarations
                        .function_bindings_by_span
                        .get(&call.callee.declaration_span)
                        .ok_or("missing header")?
                        .clone();
                    conflict.params[0].is_mut = true;
                    facts
                        .declarations
                        .function_bindings
                        .insert("other_alias".into(), conflict);
                }
                _ => {
                    facts.protocols.explicit_input_calls.remove(span);
                }
            }
            assert!(
                facts.checked_input_body(owner).is_err(),
                "accepted ordinary call corruption {corruption}"
            );
        }
        let mut explicit = checked(
            "def read(value: Borrowed[int]) -> int:\n    return value\ndef use(value: int) -> int:\n    return read(value)\n",
        )?;
        let call = explicit
            .type_info
            .protocols
            .explicit_input_calls
            .values_mut()
            .next()
            .ok_or("missing explicit call")?;
        call.kind = InputCallKind::Ordinary;
        let owner = call.owner.clone().ok_or("missing caller")?;
        assert!(explicit.type_info.checked_input_body(&owner).is_err());
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

    /// Restored graphs cannot omit an operand's consumption dependency and recompute themselves into acceptance.
    #[test]
    fn restored_alias_flow_requires_operation_consumption() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef main() -> None:\n    mut original = \"payload\"\n    let view: Borrowed[str] = original\n    print(view, replace(original))\n";
        let mut checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126"))
        );
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let function = program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                crate::ast::Declaration::Function(function) if function.name == "main" => Some(function),
                _ => None,
            })
            .ok_or("missing main")?;
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "main")
            .cloned()
            .ok_or("missing owner")?;
        let InputFlow::Checked { graph, analysis } = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or("missing flow")?
        else {
            return Err("missing checked graph".into());
        };
        let loan = graph
            .operations
            .values()
            .flat_map(|operation| &operation.operands)
            .next()
            .ok_or("missing operation loan")?
            .loan;
        let consumption = graph
            .nodes
            .iter_mut()
            .rev()
            .find(|node| {
                matches!(node.event,
                    flow::Event::Access { access: flow::Access::Read, through: Some(id), .. } if id == loan
                )
            })
            .ok_or("missing consumption read")?;
        consumption.event = flow::Event::Nothing;
        graph.operations.clear();
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty());
        assert!(
            checker.type_info.direct_input_admission(&owner, function).is_err(),
            "stripped operation dependency admitted an invalid alias read"
        );
        Ok(())
    }

    /// Existing operation metadata cannot authorize missing, reused or changed consumption events.
    #[test]
    fn operation_consumption_rejects_altered_relationships() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "def read(value: Borrowed[str]) -> int:\n    let view: Borrowed[str] = value\n    return len(view)\n";
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let crate::ast::Declaration::Function(function) =
            &program.declarations.first().ok_or("missing declaration")?.node
        else {
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
        for corruption in 0..10 {
            let mut facts = checker.type_info.clone();
            let InputFlow::Checked { graph, analysis } = facts
                .protocols
                .explicit_input_flow
                .get_mut(&owner)
                .ok_or("missing flow")?
            else {
                return Err("missing checked graph".into());
            };
            let inputs = graph.operations.values_mut().next().ok_or("missing operation")?;
            let dependency = inputs.operands.first_mut().ok_or("missing operand")?;
            match corruption {
                0 => graph.operations.clear(),
                1 => graph.nodes[dependency.final_access].event = flow::Event::Nothing,
                2 => dependency.final_access = inputs.completion_node,
                3 => inputs.completion_node = 0,
                4 => dependency.operand.start += 1,
                5 => dependency.loan = flow::LoanId(graph.loans.len()),
                6 => {
                    let flow::Root::Binding(root) = &mut dependency.source.root else {
                        return Err("expected declared source".into());
                    };
                    root.declaration_name = "foreign".into();
                }
                7 => {
                    let parent = graph.loans[dependency.loan.0].parent.ok_or("missing parent")?;
                    dependency.loan = parent;
                    let flow::Event::Access { through, .. } = &mut graph.nodes[dependency.final_access].event else {
                        return Err("missing final read".into());
                    };
                    *through = Some(parent);
                }
                8 => graph.loans[dependency.loan.0].parent = None,
                _ => graph.loans[dependency.loan.0].kind = flow::LoanKind::Exclusive,
            }
            // Recompute when structurally possible; retained analysis alone must not be the refusal mechanism.
            if let Ok(recomputed) = flow::analyze(graph) {
                *analysis = recomputed;
            }
            assert!(
                facts.direct_input_admission(&owner, function).is_err(),
                "accepted corruption {corruption}"
            );
        }
        Ok(())
    }

    /// A restored graph cannot move an earlier text read past a later mutation to erase the source-order conflict.
    #[test]
    fn operation_reads_cannot_move_after_later_operands() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str]) -> None:\n    print(value, replace(value))\n",
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str], flag: bool) -> None:\n    assert flag, f\"{value}{replace(value)}\"\n    print(value)\n",
        ] {
            let mut checker = checked(source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0126"))
            );
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let function = program
                .declarations
                .iter()
                .find_map(|declaration| match &declaration.node {
                    crate::ast::Declaration::Function(function) if function.name == "inspect" => Some(function),
                    _ => None,
                })
                .ok_or("missing inspect")?;
            let owner = checker
                .type_info
                .protocols
                .explicit_input_flow
                .keys()
                .find(|owner| owner.declaration_name == "inspect")
                .cloned()
                .ok_or("missing owner")?;
            let original = checker.type_info.clone();
            for corruption in 0..5 {
                checker.type_info = original.clone();
                let InputFlow::Checked { graph, analysis } = checker
                    .type_info
                    .protocols
                    .explicit_input_flow
                    .get_mut(&owner)
                    .ok_or("missing flow")?
                else {
                    return Err("missing checked flow".into());
                };
                let dependency = graph
                    .operations
                    .values()
                    .flat_map(|operation| &operation.operands)
                    .next()
                    .ok_or("missing operand")?;
                let origin = graph
                    .nodes
                    .iter()
                    .position(|node| matches!(node.event, flow::Event::Borrow(id) if id == dependency.loan))
                    .ok_or("missing creation")?;
                let read = origin.checked_sub(1).ok_or("missing evaluated read")?;
                let invocation = graph.invocations.values().next().ok_or("missing nested call")?;
                let call_start = invocation
                    .arguments
                    .first()
                    .ok_or("missing input")?
                    .evaluation
                    .start_node;
                let call_end = *invocation.final_accesses.last().ok_or("missing final input use")?;
                // Move the complete nested invocation, including its evaluation markers, around the first operand.
                if corruption == 0 {
                    let before = read.checked_sub(1).ok_or("missing preceding node")?;
                    graph.nodes[before].successors = vec![call_start];
                    graph.nodes[call_end].successors = vec![read];
                    graph.nodes[origin].successors = vec![call_end + 1];
                } else if corruption == 1 {
                    graph.nodes[read..=call_end].rotate_left(call_start - read);
                    let count = graph.nodes.len();
                    for (index, node) in graph.nodes.iter_mut().enumerate() {
                        node.successors = if index + 1 < count { vec![index + 1] } else { Vec::new() };
                    }
                } else if corruption == 2 {
                    graph.nodes[graph.entry].successors.clear();
                } else if corruption == 3 {
                    let operation = graph.operations.values().next().ok_or("missing operation")?;
                    let last = operation.operands.last().ok_or("missing final operand")?.final_access;
                    let continuation = graph.nodes[last].successors.clone();
                    graph.nodes[origin].successors = vec![operation.completion_node];
                    graph.nodes[last].successors = vec![call_start];
                    graph.nodes[call_end].successors = continuation;
                } else {
                    graph.nodes[graph.entry].successors = vec![graph.entry];
                }
                *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
                assert!(analysis.conflicts.is_empty(), "{analysis:?}");
                assert!(
                    checker.type_info.direct_input_admission(&owner, function).is_err(),
                    "accepted invalid operation execution (corruption {corruption})"
                );
            }
        }
        Ok(())
    }

    /// Conditional path evidence is checked independently, and a body whose conditional paths project completely
    /// executes under the conditional profile.
    #[test]
    fn conditional_paths_require_their_source_edges() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def inspect(value: Borrowed[str], flag: bool) -> None:\n    if flag:\n        pass\n    print(value)\n",
            "def inspect(value: Borrowed[str]) -> None:\n    if true:\n        pass\n    else:\n        pass\n    print(value)\n",
            "def inspect(value: Borrowed[str], flag: bool, other: bool) -> None:\n    if flag:\n        print(value)\n    elif other:\n        assert other, f\"failed: {value}\"\n        return\n    else:\n        pass\n    print(value)\n",
            "def inspect(value: Borrowed[str], flag: bool) -> None:\n    if flag:\n        print(value)\n        return\n    else:\n        print(value)\n        return\n    print(value)\n",
            "def inspect(value: Borrowed[str], flag: bool, other: bool) -> None:\n    if flag:\n        if other:\n            print(value)\n        else:\n            pass\n    else:\n        assert true, \"literal\"\n    print(value)\n",
        ] {
            let checker = checked(source)?;
            assert!(
                !checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                "{source}: {:?}",
                checker.errors
            );
            assert!(
                !checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0126"))
            );
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let declaration = program.declarations.first().ok_or("missing declaration")?;
            let crate::ast::Declaration::Function(function) = &declaration.node else {
                return Err("missing function".into());
            };
            let owner = checker
                .type_info
                .protocols
                .explicit_input_flow
                .keys()
                .next()
                .ok_or("missing owner")?;
            checker
                .type_info
                .checked_input_body(owner)?
                .ok_or("missing proof")?
                .validate_operation_coverage(CallableSource::Function(function))?;
            let InputFlow::Checked { graph, .. } = checker
                .type_info
                .protocols
                .explicit_input_flow
                .get(owner)
                .ok_or("missing graph")?
            else {
                return Err("missing checked graph".into());
            };
            for branch in graph
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(index, node)| (node.successors.len() >= 2).then_some(index))
            {
                for reorder in [false, true] {
                    if reorder && graph.nodes[branch].successors[0] == graph.nodes[branch].successors[1] {
                        continue;
                    }
                    let mut facts = checker.type_info.clone();
                    let InputFlow::Checked { graph, analysis } = facts
                        .protocols
                        .explicit_input_flow
                        .get_mut(owner)
                        .ok_or("missing flow")?
                    else {
                        return Err("missing checked flow".into());
                    };
                    if reorder {
                        graph.nodes[branch].successors.swap(0, 1);
                    } else {
                        graph.nodes[branch].successors.remove(0);
                    }
                    *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
                    assert!(analysis.conflicts.is_empty());
                    assert!(
                        facts
                            .checked_input_body(owner)?
                            .ok_or("missing proof")?
                            .validate_operation_coverage(CallableSource::Function(function))
                            .is_err(),
                        "accepted altered branch {branch}"
                    );
                }
            }
        }
        Ok(())
    }

    /// Failure-only message evaluation cannot become unconditional, skip its dependency, or join the success path.
    #[test]
    fn assertion_execution_retains_failure_edges() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def inspect(value: Borrowed[str], flag: bool) -> None:\n    assert flag, f\"failed: {value}\"\n    print(value)\n";
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let declaration = program.declarations.first().ok_or("missing declaration")?;
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return Err("missing function".into());
        };
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .next()
            .ok_or("missing owner")?;
        for corruption in 0..4 {
            let mut facts = checker.type_info.clone();
            let InputFlow::Checked { graph, analysis } = facts
                .protocols
                .explicit_input_flow
                .get_mut(owner)
                .ok_or("missing flow")?
            else {
                return Err("missing checked flow".into());
            };
            let condition = graph
                .nodes
                .iter()
                .position(|node| node.successors.len() == 2)
                .ok_or("missing assertion split")?;
            let failure = graph.nodes[condition].successors[0];
            let success = graph.nodes[condition].successors[1];
            let message = graph.operations.values().next().ok_or("missing message operation")?;
            let tail = message
                .operands
                .last()
                .ok_or("missing message dependency")?
                .final_access;
            match corruption {
                0 => graph.nodes[tail].successors = vec![success],
                1 => {
                    graph.nodes[condition].successors = vec![failure];
                    graph.nodes[tail].successors = vec![success];
                }
                2 => graph.nodes[condition].successors = vec![success],
                _ => graph.nodes[condition].successors.swap(0, 1),
            }
            *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
            assert!(analysis.conflicts.is_empty(), "{analysis:?}");
            assert!(
                facts.direct_input_admission(owner, function).is_err(),
                "accepted assertion edge corruption {corruption}"
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
        let mut facts = checker.type_info.clone();
        let InputFlow::Checked { graph, analysis } = facts
            .protocols
            .explicit_input_flow
            .get_mut(owner)
            .ok_or("missing caller flow")?
        else {
            return Err("caller flow unavailable".into());
        };
        graph
            .invocations
            .get_mut(span)
            .ok_or("missing invocation")?
            .arguments
            .swap(0, 1);
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty());
        let reordered_graph = graph.clone();
        assert!(
            facts.checked_input_body(owner).is_err(),
            "accepted reordered complete invocation arguments"
        );
        for corruption in 0..5 {
            let mut graph = reordered_graph.clone();
            let invocation = graph.invocations.get_mut(span).ok_or("missing invocation")?;
            match corruption {
                0 => invocation.completion_node = usize::MAX,
                1 => invocation.final_accesses[0] = usize::MAX,
                2 => invocation.arguments[0].evaluation.start_node = usize::MAX,
                3 => invocation.arguments[0].evaluation.end_node = usize::MAX,
                _ => invocation.arguments[0].evaluation.source_node = Some(usize::MAX),
            }
            assert!(
                matches!(
                    flow::analyze(&graph),
                    Err(flow::InvalidGraph("invocation event link is outside callable"))
                ),
                "accepted out-of-range invocation link {corruption}"
            );
        }
        Ok(())
    }

    /// A retained outer borrow cannot end before a later argument mutates its source, even after restored flow
    /// analysis is recomputed to hide that conflict.
    #[test]
    fn invocation_final_use_follows_all_argument_evaluations() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def choose(first: Borrowed[int], second: bool) -> int:\n    return first\ndef mutate(mut value: BorrowedMut[int]) -> bool:\n    value = 2\n    return true\ndef use(mut value: int) -> int:\n    return choose(value, mutate(value))\n";
        let mut checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{:?}",
            checker.errors
        );
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let function = program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
                _ => None,
            })
            .ok_or("missing use function")?;
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "use")
            .cloned()
            .ok_or("missing owner")?;
        let InputFlow::Checked { graph, analysis } = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or("missing flow")?
        else {
            return Err("missing checked flow".into());
        };
        let outer = graph
            .invocations
            .values()
            .find(|call| call.callee.declaration_name == "choose")
            .ok_or("missing choose")?;
        let outer_loan = outer
            .arguments
            .first()
            .and_then(|argument| argument.loan)
            .ok_or("missing outer loan")?;
        let final_read = graph.nodes.iter().position(|node| matches!(node.event, flow::Event::Access { through: Some(id), access: flow::Access::Read, .. } if id == outer_loan)).ok_or("missing outer final use")?;
        let origin = graph
            .nodes
            .iter()
            .position(|node| matches!(node.event, flow::Event::Borrow(id) if id == outer_loan))
            .ok_or("missing origin")?;
        let terminal = graph.nodes.remove(final_read);
        graph.nodes.insert(origin + 1, terminal);
        let count = graph.nodes.len();
        for (index, node) in graph.nodes.iter_mut().enumerate() {
            node.successors = if index + 1 < count { vec![index + 1] } else { Vec::new() };
        }
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty(), "{analysis:?}");
        assert!(
            checker.type_info.direct_input_admission(&owner, function).is_err(),
            "accepted outer loan ending before later argument mutation"
        );
        Ok(())
    }

    /// A fresh argument's complete nested evaluation belongs inside its retained interval. A marker from the
    /// nested call cannot stand in for the outer argument's start merely because their full source spans agree.
    #[test]
    fn invocation_intervals_cover_nested_argument_effects() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def produce(value: Owned[int]) -> int:\n    return value\ndef consume(value: Owned[int]) -> int:\n    return value\ndef use(value: int) -> int:\n    return consume(produce(value))\n";
        let mut checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let function = program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
                _ => None,
            })
            .ok_or("missing use function")?;
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "use")
            .cloned()
            .ok_or("missing owner")?;
        assert!(checker.type_info.direct_input_admission(&owner, function)?.is_some());
        let InputFlow::Checked { graph, analysis } = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or("missing flow")?
        else {
            return Err("missing checked flow".into());
        };
        let nested_completion = graph
            .invocations
            .values()
            .find(|call| call.callee.declaration_name == "produce")
            .ok_or("missing nested call")?
            .completion_node;
        graph
            .invocations
            .values_mut()
            .find(|call| call.callee.declaration_name == "consume")
            .ok_or("missing outer call")?
            .arguments[0]
            .evaluation
            .start_node = nested_completion;
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty());
        assert!(
            checker.type_info.direct_input_admission(&owner, function).is_err(),
            "accepted argument interval omitting its nested evaluation"
        );
        let mut facts = checked(source)?.type_info;
        let InputFlow::Checked { graph, .. } = facts
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or("missing flow")?
        else {
            return Err("missing checked flow".into());
        };
        let (nested_span, nested) = graph
            .invocations
            .iter_mut()
            .find(|(_, call)| call.callee.declaration_name == "produce")
            .ok_or("missing nested call")?;
        let nested_span = *nested_span;
        nested.arguments.clear();
        facts
            .protocols
            .explicit_input_calls
            .get_mut(&nested_span)
            .ok_or("missing selected call")?
            .parameters
            .clear();
        assert!(
            facts.checked_input_body(&owner).is_err(),
            "accepted invocation without its canonical explicit input slot"
        );
        Ok(())
    }

    /// Explicit Owned requests copy trivial values; a persisted event cannot silently turn that read into a move.
    #[test]
    fn invocation_transfer_kind_uses_checked_copy_facts() -> Result<(), Box<dyn std::error::Error>> {
        let mut checker = checked(
            "def read(value: Owned[int]) -> int:\n    return value\ndef use(value: int) -> int:\n    return read(value)\n",
        )?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "use")
            .cloned()
            .ok_or("missing owner")?;
        let InputFlow::Checked { graph, analysis } = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or("missing flow")?
        else {
            return Err("missing checked flow".into());
        };
        let source_node = graph
            .invocations
            .values()
            .next()
            .and_then(|call| call.arguments.first())
            .and_then(|argument| argument.evaluation.source_node)
            .ok_or("missing Copy source event")?;
        let flow::Event::Access { access, .. } = &mut graph.nodes[source_node].event else {
            return Err("missing Copy access".into());
        };
        *access = flow::Access::Move;
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty());
        assert!(
            checker.type_info.checked_input_body(&owner).is_err(),
            "accepted consuming event for Copy input"
        );
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
        let source = "def consume(value: Owned[str]) -> bool:\n    return value == \"x\"\ndef use() -> None:\n    value = \"x\"\n    result = consume(value)\n    print(value)\n";
        for erased in [flow::Access::Move, flow::Access::Read] {
            let mut checker = checked(source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0126"))
            );
            let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
            let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
            let function = program
                .declarations
                .iter()
                .find_map(|declaration| match &declaration.node {
                    crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
                    _ => None,
                })
                .ok_or("missing use function")?;
            let owner = checker
                .type_info
                .protocols
                .explicit_input_flow
                .keys()
                .find(|owner| owner.declaration_name == "use")
                .cloned()
                .ok_or("missing use owner")?;
            let InputFlow::Checked { graph, analysis } = checker
                .type_info
                .protocols
                .explicit_input_flow
                .get_mut(&owner)
                .ok_or("missing use flow")?
            else {
                return Err("missing checked flow".into());
            };
            let transfer = graph
                .nodes
                .iter_mut()
                .find(|node| {
                    matches!(
                        node.event,
                        flow::Event::Access {
                            access,
                            ..
                        } if access == erased
                    )
                })
                .ok_or("missing transfer event")?;
            transfer.event = flow::Event::Nothing;
            *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
            assert!(analysis.conflicts.is_empty(), "{analysis:?}");
            assert!(
                checker.type_info.direct_input_admission(&owner, function).is_err(),
                "accepted stored owner with erased {erased:?} event"
            );
        }
        Ok(())
    }

    /// A persisted replacement cannot resurrect a consumed owner when the source only initializes another local.
    /// Actual reassignment remains valid and resets the moved state after its value has been evaluated.
    #[test]
    fn source_access_cannot_invent_reinitialization() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def consume(value: Owned[str]) -> None:\n    pass\ndef use() -> None:\n    mut value = \"old\"\n    consume(value)\n    unrelated = \"safe\"\n    print(value)\n";
        let mut checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{:?}",
            checker.errors
        );
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let function = program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
                _ => None,
            })
            .ok_or("missing use function")?;
        let replacement_span = function
            .body
            .iter()
            .find_map(|statement| match &statement.node {
                crate::ast::Statement::Assignment(assignment) if assignment.name == "unrelated" => {
                    Some(assignment.name_span)
                }
                _ => None,
            })
            .ok_or("missing unrelated declaration")?;
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "use")
            .cloned()
            .ok_or("missing owner")?;
        let InputFlow::Checked { graph, analysis } = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or("missing flow")?
        else {
            return Err("missing checked flow".into());
        };
        let (read, place) = graph
            .nodes
            .iter()
            .enumerate()
            .find_map(|(index, node)| match &node.event {
                flow::Event::Access {
                    place,
                    access: flow::Access::Read,
                    through: None,
                } => Some((index, place.clone())),
                _ => None,
            })
            .ok_or("missing later source read")?;
        graph.nodes.insert(
            read,
            flow::Node {
                event: flow::Event::Access {
                    place,
                    access: flow::Access::Replace,
                    through: None,
                },
                span: replacement_span,
                successors: Vec::new(),
            },
        );
        let count = graph.nodes.len();
        for (index, node) in graph.nodes.iter_mut().enumerate() {
            node.successors = if index + 1 < count { vec![index + 1] } else { Vec::new() };
        }
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty());
        assert!(
            checker.type_info.direct_input_admission(&owner, function).is_err(),
            "accepted an invented replacement of another source owner"
        );
        let valid = checked(
            "def consume(value: Owned[str]) -> None:\n    pass\ndef use() -> None:\n    mut value = \"old\"\n    consume(value)\n    value = \"new\"\n    print(value)\n",
        )?;
        assert!(valid.errors.is_empty(), "{:?}", valid.errors);
        Ok(())
    }

    /// Ordinary source accesses retain exact multiplicity and producer order even when neither event changes
    /// liveness. Recomputing an otherwise harmless Copy-only graph cannot authorize those mutations.
    #[test]
    fn source_accesses_preserve_multiplicity_and_order() -> Result<(), Box<dyn std::error::Error>> {
        let source = "def read(left: Borrowed[int], right: Borrowed[int]) -> int:\n    return left + right\n";
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let function = program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                crate::ast::Declaration::Function(function) => Some(function),
                _ => None,
            })
            .ok_or("missing function")?;
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .next()
            .ok_or("missing owner")?;
        for duplicate in [false, true] {
            let mut facts = checker.type_info.clone();
            let InputFlow::Checked { graph, analysis } = facts
                .protocols
                .explicit_input_flow
                .get_mut(owner)
                .ok_or("missing flow")?
            else {
                return Err("missing checked flow".into());
            };
            let reads = graph
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(index, node)| {
                    matches!(
                        node.event,
                        flow::Event::Access {
                            access: flow::Access::Read,
                            ..
                        }
                    )
                    .then_some(index)
                })
                .collect::<Vec<_>>();
            let [left, right] = reads.as_slice() else {
                return Err("expected exactly two source reads".into());
            };
            if duplicate {
                graph.nodes.insert(*left + 1, graph.nodes[*left].clone());
            } else {
                graph.nodes.swap(*left, *right);
            }
            let count = graph.nodes.len();
            for (index, node) in graph.nodes.iter_mut().enumerate() {
                node.successors = if index + 1 < count { vec![index + 1] } else { Vec::new() };
            }
            *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
            assert!(analysis.conflicts.is_empty());
            assert!(
                facts.direct_input_admission(owner, function).is_err(),
                "accepted corrupted ordinary reads (duplicate: {duplicate})"
            );
        }
        Ok(())
    }

    /// Access coverage must preserve source Copy evidence for nominal derives instead of treating a valid read
    /// as a required transfer merely because a legacy ABI projection cannot describe the nominal layout.
    #[test]
    fn source_access_retains_checked_copy_derives() -> Result<(), Box<dyn std::error::Error>> {
        for (index, source) in [
            "def derived(value: Owned[Point]) -> Point:\n    first = value\n    return value\n@derive(Copy)\nmodel Point:\n    x: int\n",
            "def optional(value: Owned[Option[int]]) -> Option[int]:\n    first = value\n    return value\n",
            "def use(value: Owned[Point]) -> int:\n    return consume(value)\ndef consume(value: Owned[Point]) -> int:\n    return value.x\n@derive(Copy)\nmodel Point:\n    x: int\n",
            "def use(value: Owned[Point]) -> Point:\n    let local: Owned[Point] = value\n    return local\n@derive(Copy)\nmodel Point:\n    x: int\n",
            "def equal(value: Borrowed[Point]) -> bool:\n    return value == value\n@derive(Copy, PartialEq)\nmodel Point:\n    x: int\n",
        ].into_iter().enumerate() {
            let checker = checked(source)?;
            if matches!(index, 2 | 3) {
                assert!(!checker.errors.is_empty());
                assert!(checker.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")
                    && error.message.contains("compatible native input carrier")), "{source}\n{:?}", checker.errors);
            } else {
                assert!(checker.errors.is_empty(), "{source}\n{:?}", checker.errors);
            }
            for owner in checker.type_info.protocols.explicit_input_flow.keys() {
                assert!(checker.type_info.checked_input_body(owner)?.is_some());
            }
        }
        Ok(())
    }

    /// Exact owner/type decisions survive independently of graph events. Missing legacy facts, another owner's
    /// facts and a valid recomputed graph that changes a Copy read into a move all remain refused.
    #[test]
    fn source_copy_evidence_is_independent_of_access_events() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "def derived(value: Owned[Point]) -> Point:\n    return value\n@derive(Copy)\nmodel Point:\n    x: int\n";
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{:?}", checker.errors);
        let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|error| format!("{error:?}"))?)
            .map_err(|error| format!("{error:?}"))?;
        let declaration = program.declarations.first().ok_or("missing function")?;
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return Err("expected function".into());
        };
        let owner = checker
            .type_info
            .declarations
            .function_bindings_by_span
            .get(&declaration.span)
            .and_then(|binding| binding.identity.as_ref())
            .ok_or("missing function identity")?;
        for corruption in 0..4 {
            let mut facts = checker.type_info.clone();
            match corruption {
                0 => facts.protocols.explicit_input_copy.clear(),
                1 => {
                    let decisions = facts
                        .protocols
                        .explicit_input_copy
                        .remove(owner)
                        .ok_or("missing decisions")?;
                    let mut other = owner.clone();
                    other.declaration_name = "other".into();
                    facts.protocols.explicit_input_copy.insert(other, decisions);
                }
                2 => {
                    let decisions = facts
                        .protocols
                        .explicit_input_copy
                        .get_mut(owner)
                        .ok_or("missing decisions")?;
                    decisions.clear();
                    decisions.insert(ResolvedType::Str, true);
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
                    let access = graph
                        .nodes
                        .iter_mut()
                        .find_map(|node| match &mut node.event {
                            flow::Event::Access { access, .. } => Some(access),
                            _ => None,
                        })
                        .ok_or("missing value read")?;
                    *access = flow::Access::Move;
                    *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
                    assert!(analysis.conflicts.is_empty());
                }
            }
            assert!(
                facts.direct_input_admission(owner, function).is_err(),
                "accepted Copy corruption {corruption}"
            );
        }
        Ok(())
    }

    /// Cleanup subjects follow actual authenticated non-Copy transfers, including ordinary locals and explicit
    /// Owned initializers. Copy calls and shared observations cannot opt an unrelated owner into this policy.
    #[test]
    fn conditional_transfer_inventory_uses_actual_source_events() -> Result<(), Box<dyn std::error::Error>> {
        for (source, expected) in [
            (
                "def choose(flag: bool) -> None:\n    value = \"x\"\n    if flag:\n        consume(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                vec!["value"],
            ),
            (
                "def choose(value: Owned[str], flag: bool) -> None:\n    if flag:\n        consume(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                vec!["value"],
            ),
            (
                "def choose(flag: bool) -> None:\n    let value: Owned[str] = \"x\"\n    if flag:\n        consume(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                vec!["value"],
            ),
            (
                "def choose(flag: bool) -> None:\n    source = \"x\"\n    if flag:\n        let destination: Owned[str] = source\n",
                vec!["source"],
            ),
            (
                "def choose(flag: bool) -> None:\n    value = 1\n    if flag:\n        consume(value)\ndef consume(value: Owned[int]) -> None:\n    pass\n",
                vec![],
            ),
            (
                "def choose(flag: bool) -> None:\n    value = \"x\"\n    if flag:\n        read(value)\ndef read(value: Borrowed[str]) -> None:\n    pass\n",
                vec![],
            ),
        ] {
            let checker = checked(source)?;
            assert!(
                checker
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0123")),
                "{source}\n{:?}",
                checker.errors
            );
            let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|error| format!("{error:?}"))?)
                .map_err(|error| format!("{error:?}"))?;
            let declaration = program.declarations.first().ok_or("missing function")?;
            let crate::ast::Declaration::Function(function) = &declaration.node else {
                return Err("expected function".into());
            };
            let owner = checker
                .type_info
                .declarations
                .function_bindings_by_span
                .get(&declaration.span)
                .and_then(|binding| binding.identity.as_ref())
                .ok_or("missing owner")?;
            let proof = checker.type_info.checked_input_body(owner)?.ok_or("missing proof")?;
            let subjects = proof.whole_transfer_subjects(function)?;
            assert_eq!(
                subjects
                    .iter()
                    .map(|subject| subject.declaration_name.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "{source}"
            );
            assert!(
                checker.type_info.direct_input_admission(owner, function).is_err(),
                "inventory unexpectedly enabled conditional execution"
            );
        }
        Ok(())
    }

    /// Transfer subjects cannot come from a graph that omits a later source use or invents a Move for another
    /// owner. Both mutations can produce conflict-free graph analysis and still lack complete source authority.
    #[test]
    fn conditional_transfer_inventory_requires_complete_accesses() -> Result<(), Box<dyn std::error::Error>> {
        for (source, erased) in [
            (
                "def choose(flag: bool) -> None:\n    value = \"x\"\n    if flag:\n        consume(value)\n    print(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                true,
            ),
            (
                "def choose(flag: bool) -> None:\n    value = \"x\"\n    other = \"y\"\n    if flag:\n        consume(value)\n    print(other)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
                false,
            ),
        ] {
            let checker = checked(source)?;
            let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|error| format!("{error:?}"))?)
                .map_err(|error| format!("{error:?}"))?;
            let declaration = program.declarations.first().ok_or("missing function")?;
            let crate::ast::Declaration::Function(function) = &declaration.node else {
                return Err("expected function".into());
            };
            let mut facts = checker.type_info;
            let owner = facts
                .declarations
                .function_bindings_by_span
                .get(&declaration.span)
                .and_then(|binding| binding.identity.clone())
                .ok_or("missing owner")?;
            let InputFlow::Checked { graph, analysis } = facts
                .protocols
                .explicit_input_flow
                .get_mut(&owner)
                .ok_or("missing flow")?
            else {
                return Err("expected checked graph".into());
            };
            let read = graph.nodes.iter_mut().rev().find(|node| matches!(&node.event,
                flow::Event::Access { place, access: flow::Access::Read, .. }
                    if matches!(&place.root, flow::Root::Binding(identity) if identity.declaration_name == if erased { "value" } else { "other" })))
                .ok_or("missing ordinary read")?;
            if erased {
                read.event = flow::Event::Nothing;
            } else if let flow::Event::Access { access, .. } = &mut read.event {
                *access = flow::Access::Move;
            }
            *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
            assert!(analysis.conflicts.is_empty());
            let proof = facts.checked_input_body(&owner)?.ok_or("missing body view")?;
            assert!(
                proof.whole_transfer_subjects(function).is_err(),
                "accepted corrupted conditional inventory (erased: {erased})"
            );
        }
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
                "def read(value: Borrowed[int], other: Borrowed[int]) -> Borrowed[int] from value:\n    return other\n",
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

    /// Loops over explicit inputs execute under the conditional profile: a loop reads each item of a borrowed input,
    /// and an owned input or local may be transferred inside a loop whose break leaves it before a second iteration.
    #[test]
    fn explicit_loops_execute_under_the_conditional_profile() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "def total(values: Borrowed[list[int]]) -> int:\n    mut sum = 0\n    for value in values:\n        sum += value\n    return sum\n",
            "def consume(text: Owned[str]) -> None:\n    pass\ndef maybe(text: Owned[str], rounds: Borrowed[list[int]]) -> None:\n    for _round in rounds:\n        consume(text)\n        break\n",
            "def consume(value: Owned[str]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    value = \"owner\"\n    if flag:\n        while flag:\n            consume(value)\n            break\n",
        ] {
            let checker = checked(source)?;
            assert!(checker.errors.is_empty(), "{source}: {:?}", checker.errors);
        }
        Ok(())
    }

    /// `?` continues with the success payload and leaves on the error, like a `return`; `Ok`, `Err` and `Some`
    /// construct a fresh carrier from an owned payload; a `match` on a fresh carrier starts every arm from the
    /// evaluated scrutinee and binds its payload as a fresh owner.
    #[test]
    fn explicit_bodies_propagate_construct_and_match_results() -> Result<(), Box<dyn std::error::Error>> {
        let declarations = "def size(text: Owned[str]) -> Result[int, str]:\n    if len(text) == 0:\n        return Err(\"empty\")\n    return Ok(len(text))\n";
        for body in [
            "def both(text: Owned[str], other: Owned[str]) -> Result[int, str]:\n    first = size(text)?\n    second = size(other)?\n    return Ok(first + second)\n",
            "def describe(text: Owned[str]) -> str:\n    match size(text):\n        Ok(count) => return f\"{count}\"\n        Err(message) => return message\n",
            "def keep(value: Owned[str]) -> None:\n    pass\ndef route(text: Owned[str], other: Owned[str]) -> None:\n    match size(text):\n        Ok(_) => keep(other)\n        Err(_) => pass\n",
            "def maybe(text: Owned[str], keep: bool) -> Option[str]:\n    if keep:\n        return Some(f\"{text}!\")\n    return None\n",
        ] {
            let checker = checked(&format!("{declarations}{body}"))?;
            assert!(
                checker.errors.is_empty(),
                "{body}: {:?} {:?}",
                checker.errors,
                flow_summary(&checker)
            );
        }
        let refused = checked(&format!(
            "{declarations}def twice(text: Owned[str]) -> Result[int, str]:\n    first = size(text)?\n    second = size(text)?\n    return Ok(first + second)\n"
        ))?;
        assert!(
            refused
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{:?}",
            refused.errors
        );
        // Moving a stored owner into a carrier waits for constructed values' transfer materialization, as models do.
        let staged = checked("def wrap(text: Owned[str]) -> Option[str]:\n    return Some(text)\n")?;
        assert!(
            staged
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "{:?}",
            staged.errors
        );
        Ok(())
    }

    /// RFC 164's fallible buffer: a view carried in a `Result` comes out of its carrier with `?`, chains into the next
    /// call, propagates its error through every caller, and leaves the owner usable for recovery once the failed
    /// operation's borrows end. A carried view must still carry its declared source place.
    #[test]
    fn carried_views_propagate_and_recover() -> Result<(), Box<dyn std::error::Error>> {
        let shared = checked(
            "def view(value: Borrowed[str]) -> Result[Borrowed[str], str] from value:\n    return Ok(value)\n",
        )?;
        assert!(
            shared.errors.is_empty(),
            "{:?} {:?}",
            shared.errors,
            flow_summary(&shared)
        );
        let buffer = "class SampleBuffer:\n    pub capacity: int\n    pub samples: list[int]\n\n    def append(mut self, sample: int) -> Result[Reference[Self], str] from self:\n        if len(self.samples) >= self.capacity:\n            return Err(\"full\")\n        self.samples.append(sample)\n        return Ok(self)\n\n    def append_all(mut self, samples: Borrowed[list[int]]) -> Result[Reference[Self], str] from self:\n        for sample in samples:\n            self.append(sample)?\n        return Ok(self)\n\n";
        let accepted = checked(&format!(
            "{buffer}def fill(mut buffer: BorrowedMut[SampleBuffer]) -> Result[None, str]:\n    (buffer.append(10)?).append_all([20, 30])?\n    return Ok(None)\n\ndef recover() -> Result[None, str]:\n    mut buffer = SampleBuffer(capacity=2, samples=[])\n    match fill(buffer):\n        Ok(_) => println(\"All samples added\")\n        Err(error) =>\n            println(error)\n            assert len(buffer.samples) == 2\n            buffer.capacity = 3\n            buffer.append(30)?\n    assert len(buffer.samples) == 3\n    return Ok(None)\n"
        ))?;
        assert!(
            accepted.errors.is_empty(),
            "{:?} {:?}",
            accepted.errors,
            flow_summary(&accepted)
        );
        // Every call and `?` sees the projected value type, in the checked expression types and in every call-result
        // record Body IR reads: no type still carries the written mode.
        let leaked = accepted
            .type_info
            .expressions
            .expr_types
            .iter()
            .chain(&accepted.type_info.expressions.body_ir_expr_types)
            .chain(&accepted.type_info.calls.body_ir_call_result_types)
            .filter(|(_, ty)| format!("{ty:?}").contains("Reference"))
            .map(|(span, ty)| format!("{span:?}: {ty:?}"))
            .collect::<Vec<_>>();
        assert!(leaked.is_empty(), "{leaked:?}");
        let refused = checked(&format!(
            "{buffer}def other(mut buffer: BorrowedMut[SampleBuffer], mut spare: BorrowedMut[SampleBuffer]) -> Result[Reference[SampleBuffer], str] from buffer:\n    return Ok(spare)\n"
        ))?;
        assert!(
            refused
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0124")),
            "{:?}",
            refused.errors
        );
        Ok(())
    }

    /// A builtin list method changes or reads its receiver place in place: an exclusive receiver's field may grow,
    /// while a shared input cannot be changed through its borrow.
    #[test]
    fn explicit_bodies_call_builtin_list_methods_on_places() -> Result<(), Box<dyn std::error::Error>> {
        let accepted = checked(
            "class SampleBuffer:\n    pub capacity: int\n    pub samples: list[int]\n\n    def add(mut self, sample: Borrowed[int]) -> bool:\n        self.samples.append(sample)\n        return self.samples.contains(sample)\n\ndef main() -> None:\n    mut buffer = SampleBuffer(capacity=2, samples=[])\n    buffer.add(10)\n    println(len(buffer.samples))\n",
        )?;
        assert!(
            accepted.errors.is_empty(),
            "{:?} {:?}",
            accepted.errors,
            flow_summary(&accepted)
        );
        let refused = checked("def grow(values: Borrowed[list[int]]) -> None:\n    values.append(1)\n")?;
        assert!(
            refused
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{:?} {:?}",
            refused.errors,
            flow_summary(&refused)
        );
        Ok(())
    }

    /// A plain helper without arguments is an ordinary helper call like one with Copy arguments: a checked body may
    /// read its scalar result.
    #[test]
    fn checked_bodies_call_plain_helpers_without_arguments() -> Result<(), Box<dyn std::error::Error>> {
        let checker = checked(
            "def seven() -> int:\n    return 7\ndef take(text: Owned[str]) -> None:\n    pass\ndef main() -> None:\n    before = seven()\n    take(\"x\")\n    println(before)\n",
        )?;
        assert!(
            checker.errors.is_empty(),
            "{:?} {:?}",
            checker.errors,
            flow_summary(&checker)
        );
        Ok(())
    }

    /// Printing a list of primitives reaches no nominal display hook, so a checked caller may print the list an
    /// exclusive input replaced. A list of models may display through derived or user hooks and stays refused.
    #[test]
    fn checked_callers_print_lists_of_primitives() -> Result<(), Box<dyn std::error::Error>> {
        let accepted = checked(
            "def collapse(mut values: BorrowedMut[list[int]]) -> None:\n    values = [9]\ndef main() -> None:\n    mut values = [1, 2, 3]\n    collapse(values)\n    println(values)\n",
        )?;
        assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
        let refused = checked(
            "class Tag:\n    pub name: str\ndef keep(mut tags: BorrowedMut[list[Tag]]) -> None:\n    pass\ndef main() -> None:\n    mut tags = [Tag(name=\"a\")]\n    keep(tags)\n    println(tags)\n",
        )?;
        assert!(
            refused
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "{:?}",
            refused.errors
        );
        Ok(())
    }

    /// A transfer inside a loop that can reach another iteration uses an owner the first iteration already moved:
    /// the back edge from the body's end, and from a `continue`, carries the transfer to the next one.
    #[test]
    fn explicit_loops_refuse_a_transfer_another_iteration_repeats() -> Result<(), Box<dyn std::error::Error>> {
        let declarations = "def consume(text: Owned[str]) -> None:\n    pass\n";
        for body in [
            "def again(text: Owned[str], rounds: Borrowed[list[int]]) -> None:\n    for _round in rounds:\n        consume(text)\n",
            "def again(text: Owned[str], flag: bool) -> None:\n    while flag:\n        consume(text)\n",
            "def again(text: Owned[str], flag: bool) -> None:\n    while flag:\n        consume(text)\n        if flag:\n            continue\n        break\n",
        ] {
            let refused = checked(&format!("{declarations}{body}"))?;
            assert!(
                refused
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0126")),
                "{body}: {:?}",
                refused.errors
            );
        }
        Ok(())
    }

    /// Reannotation remains outside admission. A method body's alias, an alias returned as the declared view source
    /// and a conditional owned local (under the conditional profile) are admitted.
    #[test]
    fn local_alias_admission_preserves_other_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        for source in [
            "class Counter:\n    value: int\n    def read(self) -> int:\n        let local: Borrowed[int] = self.value\n        return local\n",
            "def read(value: Borrowed[str]) -> Borrowed[str] from value:\n    let local: Borrowed[str] = value\n    return local\n",
            "def read(value: Owned[int], choose: bool) -> int:\n    if choose:\n        let local: Owned[int] = value\n        return local\n    return value\n",
        ] {
            let checker = checked(source)?;
            assert!(checker.errors.is_empty(), "{source}: {:?}", checker.errors);
        }
        for source in ["def wrong() -> int:\n    mut value = 1\n    value: BorrowedMut[int] = 2\n    return value\n"] {
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
    /// Both returned views are executable.
    #[test]
    fn receiver_contracts_cover_abstract_and_concrete_methods() -> Result<(), Box<dyn std::error::Error>> {
        for (source, fields) in [
            ("pub trait Reader:\n    def view(self) -> Borrowed[Self] from self\n", 0),
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
            assert!(checker.errors.is_empty(), "{source}: {:?}", checker.errors);
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

    /// A method frame is checked like a function, and its caller's receiver loan conflicts with an overlapping
    /// explicit argument while a disjoint field argument stays accepted.
    #[test]
    fn method_frames_check_receiver_and_argument_overlap() -> Result<(), Box<dyn std::error::Error>> {
        let declarations = "class Counter:\n    pub value: int\n    pub step: int\n\n    def bump(mut self, mut amount: BorrowedMut[int]) -> None:\n        self.value = self.value + amount\n";
        let accepted = checked(&format!(
            "{declarations}\ndef main() -> int:\n    mut counter = Counter(value=1, step=2)\n    mut step = 2\n    counter.bump(step)\n    return counter.value\n"
        ))?;
        assert!(
            accepted.errors.is_empty(),
            "{:?} {:?}",
            accepted.errors,
            accepted
                .type_info
                .protocols
                .explicit_input_flow
                .values()
                .filter_map(|flow| match flow {
                    InputFlow::Unavailable(reason) => Some(reason.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        );
        let overlapping = checked(&format!(
            "{declarations}\ndef main() -> int:\n    mut counter = Counter(value=1, step=2)\n    counter.bump(counter.step)\n    return counter.value\n"
        ))?;
        assert!(
            overlapping
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{:?}",
            overlapping.errors
        );
        Ok(())
    }

    /// The RFC's returned field views: a method may hold the whole receiver while running, and its result retains
    /// only the declared field, so a disjoint sibling edit is accepted while overlapping edits, owner replacement,
    /// wrong sources, fresh values and manufactured exclusive access are refused.
    #[test]
    fn returned_field_views_retain_only_their_declared_source() -> Result<(), Box<dyn std::error::Error>> {
        let declarations = "class Address:\n    pub city: str\n\n    def set_city(mut self, city: str) -> Reference[Self] from self:\n        self.city = city\n        return self\n\nclass Customer:\n    pub name: str\n    pub address: Address\n\n    def edit_address(mut self) -> Reference[Address] from self.address:\n        return self.address\n";
        let accepted = checked(&format!(
            "{declarations}\ndef main() -> None:\n    mut customer = Customer(name=\"Sam\", address=Address(city=\"Utrecht\"))\n    mut address = customer.edit_address()\n    customer.name = \"Samuel\"\n    address.set_city(\"Rotterdam\")\n    assert customer.address.city == \"Rotterdam\"\n"
        ))?;
        assert!(
            accepted.errors.is_empty(),
            "{:?} {:?}",
            accepted.errors,
            flow_summary(&accepted)
        );
        for (body, code) in [
            (
                "    mut address = customer.edit_address()\n    customer.address.city = \"Amsterdam\"\n    address.set_city(\"Rotterdam\")\n",
                "INCAN-T0126",
            ),
            (
                "    mut address = customer.edit_address()\n    customer = Customer(name=\"Alex\", address=Address(city=\"Amsterdam\"))\n    address.set_city(\"Rotterdam\")\n",
                "INCAN-T0126",
            ),
        ] {
            let refused = checked(&format!(
                "{declarations}\ndef main() -> None:\n    mut customer = Customer(name=\"Sam\", address=Address(city=\"Utrecht\"))\n{body}"
            ))?;
            assert!(
                refused.errors.iter().any(|error| error.stable_code() == Some(code)),
                "{body}: {:?} {:?}",
                refused.errors,
                flow_summary(&refused)
            );
        }
        for source in [
            "class Address:\n    pub city: str\n    pub street: str\n    def view(mut self) -> Reference[str] from self.city:\n        return self.street\n",
            "class Address:\n    pub city: str\n    def view(mut self) -> Reference[Address] from self:\n        return Address(city=\"Utrecht\")\n",
            "class Address:\n    pub city: str\n    def view(self) -> Reference[Address] from self:\n        return self\n",
            "class Address:\n    pub city: str\ndef view(address: Owned[Address]) -> Borrowed[Address] from address:\n    return address\n",
        ] {
            let refused = checked(source)?;
            assert!(
                refused
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0124")),
                "{source}: {:?}",
                refused.errors
            );
        }
        Ok(())
    }

    /// Each body's flow outcome, for assertion messages.
    fn flow_summary(checker: &TypeChecker) -> Vec<(String, String)> {
        checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .map(|(owner, flow)| {
                let outcome = match flow {
                    InputFlow::Checked { analysis, .. } => format!("checked {:?}", analysis.conflicts),
                    InputFlow::Unavailable(reason) => format!("unavailable {reason:?}"),
                    InputFlow::Invalid { fault, .. } => format!("invalid {fault:?}"),
                };
                (owner.declaration_name.clone(), outcome)
            })
            .collect()
    }
}
