//! Checker-owned control-flow facts for explicit input and local place contracts.
//!
//! The graph builder supplies resolved bindings, fields and checked permissions. This layer proves loan conflicts
//! and transfer availability over graph edges; it never infers source legality from native storage or repairs it
//! with cloning. The executable ownership gate stays separate until these facts reach Body IR.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, IncanInputAccess};

mod operations;
pub(in crate::typechecker) use operations::{OperandDependency, OperationInputs};

/// One source storage projection. Dynamic indices conservatively overlap every element projection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::typechecker) enum Projection {
    /// A checked stored field's declaration, independent of its public spelling.
    Field(CanonicalSymbolId),
    /// An element without a checked disjointness proof. The evaluated index identity distinguishes containment;
    /// unknown indices overlap, but a loan of one evaluation cannot authorize access through a different one.
    Element(HirSourceSpan),
}

/// A declared storage owner or a bounded argument temporary. Temporary identities retain the real expression,
/// invocation and callable ancestry; they are not invented declarations and cannot escape into local bindings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::typechecker) enum Root {
    /// Canonical parameter, receiver or local declaration with checked binding permissions.
    Binding(CanonicalSymbolId),
    /// Fresh argument storage bounded by one invocation; it never supplies a declaration identity.
    Temporary {
        owner: CanonicalSymbolId,
        expression: HirSourceSpan,
        invocation: HirSourceSpan,
    },
}

/// Source place identity; lexical shadowing changes the root even when the spelling is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::typechecker) struct Place {
    pub root: Root,
    pub projection: Vec<Projection>,
}

impl Place {
    /// A prefix owns its descendants; differing canonical stored fields are proven disjoint.
    fn contains(&self, other: &Self) -> bool {
        self.root == other.root && other.projection.starts_with(&self.projection)
    }

    /// Conservatively overlap roots and dynamic elements, distinguishing only proven sibling fields.
    fn overlaps(&self, other: &Self) -> bool {
        self.root == other.root
            && self
                .projection
                .iter()
                .zip(&other.projection)
                .all(|(left, right)| !matches!((left, right), (Projection::Field(a), Projection::Field(b)) if a != b))
    }
}

/// Checked permissions are independent: exclusive access does not itself grant writes or ownership transfer.
#[derive(Debug, Clone, Copy)]
pub(in crate::typechecker) struct Permission {
    pub exclusive: bool,
    pub writable: bool,
    pub transferable: bool,
}

/// Per-callable loan identity, allocated once for a resolved borrow expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::typechecker) struct LoanId(pub usize);

/// Shared or exclusive access, with mutation permission stored independently on the loan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::typechecker) enum LoanKind {
    Shared,
    Exclusive,
}

/// A nonescaping dependency created at a graph node, optionally reborrowed through another loan.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct Loan {
    pub source: Place,
    pub kind: LoanKind,
    pub writable: bool,
    pub parent: Option<LoanId>,
    pub span: HirSourceSpan,
}

/// The operation's checked value semantics; Copy reads never masquerade as consuming transfers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::typechecker) enum Access {
    Read,
    Write,
    Move,
    Replace,
}

/// A resolved ownership event. Evaluation order and branch/loop edges belong to the source graph builder.
#[derive(Debug, Clone)]
pub(in crate::typechecker) enum Event {
    Nothing,
    Borrow(LoanId),
    Access {
        place: Place,
        access: Access,
        through: Option<LoanId>,
    },
}

/// One operation and its actual successors; an empty successor set terminates this path.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct Node {
    pub event: Event,
    pub successors: Vec<usize>,
    pub span: HirSourceSpan,
}

/// Producer-owned boundaries cover the entire evaluation, including fresh values and nested calls. A stored
/// value or borrow additionally names its final source event; the consumer must never recover it by span search.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct InputEvaluation {
    pub start_node: usize,
    pub end_node: usize,
    pub source_node: Option<usize>,
}

/// One evaluated source argument retains its declaration slot and the loan created for that invocation.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct InvocationArgument {
    pub span: HirSourceSpan,
    pub slot: usize,
    pub written_position: usize,
    pub access: IncanInputAccess,
    pub loan: Option<LoanId>,
    pub source: Option<Place>,
    pub evaluation: InputEvaluation,
}

/// Evaluation order, original written positions and receiver borrowing belong to this exact selected invocation.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct Invocation {
    pub callee: CanonicalSymbolId,
    pub receiver: Option<LoanId>,
    /// A native by-value receiver has an evaluation and optional source but no borrow loan.
    pub receiver_owned: bool,
    pub receiver_source: Option<Place>,
    pub arguments: Vec<InvocationArgument>,
    pub receiver_evaluation: Option<InputEvaluation>,
    pub completion_node: usize,
    pub final_accesses: Vec<usize>,
}

/// An explicit owned declaration retains its initializer and the exact event reading or transferring a stored
/// source. Fresh computed values have neither a source place nor an access node; their evaluation remains in CFG.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct OwnedInitialization {
    pub initializer: HirSourceSpan,
    pub source: Option<Place>,
    pub access_node: Option<usize>,
}

/// Stored initializer events belong to one explicit destination and must read/move its exact source within the
/// initializer's expansion-bearing range. This validates retained links; it never discovers them by searching spans.
fn validate_owned_initializations(graph: &Graph) -> Result<(), InvalidGraph> {
    let mut sources = BTreeSet::new();
    for (destination, initializer) in &graph.owned_locals {
        if destination.kind != incan_semantics_core::SemanticSourceTargetKind::Local
            || graph.local_loans.contains_key(destination)
            || graph
                .permissions
                .get(&Root::Binding(destination.clone()))
                .is_none_or(|permission| !permission.transferable)
        {
            return Err(InvalidGraph("owned initializer has no independent local owner"));
        }
        match (&initializer.source, initializer.access_node) {
            (None, None) => {}
            (Some(source), Some(index)) => {
                if matches!(&source.root, Root::Binding(root) if root == destination)
                    || !graph.permissions.contains_key(&source.root)
                    || !sources.insert(index)
                {
                    return Err(InvalidGraph("owned initializer has an invalid or reused source access"));
                }
                let node = graph
                    .nodes
                    .get(index)
                    .ok_or(InvalidGraph("owned initializer access is outside callable"))?;
                if node.span.expansion != initializer.initializer.expansion
                    || node.span.start < initializer.initializer.start
                    || node.span.end > initializer.initializer.end
                    || !matches!(&node.event, Event::Access { place, access: Access::Read | Access::Move, .. } if place == source)
                {
                    return Err(InvalidGraph(
                        "owned initializer access differs from its retained source",
                    ));
                }
            }
            _ => {
                return Err(InvalidGraph(
                    "owned initializer source and access must be retained together",
                ));
            }
        }
    }
    Ok(())
}

/// The resolved input of one callable analysis. Unknown identities or malformed edges must refuse validation.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct Graph {
    /// Exact source callable that produced this graph; a map key alone cannot authenticate a borrowed graph.
    pub owner: CanonicalSymbolId,
    pub permissions: BTreeMap<Root, Permission>,
    pub loans: Vec<Loan>,
    /// Explicit local declarations retain their selected loan independently of display spelling.
    pub local_loans: BTreeMap<CanonicalSymbolId, LoanId>,
    /// Owned locals whose values keep one or more checked source loans alive through their last value use.
    pub dependent_locals: BTreeMap<CanonicalSymbolId, Vec<LoanId>>,
    /// Aggregate expression spans whose physical value stores the retained references.
    pub dependent_values: BTreeSet<HirSourceSpan>,
    /// Canonical owned destinations are distinct from local aliases and invocation-bounded argument owners.
    pub owned_locals: BTreeMap<CanonicalSymbolId, OwnedInitialization>,
    /// Argument-to-loan bindings are retained at construction; lowering must not recover them from similar spans.
    pub invocations: BTreeMap<HirSourceSpan, Invocation>,
    /// Observer and operator dependencies retain their evaluated operand and exact consumption event.
    pub operations: BTreeMap<HirSourceSpan, OperationInputs>,
    /// Closures bound to a local, by closure expression, with the captures they read or borrow when built.
    pub closures: BTreeMap<HirSourceSpan, ClosureCaptures>,
    pub nodes: Vec<Node>,
    pub entry: usize,
}

/// A closure bound to a local. It reads each captured Copy value when it is built and retains one shared loan for
/// every other capture, reborrowing a captured alias through its existing loan. Every call of the local uses all of
/// those retained loans.
#[derive(Debug, Clone)]
pub(in crate::typechecker) struct ClosureCaptures {
    pub local: CanonicalSymbolId,
    /// The captured declarations, in the checker's capture order.
    pub captures: Vec<CanonicalSymbolId>,
    /// The declaration and loan for each borrowed capture.
    pub loans: Vec<(CanonicalSymbolId, LoanId)>,
    /// The declaration and read event for each captured Copy value.
    pub reads: Vec<(CanonicalSymbolId, usize)>,
}

/// Each capture is either one shared retained loan or one read at the closure itself. The retained links are
/// validated here; they are never recovered from spans.
fn validate_closures(graph: &Graph) -> Result<(), InvalidGraph> {
    for (span, closure) in &graph.closures {
        if closure.local.kind != incan_semantics_core::SemanticSourceTargetKind::Local {
            return Err(InvalidGraph("closure is not bound to a local"));
        }
        let mut covered = BTreeSet::new();
        for (capture, id) in &closure.loans {
            let loan = graph.loans.get(id.0).ok_or(InvalidGraph("closure loan is unknown"))?;
            let Root::Binding(root) = &loan.source.root else {
                return Err(InvalidGraph("closure loan borrows a bounded temporary"));
            };
            // A captured alias is reborrowed through its own loan; a direct capture is borrowed whole.
            let source_matches = match loan.parent {
                Some(parent) => graph.local_loans.get(capture) == Some(&parent),
                None => loan.source.projection.is_empty() && capture == root,
            };
            if !closure.captures.contains(capture) || !source_matches {
                return Err(InvalidGraph("closure loan borrows no captured place"));
            }
            if loan.span != *span || loan.kind != LoanKind::Shared || loan.writable || !covered.insert(capture.clone())
            {
                return Err(InvalidGraph("closure loan differs from its shared captured borrow"));
            }
        }
        if graph.local_loans.contains_key(&closure.local) {
            return Err(InvalidGraph("closure local holds a loan its closure does not record"));
        }
        for (capture, index) in &closure.reads {
            let node = graph
                .nodes
                .get(*index)
                .ok_or(InvalidGraph("closure capture read is outside callable"))?;
            let Event::Access {
                place,
                access: Access::Read,
                through,
            } = &node.event
            else {
                return Err(InvalidGraph("closure capture is not a read"));
            };
            let Root::Binding(source) = &place.root else {
                return Err(InvalidGraph("closure capture reads a bounded temporary"));
            };
            let source_matches = match through {
                Some(parent) => {
                    graph.local_loans.get(capture) == Some(parent)
                        && graph.loans.get(parent.0).is_some_and(|loan| loan.source == *place)
                }
                None => place.projection.is_empty() && capture == source,
            };
            if node.span != *span
                || !source_matches
                || !closure.captures.contains(capture)
                || !covered.insert(capture.clone())
            {
                return Err(InvalidGraph("closure capture read differs from its capture"));
            }
        }
        if covered.len() != closure.captures.len() {
            return Err(InvalidGraph("closure capture has neither a read nor a loan"));
        }
    }
    Ok(())
}

/// Source conflict category, independent of human diagnostic wording or native compiler diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::typechecker) enum ConflictKind {
    Permission,
    ActiveLoan(LoanId),
    Moved(usize),
    UnavailableLoan(LoanId),
}

/// One conflicting operation with its source anchor and dependency origin.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::typechecker) struct Conflict {
    pub node: usize,
    pub kind: ConflictKind,
}

/// Validated liveness facts and source conflicts. A successful analysis can still contain user-program conflicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::typechecker) struct Analysis {
    pub live_before: Vec<BTreeSet<LoanId>>,
    pub live_after: Vec<BTreeSet<LoanId>>,
    pub conflicts: Vec<Conflict>,
}

/// Refuse a malformed checker graph instead of treating missing identities or edges as safe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::typechecker) struct InvalidGraph(pub &'static str);

/// Analyze resolved edges with backward loan liveness and forward may-move facts. These monotone finite sets
/// converge over loop back-edges without a timeout or a textual last-use approximation.
pub(in crate::typechecker) fn analyze(graph: &Graph) -> Result<Analysis, InvalidGraph> {
    let validated = validate(graph)?;
    let (live_before, live_after) = liveness(graph, &validated);
    let states = incoming_states(graph, &live_before, &live_after);
    let dominators = dominators(graph, &validated.reachable, &validated.origins);
    let mut conflicts = BTreeSet::new();
    for (node, incoming) in states.iter().enumerate() {
        let Some(incoming) = incoming else { continue };
        let mut state = incoming.clone();
        state.loans.retain(|loan| live_before[node].contains(loan));
        check_event(graph, &validated, node, &state, &dominators[node], &mut conflicts)?;
    }
    Ok(Analysis {
        live_before,
        live_after,
        conflicts: conflicts.into_iter().collect(),
    })
}

/// Validation separates malformed internal facts from source-program conflicts.
struct Validated {
    reachable: BTreeSet<usize>,
    parents: Vec<BTreeSet<LoanId>>,
    origins: Vec<usize>,
}

/// Require every referenced place, loan and edge to be declared, with one creation point per loan and acyclic
/// reborrow ancestry. A caller cannot turn missing provenance into successful analysis.
fn validate(graph: &Graph) -> Result<Validated, InvalidGraph> {
    if graph.entry >= graph.nodes.len() {
        return Err(InvalidGraph("missing entry node"));
    }
    validate_invocations(graph)?;
    validate_owned_initializations(graph)?;
    operations::validate_operations(graph)?;
    validate_closures(graph)?;
    let mut origins = vec![None; graph.loans.len()];
    let mut destinations = BTreeSet::new();
    for (local, loan) in &graph.local_loans {
        if local.kind != incan_semantics_core::SemanticSourceTargetKind::Local {
            return Err(InvalidGraph("loan destination is not a local declaration"));
        }
        if !destinations.insert(*loan) {
            return Err(InvalidGraph("loan has multiple local destinations"));
        }
        // A local alias is only the borrow and cannot also declare an independent owner.
        if graph.permissions.contains_key(&Root::Binding(local.clone())) {
            return Err(InvalidGraph("local alias also declares an owned storage root"));
        }
        let source = graph
            .loans
            .get(loan.0)
            .ok_or(InvalidGraph("local destination refers to an unknown loan"))?;
        if matches!(&source.source.root, Root::Binding(root) if root == local) {
            return Err(InvalidGraph("local loan refers to its own destination"));
        }
    }
    for (local, loans) in &graph.dependent_locals {
        if local.kind != incan_semantics_core::SemanticSourceTargetKind::Local
            || !graph.permissions.contains_key(&Root::Binding(local.clone()))
            || loans.is_empty()
        {
            return Err(InvalidGraph("dependent value has no owned local destination"));
        }
        for loan in loans {
            if !destinations.insert(*loan) || graph.loans.get(loan.0).is_none() {
                return Err(InvalidGraph("dependent value has an unknown or reused loan"));
            }
        }
    }
    if graph.dependent_values.len() > graph.dependent_locals.len() {
        return Err(InvalidGraph("dependent aggregate has no local destination"));
    }

    // ---- Referenced edges and storage identities ----
    for (index, node) in graph.nodes.iter().enumerate() {
        if node.successors.iter().any(|successor| *successor >= graph.nodes.len()) {
            return Err(InvalidGraph("successor outside callable"));
        }
        match &node.event {
            Event::Borrow(loan) => {
                let origin = origins.get_mut(loan.0).ok_or(InvalidGraph("unknown loan"))?;
                if origin.replace(index).is_some() {
                    return Err(InvalidGraph("loan has multiple creation points"));
                }
            }
            Event::Access { place, through, .. } => {
                if !graph.permissions.contains_key(&place.root) {
                    return Err(InvalidGraph("unknown place root"));
                }
                if let Some(id) = through {
                    let loan = graph.loans.get(id.0).ok_or(InvalidGraph("unknown loan"))?;
                    if !loan.source.contains(place) {
                        return Err(InvalidGraph("loan does not reach accessed place"));
                    }
                }
            }
            Event::Nothing => {}
        }
    }
    let mut parents = Vec::new();

    // ---- Reborrow ancestry and source containment ----
    for (index, loan) in graph.loans.iter().enumerate() {
        if !graph.permissions.contains_key(&loan.source.root) {
            return Err(InvalidGraph("unknown loan source root"));
        }
        if loan.kind == LoanKind::Shared && loan.writable {
            return Err(InvalidGraph("shared loan grants writes"));
        }
        let mut ancestry = BTreeSet::new();
        let mut parent = loan.parent;
        while let Some(id) = parent {
            if id.0 == index || !ancestry.insert(id) {
                return Err(InvalidGraph("cyclic reborrow ancestry"));
            }
            let source = graph.loans.get(id.0).ok_or(InvalidGraph("unknown parent loan"))?;
            if !source.source.contains(&loan.source) {
                return Err(InvalidGraph("parent loan does not reach reborrowed place"));
            }
            parent = source.parent;
        }
        parents.push(ancestry);
    }
    let origins = origins
        .into_iter()
        .map(|origin| origin.ok_or(InvalidGraph("loan has no creation point")))
        .collect::<Result<_, _>>()?;

    // ---- Reachable control flow ----
    let mut reachable = BTreeSet::new();
    let mut pending = vec![graph.entry];
    while let Some(node) = pending.pop() {
        if reachable.insert(node) {
            pending.extend(&graph.nodes[node].successors);
        }
    }
    Ok(Validated {
        reachable,
        parents,
        origins,
    })
}

/// Invocation bindings retain a complete slot map and actual source loans through the call boundary.
fn validate_invocations(graph: &Graph) -> Result<(), InvalidGraph> {
    for (span, invocation) in &graph.invocations {
        let evaluations = invocation
            .receiver_evaluation
            .iter()
            .chain(invocation.arguments.iter().map(|argument| &argument.evaluation));
        if std::iter::once(invocation.completion_node)
            .chain(invocation.final_accesses.iter().copied())
            .chain(evaluations.flat_map(|evaluation| {
                [evaluation.start_node, evaluation.end_node]
                    .into_iter()
                    .chain(evaluation.source_node)
            }))
            .any(|index| index >= graph.nodes.len())
        {
            return Err(InvalidGraph("invocation event link is outside callable"));
        }
        let mut slots = BTreeSet::new();
        let mut written_positions = BTreeSet::new();
        for argument in &invocation.arguments {
            if argument.slot >= invocation.arguments.len() || !slots.insert(argument.slot) {
                return Err(InvalidGraph("invocation has an incomplete or duplicate parameter slot"));
            }
            if argument.written_position >= invocation.arguments.len()
                || !written_positions.insert(argument.written_position)
            {
                return Err(InvalidGraph(
                    "invocation has an incomplete or duplicate written argument position",
                ));
            }
            match (argument.access, argument.loan) {
                (IncanInputAccess::Shared | IncanInputAccess::Exclusive, Some(id)) => {
                    let loan = graph
                        .loans
                        .get(id.0)
                        .ok_or(InvalidGraph("argument has an unknown loan"))?;
                    let expected = if argument.access == IncanInputAccess::Shared {
                        LoanKind::Shared
                    } else {
                        LoanKind::Exclusive
                    };
                    if loan.kind != expected || loan.span != argument.span {
                        return Err(InvalidGraph("argument loan differs from its source access"));
                    }
                    if argument.source.as_ref() != Some(&loan.source) {
                        return Err(InvalidGraph("argument source differs from its selected loan"));
                    }
                    if let Root::Temporary {
                        owner,
                        expression,
                        invocation,
                    } = &loan.source.root
                        && (owner != &graph.owner || expression != &argument.span || invocation != span)
                    {
                        return Err(InvalidGraph("argument temporary belongs to another invocation"));
                    }
                    validate_invocation_loan(graph, *span, id)?;
                }
                (IncanInputAccess::Inferred | IncanInputAccess::Owned, None) => {
                    if let Some(source) = &argument.source
                        && !graph.permissions.contains_key(&source.root)
                    {
                        return Err(InvalidGraph("argument names an unknown source owner"));
                    }
                }
                _ => return Err(InvalidGraph("argument access and source loan disagree")),
            }
        }
        if invocation.receiver_owned {
            if invocation.receiver.is_some() || invocation.receiver_evaluation.is_none() {
                return Err(InvalidGraph("owned receiver has a borrow loan or no evaluation"));
            }
            if let Some(source) = &invocation.receiver_source
                && !graph.permissions.contains_key(&source.root)
            {
                return Err(InvalidGraph("owned receiver names an unknown source owner"));
            }
        } else if invocation.receiver_source.is_some() {
            return Err(InvalidGraph("borrowed receiver retains a separate owned source"));
        }
        if let Some(receiver) = invocation.receiver {
            validate_invocation_loan(graph, *span, receiver)?;
        }
    }
    Ok(())
}

/// A retained argument or receiver loan must participate in the graph's actual invocation operation.
fn validate_invocation_loan(graph: &Graph, span: HirSourceSpan, id: LoanId) -> Result<(), InvalidGraph> {
    let loan = graph
        .loans
        .get(id.0)
        .ok_or(InvalidGraph("invocation has an unknown loan"))?;
    if !graph.nodes.iter().any(|node| {
        node.span == span
            && matches!(&node.event,
        Event::Access { place, access: Access::Read, through: Some(actual) } if *actual == id && place == &loan.source)
    }) {
        return Err(InvalidGraph("invocation does not retain its argument or receiver loan"));
    }
    Ok(())
}

/// A child use keeps each suspended parent dependency alive, even if the parent's own binding is not read again.
fn retain_parents(loans: &mut BTreeSet<LoanId>, validated: &Validated) {
    let parents = loans
        .iter()
        .flat_map(|loan| validated.parents[loan.0].iter().copied())
        .collect::<BTreeSet<_>>();
    loans.extend(parents);
}

/// Compute liveness per edge. A borrow defines its own loan; a reborrow uses its parent, and only actual later
/// dependent uses retain either dependency on a successor path.
fn liveness(graph: &Graph, validated: &Validated) -> (Vec<BTreeSet<LoanId>>, Vec<BTreeSet<LoanId>>) {
    let mut before = vec![BTreeSet::new(); graph.nodes.len()];
    let mut after = before.clone();
    loop {
        let mut changed = false;
        for node in validated.reachable.iter().rev().copied() {
            let mut output = graph.nodes[node]
                .successors
                .iter()
                .flat_map(|successor| before[*successor].iter().copied())
                .collect::<BTreeSet<_>>();
            retain_parents(&mut output, validated);
            let mut input = output.clone();
            match graph.nodes[node].event {
                Event::Borrow(loan) => {
                    input.remove(&loan);
                    if let Some(parent) = graph.loans[loan.0].parent {
                        input.insert(parent);
                    }
                }
                Event::Access {
                    through: Some(loan), ..
                } => {
                    input.insert(loan);
                }
                _ => {}
            }
            retain_parents(&mut input, validated);
            if input != before[node] || output != after[node] {
                before[node] = input;
                after[node] = output;
                changed = true;
            }
        }
        if !changed {
            return (before, after);
        }
    }
}

/// May-facts include a transfer or live loan from any incoming path; origins remain available for diagnostics.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct State {
    loans: BTreeSet<LoanId>,
    moved: BTreeMap<Place, BTreeSet<usize>>,
}

impl State {
    /// Join predecessor facts without dropping the origin of a conflict from another branch or iteration.
    fn join(&mut self, other: &Self) {
        self.loans.extend(&other.loans);
        for (place, origins) in &other.moved {
            self.moved.entry(place.clone()).or_default().extend(origins);
        }
    }
}

/// Propagate transfer and creation facts to a fixed point. Replacement clears only that place and descendants;
/// writing one field cannot silently restore an already consumed parent aggregate.
fn incoming_states(graph: &Graph, before: &[BTreeSet<LoanId>], after: &[BTreeSet<LoanId>]) -> Vec<Option<State>> {
    let mut incoming = vec![None; graph.nodes.len()];
    incoming[graph.entry] = Some(State::default());
    let mut pending = VecDeque::from([graph.entry]);
    while let Some(node) = pending.pop_front() {
        let Some(mut output) = incoming[node].clone() else {
            continue;
        };
        output.loans.retain(|loan| before[node].contains(loan));
        match &graph.nodes[node].event {
            Event::Borrow(loan) => {
                output.loans.insert(*loan);
            }
            Event::Access {
                place,
                access: Access::Move,
                ..
            } => {
                output.moved.entry(place.clone()).or_default().insert(node);
            }
            Event::Access {
                place,
                access: Access::Replace,
                ..
            } => {
                output.moved.retain(|moved, _| !place.contains(moved));
            }
            _ => {}
        }
        output.loans.retain(|loan| after[node].contains(loan));
        for successor in &graph.nodes[node].successors {
            let previous = incoming[*successor].clone();
            incoming[*successor].get_or_insert_with(State::default).join(&output);
            if incoming[*successor] != previous {
                pending.push_back(*successor);
            }
        }
    }
    incoming
}

/// A loan creation must dominate its use. May-created facts alone would incorrectly admit a use reached through
/// a branch on which no borrow occurred; such a source requires an initialized, representable dependency. Only
/// creation nodes enter the domain, so a large straight-line function does not allocate a full node-by-node matrix.
fn dominators(graph: &Graph, reachable: &BTreeSet<usize>, origins: &[usize]) -> Vec<BTreeSet<usize>> {
    let mut predecessors = vec![Vec::new(); graph.nodes.len()];
    for node in reachable {
        for successor in &graph.nodes[*node].successors {
            predecessors[*successor].push(*node);
        }
    }
    let domain: BTreeSet<_> = origins
        .iter()
        .copied()
        .filter(|origin| reachable.contains(origin))
        .collect();
    let mut result = vec![domain.clone(); graph.nodes.len()];
    result[graph.entry] = domain.intersection(&BTreeSet::from([graph.entry])).copied().collect();
    loop {
        let mut changed = false;
        for node in reachable.iter().copied().filter(|node| *node != graph.entry) {
            let mut inherited = domain.clone();
            for predecessor in &predecessors[node] {
                inherited.retain(|candidate| result[*predecessor].contains(candidate));
            }
            if domain.contains(&node) {
                inherited.insert(node);
            }
            if inherited != result[node] {
                result[node] = inherited;
                changed = true;
            }
        }
        if !changed {
            return result;
        }
    }
}

/// Resolve the permission of an access path; a loan never permits ownership transfer of its referent.
fn permission(graph: &Graph, place: &Place, through: Option<LoanId>) -> Result<Permission, InvalidGraph> {
    if let Some(id) = through {
        let loan = graph.loans.get(id.0).ok_or(InvalidGraph("unknown loan"))?;
        return Ok(Permission {
            exclusive: loan.kind == LoanKind::Exclusive,
            writable: loan.writable,
            transferable: false,
        });
    }
    graph
        .permissions
        .get(&place.root)
        .copied()
        .ok_or(InvalidGraph("unknown place root"))
}

/// Validate a reachable operation against its incoming dataflow state. Related origins are retained as loan or
/// move identities, allowing the source diagnostic layer to attach the declaration and dependent-use anchors.
fn check_event(
    graph: &Graph,
    validated: &Validated,
    node: usize,
    state: &State,
    dominators: &BTreeSet<usize>,
    conflicts: &mut BTreeSet<Conflict>,
) -> Result<(), InvalidGraph> {
    // ---- Requested access and source permission ----
    let (place, access, through, allowed) = match &graph.nodes[node].event {
        Event::Nothing => return Ok(()),
        Event::Borrow(id) => {
            let loan = &graph.loans[id.0];
            let permission = permission(graph, &loan.source, loan.parent)?;
            let allowed =
                (loan.kind == LoanKind::Shared || permission.exclusive) && (!loan.writable || permission.writable);
            (
                &loan.source,
                if loan.kind == LoanKind::Shared {
                    Access::Read
                } else {
                    Access::Write
                },
                loan.parent,
                allowed,
            )
        }
        Event::Access { place, access, through } => {
            let permission = permission(graph, place, *through)?;
            let allowed = match access {
                Access::Read => true,
                Access::Write | Access::Replace => permission.writable,
                Access::Move => permission.transferable,
            };
            (place, *access, *through, allowed)
        }
    };
    if !allowed {
        conflicts.insert(Conflict {
            node,
            kind: ConflictKind::Permission,
        });
    }

    // ---- The loan exists on every path reaching its use ----
    let mut path = BTreeSet::new();
    if let Some(loan) = through {
        path.insert(loan);
        path.extend(&validated.parents[loan.0]);
        for dependency in &path {
            if !dominators.contains(&validated.origins[dependency.0]) {
                conflicts.insert(Conflict {
                    node,
                    kind: ConflictKind::UnavailableLoan(*dependency),
                });
            }
        }
    }
    for (moved, origins) in &state.moved {
        let replaces_moved_storage = access == Access::Replace && place.contains(moved);
        if moved.overlaps(place) && !replaces_moved_storage {
            for origin in origins {
                conflicts.insert(Conflict {
                    node,
                    kind: ConflictKind::Moved(*origin),
                });
            }
        }
    }

    // ---- Conflicting outstanding dependencies ----
    for id in &state.loans {
        if path.contains(id) {
            continue;
        }
        let loan = &graph.loans[id.0];
        if loan.source.overlaps(place) && (loan.kind == LoanKind::Exclusive || access != Access::Read) {
            conflicts.insert(Conflict {
                node,
                kind: ConflictKind::ActiveLoan(*id),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
