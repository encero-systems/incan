//! Source control-flow projection for explicit inputs. Unsupported expressions refuse this profile; an empty graph
//! is never substituted for missing call, capture, suspension or returned-dependency facts.

use std::collections::{BTreeMap, BTreeSet};

use super::TypeChecker;
use super::flow::{Access, Event, Graph, Node, Permission, Place, Projection};
use crate::ast::{BinaryOp, BindingKind, Condition, Expr, FunctionDecl, Span, Spanned, Statement};
use crate::symbols::SymbolKind;
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

/// A source shape whose access behavior has not been projected. The anchor is the unsupported operation itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Unavailable {
    pub span: Span,
    pub reason: &'static str,
}

/// One enclosing loop owns its back-edge and the paths that leave it; nested loops cannot steal each other's exits.
struct LoopEdges {
    header: usize,
    breaks: Vec<usize>,
}

/// Invocation-local projection state, borrowing immutable checked identities and types rather than resolving names.
struct Builder<'a> {
    checker: &'a TypeChecker,
    graph: Graph,
    locals: BTreeMap<CanonicalSymbolId, Permission>,
    constrained: BTreeSet<CanonicalSymbolId>,
    loops: Vec<LoopEdges>,
}

/// Project a checked free-function body against its exact declaration contract. This initial source profile has no
/// calls or local loans yet; those shapes return an explicit unavailable result and cannot authorize execution.
pub(super) fn build(checker: &TypeChecker, function: &FunctionDecl, span: Span) -> Result<Graph, Unavailable> {
    let binding = checker
        .type_info
        .declarations
        .function_bindings_by_span
        .get(&span)
        .ok_or(Unavailable {
            span,
            reason: "missing checked function header",
        })?;
    let identity = binding.identity.as_ref().ok_or(Unavailable {
        span,
        reason: "missing selected declaration identity",
    })?;
    let contract = checker
        .type_info
        .declarations
        .ownership_contracts
        .get(identity)
        .ok_or(Unavailable {
            span,
            reason: "no explicit input contract",
        })?;
    if function.is_async()
        || contract.result_source.is_some()
        || contract.modes.iter().any(|mode| {
            mode.component.parameter.is_none()
                || !mode.component.children.is_empty()
                || mode.mode == OwnershipMode::Reference
        })
    {
        return Err(Unavailable {
            span,
            reason: "escaping or nested ownership contract",
        });
    }
    let mut builder = Builder {
        checker,
        locals: checker
            .symbols
            .all_symbols()
            .iter()
            .enumerate()
            .filter_map(|(index, symbol)| {
                let identity = checker.symbols.identity_of(index)?;
                let SymbolKind::Variable(variable) = &symbol.kind else {
                    return None;
                };
                (identity.kind == SemanticSourceTargetKind::Local).then(|| {
                    (
                        identity.clone(),
                        Permission {
                            exclusive: variable.is_mutable,
                            writable: variable.is_mutable,
                            transferable: true,
                        },
                    )
                })
            })
            .collect(),
        graph: Graph {
            permissions: BTreeMap::new(),
            loans: Vec::new(),
            nodes: vec![Node {
                event: Event::Nothing,
                successors: Vec::new(),
                span,
            }],
            entry: 0,
        },
        constrained: BTreeSet::new(),
        loops: Vec::new(),
    };
    for (slot, parameter) in function.params.iter().enumerate() {
        let root = checker
            .type_info
            .resolved_write_identity(parameter.span, &parameter.node.name)
            .or_else(|| {
                let selected = checker.symbols.lookup(&parameter.node.name)?;
                let identity = checker.symbols.identity_of(selected)?;
                (identity.kind == SemanticSourceTargetKind::Parameter && identity.declaration_span == parameter.span)
                    .then_some(identity)
            })
            .ok_or(Unavailable {
                span: parameter.span,
                reason: "missing prepared parameter identity",
            })?
            .clone();
        let mode = checker.checked_input_contract(identity, slot);
        let writable = parameter.node.is_mut && mode.is_none_or(|mode| mode.mode != OwnershipMode::Borrowed);
        let exclusive = mode.map_or(writable, |mode| {
            mode.mode == OwnershipMode::BorrowedMut || (mode.mode == OwnershipMode::Owned && writable)
        });
        builder.graph.permissions.insert(
            root.clone(),
            Permission {
                exclusive,
                writable,
                transferable: mode.is_none_or(|mode| mode.mode == OwnershipMode::Owned),
            },
        );
        if mode.is_some() {
            builder.constrained.insert(root);
        }
    }
    builder.block(&function.body, vec![0])?;
    Ok(builder.graph)
}

impl Builder<'_> {
    /// Connect each current predecessor to one evaluated operation, preserving distinct branch paths.
    fn event(&mut self, incoming: Vec<usize>, event: Event, span: Span) -> Vec<usize> {
        let next = self.graph.nodes.len();
        self.graph.nodes.push(Node {
            event,
            successors: Vec::new(),
            span,
        });
        self.connect(&incoming, next);
        vec![next]
    }

    /// Join already evaluated paths without introducing an operation or a new owner identity.
    fn connect(&mut self, incoming: &[usize], next: usize) {
        for previous in incoming {
            self.graph.nodes[*previous].successors.push(next);
        }
    }

    /// Register a checked local owner once. Parameters must arrive from the prepared declaration, not this fallback.
    fn root(&mut self, identity: &CanonicalSymbolId, span: Span) -> Result<Place, Unavailable> {
        if !self.graph.permissions.contains_key(identity) {
            if identity.kind != SemanticSourceTargetKind::Local {
                return Err(Unavailable {
                    span,
                    reason: "place is not a prepared parameter or checked local",
                });
            }
            let permission = self.locals.get(identity).copied().ok_or(Unavailable {
                span,
                reason: "missing checked local permissions",
            })?;
            self.graph.permissions.insert(identity.clone(), permission);
        }
        Ok(Place {
            root: identity.clone(),
            projection: Vec::new(),
        })
    }

    /// Read only checked storage projections. Properties and native accessors are calls, even when spelled as fields.
    fn place(&mut self, expression: &Spanned<Expr>) -> Result<Place, Unavailable> {
        match &expression.node {
            Expr::Ident(_) => {
                let identity = self
                    .checker
                    .type_info
                    .resolved_identity(expression.span)
                    .ok_or(Unavailable {
                        span: expression.span,
                        reason: "missing checked place identity",
                    })?
                    .clone();
                self.root(&identity, expression.span)
            }
            Expr::Paren(inner) => self.place(inner),
            Expr::Field(base, _) => {
                if self
                    .checker
                    .type_info
                    .computed_property_access(expression.span)
                    .is_some()
                {
                    return Err(Unavailable {
                        span: expression.span,
                        reason: "computed property access",
                    });
                }
                let identity = self
                    .checker
                    .type_info
                    .resolved_identity(expression.span)
                    .filter(|identity| identity.kind == SemanticSourceTargetKind::Field)
                    .ok_or(Unavailable {
                        span: expression.span,
                        reason: "field lacks a checked stored identity",
                    })?
                    .clone();
                let mut place = self.place(base)?;
                place.projection.push(Projection::Field(identity));
                Ok(place)
            }
            Expr::Index(base, index) => {
                let mut place = self.place(base)?;
                place.projection.push(Projection::Element(index.span));
                Ok(place)
            }
            _ => Err(Unavailable {
                span: expression.span,
                reason: "expression is not a stored place",
            }),
        }
    }

    /// Index computations run before access, exactly once; projection lookup itself is not another read of the owner.
    fn evaluate_place(&mut self, expression: &Spanned<Expr>, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        match &expression.node {
            Expr::Ident(_) => Ok(incoming),
            Expr::Paren(base) | Expr::Field(base, _) => self.evaluate_place(base, incoming),
            Expr::Index(base, index) => {
                let incoming = self.evaluate_place(base, incoming)?;
                self.expression(index, incoming, false)
            }
            _ => Err(Unavailable {
                span: expression.span,
                reason: "unsupported place evaluation",
            }),
        }
    }

    /// Evaluate an expression in source order. A checked Copy read is not a transfer; ordinary unconstrained values
    /// retain their existing semantics until the separate ordinary-binding migration ships.
    fn expression(
        &mut self,
        expression: &Spanned<Expr>,
        incoming: Vec<usize>,
        consume: bool,
    ) -> Result<Vec<usize>, Unavailable> {
        if self
            .checker
            .type_info
            .calls
            .resolved_operator_calls
            .contains_key(&expression.span)
        {
            return Err(Unavailable {
                span: expression.span,
                reason: "operator needs selected callable access facts",
            });
        }
        match &expression.node {
            Expr::Ident(_) | Expr::Field(_, _) | Expr::Index(_, _) => {
                let place = self.place(expression)?;
                let incoming = self.evaluate_place(expression, incoming)?;
                let ty = self.checker.type_info.expr_type(expression.span).ok_or(Unavailable {
                    span: expression.span,
                    reason: "missing checked access type",
                })?;
                let access = if consume && self.constrained.contains(&place.root) && !self.checker.is_copy_type(ty) {
                    Access::Move
                } else {
                    Access::Read
                };
                Ok(self.event(
                    incoming,
                    Event::Access {
                        place,
                        access,
                        through: None,
                    },
                    expression.span,
                ))
            }
            Expr::Literal(_) => Ok(incoming),
            Expr::Paren(inner) | Expr::Unary(_, inner) => self.expression(inner, incoming, consume),
            Expr::Binary(left, operation, right) => {
                let incoming = self.expression(left, incoming, false)?;
                let mut outgoing = self.expression(right, incoming.clone(), false)?;
                if matches!(operation, BinaryOp::And | BinaryOp::Or) {
                    outgoing.extend(incoming);
                }
                Ok(outgoing)
            }
            _ => Err(Unavailable {
                span: expression.span,
                reason: "expression needs checked call or dependency access facts",
            }),
        }
    }

    /// Patterns need binding-specific dependency transfer and are unavailable until their checked projection exists.
    fn condition(&mut self, condition: &Condition, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        match condition {
            Condition::Expr(value) => self.expression(value, incoming, false),
            Condition::Let { .. } => Err(Unavailable {
                span: condition.span(),
                reason: "pattern condition dependency",
            }),
        }
    }

    /// Sequence reachable statements. A return/break/continue ends its path; later statements are not successors.
    fn block(
        &mut self,
        statements: &[Spanned<Statement>],
        mut incoming: Vec<usize>,
    ) -> Result<Vec<usize>, Unavailable> {
        for statement in statements {
            if incoming.is_empty() {
                break;
            }
            incoming = self.statement(statement, incoming)?;
        }
        Ok(incoming)
    }

    /// Project explicit branches, loop back-edges and exits. Unsupported statement effects refuse the whole profile.
    fn statement(&mut self, statement: &Spanned<Statement>, incoming: Vec<usize>) -> Result<Vec<usize>, Unavailable> {
        match &statement.node {
            Statement::Pass => Ok(incoming),
            Statement::Expr(value) => self.expression(value, incoming, false),
            Statement::Return(value) => {
                if let Some(value) = value {
                    self.expression(value, incoming, true)?;
                }
                Ok(Vec::new())
            }
            Statement::Assignment(assignment) => {
                if assignment.ty.is_some() {
                    return Err(Unavailable {
                        span: statement.span,
                        reason: "local annotation access obligations",
                    });
                }
                let root = self
                    .checker
                    .type_info
                    .resolved_write_identity(assignment.name_span, &assignment.name)
                    .ok_or(Unavailable {
                        span: assignment.name_span,
                        reason: "missing checked assignment target",
                    })?
                    .clone();
                let previously_known = self.graph.permissions.contains_key(&root);
                let source = self.place(&assignment.value).ok();
                let incoming = self.expression(&assignment.value, incoming, true)?;
                let place = self.root(&root, assignment.name_span)?;
                if !previously_known && !matches!(assignment.binding, BindingKind::Reassign) {
                    if source.is_some_and(|source| self.constrained.contains(&source.root)) {
                        self.constrained.insert(root);
                    }
                    return Ok(incoming);
                }
                Ok(self.event(
                    incoming,
                    Event::Access {
                        place,
                        access: Access::Replace,
                        through: None,
                    },
                    assignment.name_span,
                ))
            }
            Statement::If(branch) => {
                let mut remaining = self.condition(&branch.condition, incoming)?;
                let mut outgoing = self.block(&branch.then_body, remaining.clone())?;
                for (condition, body) in &branch.elif_branches {
                    remaining = self.expression(condition, remaining, false)?;
                    outgoing.extend(self.block(body, remaining.clone())?);
                }
                outgoing.extend(match &branch.else_body {
                    Some(body) => self.block(body, remaining)?,
                    None => remaining,
                });
                Ok(outgoing)
            }
            Statement::While(loop_) => {
                let header = self.event(incoming, Event::Nothing, statement.span)[0];
                let condition = self.condition(&loop_.condition, vec![header])?;
                self.loops.push(LoopEdges {
                    header,
                    breaks: Vec::new(),
                });
                let body = self.block(&loop_.body, condition.clone())?;
                self.connect(&body, header);
                let loop_ = self.loops.pop().ok_or(Unavailable {
                    span: statement.span,
                    reason: "missing loop owner",
                })?;
                Ok(condition.into_iter().chain(loop_.breaks).collect())
            }
            Statement::Loop(loop_) => {
                let header = self.event(incoming, Event::Nothing, statement.span)[0];
                self.loops.push(LoopEdges {
                    header,
                    breaks: Vec::new(),
                });
                let body = self.block(&loop_.body, vec![header])?;
                self.connect(&body, header);
                Ok(self
                    .loops
                    .pop()
                    .ok_or(Unavailable {
                        span: statement.span,
                        reason: "missing loop owner",
                    })?
                    .breaks)
            }
            Statement::Break(None) => {
                self.loops
                    .last_mut()
                    .ok_or(Unavailable {
                        span: statement.span,
                        reason: "break without loop",
                    })?
                    .breaks
                    .extend(incoming);
                Ok(Vec::new())
            }
            Statement::Continue => {
                let header = self
                    .loops
                    .last()
                    .ok_or(Unavailable {
                        span: statement.span,
                        reason: "continue without loop",
                    })?
                    .header;
                self.connect(&incoming, header);
                Ok(Vec::new())
            }
            _ => Err(Unavailable {
                span: statement.span,
                reason: "statement needs additional checked dependency facts",
            }),
        }
    }
}
