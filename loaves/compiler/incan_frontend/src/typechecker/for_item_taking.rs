//! `for` loops that take the items out of the list they iterate (#1844).
//!
//! A `for` loop over a list reads each item where it stays, so the list keeps its items. When the loop body hands an
//! item on by value and the item can be neither copied nor cloned, such as a `JoinHandle[T]`, the item has to leave
//! the list, so the loop takes every item out of it. The body hands an item on when it awaits it, returns or yields
//! it, breaks with it, assigns it to another name, passes it to a call, places it in a new tuple, list, set or dict
//! value, or iterates it (a list of such items) in a nested `for` loop that takes its items. Whether an item type can
//! be neither copied nor cloned comes from the compiler-known type table (`not_cloneable`), not from the clone check.
//!
//! Taking the items keeps the program's behavior only while nothing reads the list again, so the checker decides it
//! here, before the body is checked, records the loop for lowering ([`TypeCheckInfo::for_loop_takes_items`]), which
//! iterates the list by value, and refuses (`INCAN-T0119`):
//!
//! - a read of the list inside the loop body or after the loop, until an assignment gives the binding a new list on
//!   every path after the loop: a statement of the block that holds the loop, or of a block around it, that no `break`
//!   or `continue` of an enclosing loop can skip;
//! - a closure that captured the list before the loop, since the closure can still read the list;
//! - a loop that an enclosing loop repeats over a list defined outside it, unless each pass assigns the list a new one
//!   before the loop; and when it does, a read of the list in the enclosing loops (their conditions included) that
//!   comes before that assignment, since a repeat reaches it after the loop took the items.
//!
//! The list is a local binding, the binding of an enclosing loop that takes its own items, or a parameter whose changes
//! do not reach the caller; a field, a static or any other expression keeps the ordinary iteration.
//!
//! [`TypeCheckInfo::for_loop_takes_items`]: crate::typechecker::TypeCheckInfo::for_loop_takes_items

use std::collections::{HashMap, HashSet};

use incan_lang::lang::builtins;
use incan_lang::lang::keywords::KeywordId;
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeId};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_semantics_core::{SemanticSourceTargetKind, SurfaceFeatureKey};

use crate::ast::*;
use crate::ast_walk::any_expr_in_body;
use crate::diagnostics::errors::{self, TakenListReuse};
use crate::symbols::{ResolvedType, SymbolId, SymbolKind, TypeInfo};
use crate::typechecker::helpers::collection_type_id;

use super::TypeChecker;
use super::check_stmt::{TupleShape, classify_tuple_shape};

/// The `for` loop state of the function body being checked.
#[derive(Debug, Default, Clone)]
pub(super) struct ForItemTaking {
    /// Lists whose items a checked `for` loop takes, keyed by the list's binding.
    taken_lists: HashMap<SymbolId, TakenList>,
    /// The pattern bindings of the loops that read their items in place; a nested loop never takes items from them.
    in_place_loop_bindings: HashSet<SymbolId>,
    /// The statement blocks around the statement being checked, outermost first.
    block_path: Vec<usize>,
    /// The last block number handed out in this body.
    last_block: usize,
    /// Every read of a list binding checked so far, in source order.
    list_reads: Vec<(SymbolId, Span)>,
    /// Every read of a list binding from inside a closure body, of a binding held outside that closure.
    captures: Vec<(SymbolId, Span)>,
    /// Every assignment to an existing list binding checked so far, with where it starts and the blocks around it.
    reassignments: Vec<Reassignment>,
    /// Every `break` and `continue` checked so far: where it is, and the start of the loop it leaves.
    loop_exits: Vec<(usize, usize)>,
}

/// A list whose items a `for` loop takes out.
#[derive(Debug, Clone)]
struct TakenList {
    /// The list's name, as the refusal spells it.
    list: String,
    /// The loop's iterable, where the items are taken.
    loop_span: Span,
    /// Where the loop body ends: a read of the list before this offset is inside the loop.
    body_end: usize,
    /// The item type, as the refusal spells it.
    item_ty: String,
    /// The blocks around the loop: an assignment in one of them, after the loop, runs on every path after it unless a
    /// `break` or `continue` skips it.
    block_path: Vec<usize>,
}

/// An assignment to an existing list binding.
#[derive(Debug, Clone)]
struct Reassignment {
    /// The assigned binding.
    symbol: SymbolId,
    /// Where the assignment starts.
    start: usize,
    /// The blocks around the assignment, outermost first.
    block_path: Vec<usize>,
}

impl TypeChecker {
    /// Decide, before its body is checked, whether a `for` loop takes the items out of the list it iterates, and
    /// return whether it does.
    ///
    /// A loop takes the items when its iterable names a list binding the loop may empty and the body hands on by value
    /// a pattern binding whose type can be neither copied nor cloned. The decision is recorded for lowering and the
    /// list is watched for reads until the function ends. A closure that captured the list before the loop, a repeat
    /// of the loop by an enclosing loop over a list defined outside it, and the reads such a repeat reaches after the
    /// loop are refused here.
    pub(super) fn plan_for_item_taking(&mut self, for_stmt: &ForStmt, item_ty: &ResolvedType) -> bool {
        let Some(list) = named_list(&for_stmt.iter) else {
            return false;
        };
        let moving = self.moving_bindings(&for_stmt.pattern.node, item_ty);
        if !self.body_hands_on(&for_stmt.body, &moving) {
            return false;
        }
        let Some(symbol) = self.symbols.lookup(list) else {
            return false;
        };
        let Some(definition) = self.list_binding_definition(symbol) else {
            return false;
        };
        self.type_info.record_for_loop_takes_items(for_stmt.iter.span);
        let taken = TakenList {
            list: list.to_string(),
            loop_span: for_stmt.iter.span,
            body_end: for_stmt
                .body
                .last()
                .map_or(for_stmt.iter.span.end, |stmt| stmt.span.end),
            item_ty: item_type_display(item_ty),
            block_path: self.for_item_taking.block_path.clone(),
        };

        // ---- A closure that captured the list before the loop can still read it ----
        let capture = self
            .for_item_taking
            .captures
            .iter()
            .find(|(captured, _)| *captured == symbol)
            .map(|(_, span)| *span);
        if let Some(capture) = capture {
            self.refuse_taken_list_use(&taken, TakenListReuse::CapturedBeforeLoop, capture, taken.loop_span);
        }

        // ---- An enclosing loop that repeats the loop over a list defined outside it ----
        let repeating: Vec<(usize, usize)> = self
            .loop_stack
            .iter()
            .filter(|enclosing| definition.scope < enclosing.scope)
            .map(|enclosing| (enclosing.start, enclosing.block_depth))
            .collect();
        if let (Some(&(outermost_start, _)), Some(&(_, innermost_depth))) = (repeating.first(), repeating.last()) {
            match self.refill_in_each_pass(symbol, innermost_depth) {
                None => {
                    self.refuse_taken_list_use(&taken, TakenListReuse::RepeatedLoop, taken.loop_span, definition.span);
                }
                Some(refill_start) => {
                    let reached_again: Vec<Span> = self
                        .for_item_taking
                        .list_reads
                        .iter()
                        .filter(|(read, span)| {
                            *read == symbol && span.start >= outermost_start && span.start < refill_start
                        })
                        .map(|(_, span)| *span)
                        .collect();
                    for span in reached_again {
                        self.refuse_taken_list_use(&taken, TakenListReuse::ReadBeforeRefill, span, taken.loop_span);
                    }
                }
            }
        }
        self.for_item_taking.taken_lists.insert(symbol, taken);
        true
    }

    /// Remember the bindings of a `for` pattern whose loop reads its items in place, so a nested loop over one of them
    /// keeps ordinary iteration; a loop that takes its items owns them, and a nested loop may take from those.
    pub(super) fn remember_for_pattern_bindings(&mut self, pattern: &Pattern, takes_items: bool) {
        if takes_items {
            return;
        }
        let mut names = Vec::new();
        pattern_binding_types(pattern, &ResolvedType::Unknown, &mut names);
        for (name, _) in names {
            if let Some(symbol) = self.symbols.lookup(name) {
                self.for_item_taking.in_place_loop_bindings.insert(symbol);
            }
        }
    }

    /// Account for a statement at `span` that assigns the existing binding `name` again.
    ///
    /// The assignment is remembered, so a later `for` loop can tell that it refills the list in each pass of an
    /// enclosing loop. A list whose items a loop took stops being watched when the assignment runs on every path after
    /// that loop: it is a statement of the block that holds the loop, or of a block around that one, and no `break` or
    /// `continue` after the loop leaves a loop that holds the assignment, which would skip it. An assignment anywhere
    /// else, such as a branch or the loop body, leaves the list watched.
    pub(super) fn note_list_reassignment(&mut self, name: &str, span: Span) {
        let Some(symbol) = self.symbols.lookup(name) else {
            return;
        };
        let block_path = self.for_item_taking.block_path.clone();
        if let Some(taken) = self.for_item_taking.taken_lists.get(&symbol)
            && taken.block_path.starts_with(&block_path)
            && !self.loop_exit_skips(taken.loop_span.end)
        {
            self.for_item_taking.taken_lists.remove(&symbol);
        }
        self.for_item_taking.reassignments.push(Reassignment {
            symbol,
            start: span.start,
            block_path,
        });
    }

    /// Record a `break` or `continue` at `span`, which leaves the innermost enclosing loop.
    pub(super) fn note_loop_exit(&mut self, span: Span) {
        if let Some(enclosing) = self.loop_stack.last() {
            let target = enclosing.start;
            self.for_item_taking.loop_exits.push((span.start, target));
        }
    }

    /// Whether a `break` or `continue` after `after` leaves a loop that holds the statement being checked, and so can
    /// skip it.
    fn loop_exit_skips(&self, after: usize) -> bool {
        self.for_item_taking
            .loop_exits
            .iter()
            .any(|(at, target)| *at > after && self.loop_stack.iter().any(|enclosing| enclosing.start == *target))
    }

    /// Return where the latest assignment of a new list to `symbol` starts, when it runs in each pass of the innermost
    /// enclosing loop before the loop being planned: a statement of a block that holds that loop and lies inside the
    /// enclosing loop's body, whose blocks start after `enclosing_depth`.
    fn refill_in_each_pass(&self, symbol: SymbolId, enclosing_depth: usize) -> Option<usize> {
        let loop_path = &self.for_item_taking.block_path;
        self.for_item_taking
            .reassignments
            .iter()
            .filter(|refill| {
                refill.symbol == symbol
                    && refill.block_path.len() > enclosing_depth
                    && loop_path.starts_with(&refill.block_path)
            })
            .map(|refill| refill.start)
            .max()
    }

    /// Return how many statement blocks surround the statement being checked.
    pub(super) fn item_taking_block_depth(&self) -> usize {
        self.for_item_taking.block_path.len()
    }

    /// Start a fresh `for` loop state for a function body, returning the enclosing body's state to restore after it.
    pub(super) fn enter_for_item_taking_scope(&mut self) -> ForItemTaking {
        std::mem::take(&mut self.for_item_taking)
    }

    /// Restore the enclosing body's `for` loop state once a function body has been checked.
    pub(super) fn exit_for_item_taking_scope(&mut self, previous: ForItemTaking) {
        self.for_item_taking = previous;
    }

    /// Enter a statement block, so an assignment knows which blocks it runs in.
    pub(super) fn enter_item_taking_block(&mut self) {
        self.for_item_taking.last_block += 1;
        let block = self.for_item_taking.last_block;
        self.for_item_taking.block_path.push(block);
    }

    /// Leave the statement block entered last.
    pub(super) fn exit_item_taking_block(&mut self) {
        let _ = self.for_item_taking.block_path.pop();
    }

    /// Account for a read of the binding `symbol` at `span`.
    ///
    /// A read of a list binding is remembered, and so is a read from inside a closure body of a list binding held
    /// outside the closure, which is a capture. A read of a list whose items a `for` loop takes, inside that loop's
    /// body or after it, is refused (`INCAN-T0119`). A read that comes before the loop's iterable in the source is not:
    /// the header's own read, and a read an earlier statement makes when the checker visits it again.
    pub(super) fn observe_binding_read(&mut self, symbol: SymbolId, span: Span) {
        let Some(definition) = self.symbols.get(symbol) else {
            return;
        };
        let SymbolKind::Variable(info) = &definition.kind else {
            return;
        };
        if list_item_type(&info.ty).is_none() {
            return;
        }
        if self.symbols.read_crosses_callable_scope(definition.scope) {
            self.for_item_taking.captures.push((symbol, span));
        }
        self.for_item_taking.list_reads.push((symbol, span));
        let Some(taken) = self.for_item_taking.taken_lists.get(&symbol).cloned() else {
            return;
        };
        if span.start < taken.loop_span.end {
            return;
        }
        let reuse = if span.end <= taken.body_end {
            TakenListReuse::InsideLoop
        } else {
            TakenListReuse::AfterLoop
        };
        self.refuse_taken_list_use(&taken, reuse, span, taken.loop_span);
    }

    /// Report one use of a list whose items a `for` loop takes, once per source location.
    fn refuse_taken_list_use(&mut self, taken: &TakenList, reuse: TakenListReuse, span: Span, related: Span) {
        if self
            .errors
            .iter()
            .any(|error| error.span == span && error.stable_code() == Some("INCAN-T0119"))
        {
            return;
        }
        let error = errors::taken_list_used_again(&taken.list, &taken.item_ty, reuse, span, related);
        self.errors.push(error);
    }

    /// Whether a value of this type can be neither copied nor cloned: a compiler-known type the type table marks
    /// `not_cloneable` (`JoinHandle[T]`), alone or inside a tuple or a collection.
    ///
    /// A declared class, model, newtype or enum, a Rust type, and a type parameter are never such a value here, even
    /// when one shares a compiler-known type's name.
    fn value_is_not_cloneable(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::Tuple(items) => items.iter().any(|item| self.value_is_not_cloneable(item)),
            ResolvedType::Generic(name, args)
                if collection_type_id(name).is_some()
                    || matches!(
                        self.surface_type_in_scope(name),
                        Some(SurfaceTypeId::Vec | SurfaceTypeId::HashMap)
                    ) =>
            {
                args.iter().any(|arg| self.value_is_not_cloneable(arg))
            }
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => self
                .surface_type_in_scope(name)
                .is_some_and(surface_types::is_not_cloneable),
            _ => false,
        }
    }

    /// Return the compiler-known type `name` spells here: when the name is not bound, or is bound to the stdlib import
    /// of that type. A name bound to a declared type or a Rust item spells no compiler-known type.
    fn surface_type_in_scope(&self, name: &str) -> Option<SurfaceTypeId> {
        match self.lookup_symbol(name).map(|symbol| &symbol.kind) {
            None | Some(SymbolKind::Type(TypeInfo::Builtin)) => surface_types::from_str(name),
            Some(_) => None,
        }
    }

    /// Return the bindings of a `for` pattern whose values can be neither copied nor cloned, with their types.
    fn moving_bindings<'p>(&self, pattern: &'p Pattern, item_ty: &ResolvedType) -> HashMap<&'p str, ResolvedType> {
        let mut bindings = Vec::new();
        pattern_binding_types(pattern, item_ty, &mut bindings);
        bindings
            .into_iter()
            .filter(|(_, ty)| self.value_is_not_cloneable(ty))
            .collect()
    }

    /// Whether a `for` body hands one of the `moving` bindings on by value anywhere, nested blocks included.
    fn body_hands_on(&self, body: &[Spanned<Statement>], moving: &HashMap<&str, ResolvedType>) -> bool {
        !moving.is_empty()
            && (self.statements_hand_on(body, moving)
                || any_expr_in_body(body, |expr| {
                    expr_hands_on(expr, moving) || self.expr_blocks_hand_on(expr, moving)
                }))
    }

    /// Whether a statement block held by an expression (a `match` arm, an `if` or `loop:` expression, a `race for`
    /// arm) hands a `moving` binding on at statement level.
    fn expr_blocks_hand_on(&self, expr: &Expr, moving: &HashMap<&str, ResolvedType>) -> bool {
        match expr {
            Expr::Match(_, arms) => arms
                .iter()
                .any(|arm| matches!(&arm.node.body, MatchBody::Block(body) if self.statements_hand_on(body, moving))),
            Expr::If(if_expr) => {
                self.statements_hand_on(&if_expr.then_body, moving)
                    || if_expr
                        .else_body
                        .as_ref()
                        .is_some_and(|else_body| self.statements_hand_on(else_body, moving))
            }
            Expr::Loop(loop_expr) => self.statements_hand_on(&loop_expr.body, moving),
            Expr::Surface(surface) => match &surface.payload {
                SurfaceExprPayload::RaceFor(race) => race
                    .arms
                    .iter()
                    .any(|arm| matches!(&arm.body, RaceForBody::Block(body) if self.statements_hand_on(body, moving))),
                _ => false,
            },
            _ => false,
        }
    }

    /// Whether a statement list, or a statement block nested in it, returns, breaks with or assigns a `moving`
    /// binding, or iterates one in a nested `for` loop that takes its items.
    ///
    /// Blocks held by expressions are reached through [`Self::expr_blocks_hand_on`] from the expression walk.
    fn statements_hand_on(&self, body: &[Spanned<Statement>], moving: &HashMap<&str, ResolvedType>) -> bool {
        body.iter().any(|stmt| match &stmt.node {
            Statement::Return(Some(value)) | Statement::Break(Some(value)) => is_moving_binding(value, moving),
            Statement::Assignment(assign) => is_moving_binding(&assign.value, moving),
            Statement::If(if_stmt) => {
                self.statements_hand_on(&if_stmt.then_body, moving)
                    || if_stmt
                        .elif_branches
                        .iter()
                        .any(|(_, branch)| self.statements_hand_on(branch, moving))
                    || if_stmt
                        .else_body
                        .as_ref()
                        .is_some_and(|else_body| self.statements_hand_on(else_body, moving))
            }
            Statement::Loop(loop_stmt) => self.statements_hand_on(&loop_stmt.body, moving),
            Statement::While(while_stmt) => self.statements_hand_on(&while_stmt.body, moving),
            Statement::For(inner) => {
                self.nested_loop_takes_items(inner, moving) || self.statements_hand_on(&inner.body, moving)
            }
            Statement::Unsafe(unsafe_stmt) => self.statements_hand_on(&unsafe_stmt.body, moving),
            _ => false,
        })
    }

    /// Whether a nested `for` loop iterates a `moving` list binding and hands its own items on by value, so it takes
    /// them out of that list.
    fn nested_loop_takes_items(&self, inner: &ForStmt, moving: &HashMap<&str, ResolvedType>) -> bool {
        let Some(item_ty) = named_list(&inner.iter)
            .and_then(|list| moving.get(list))
            .and_then(list_item_type)
        else {
            return false;
        };
        let inner_moving = self.moving_bindings(&inner.pattern.node, item_ty);
        self.body_hands_on(&inner.body, &inner_moving)
    }

    /// Return the list binding a `for` loop may take items from: a local list binding other than the binding of an
    /// enclosing loop that reads its items in place, or a list parameter whose changes do not reach the caller.
    fn list_binding_definition(&self, symbol: SymbolId) -> Option<ListDefinition> {
        let definition = self.symbols.get(symbol)?;
        let SymbolKind::Variable(info) = &definition.kind else {
            return None;
        };
        list_item_type(&info.ty)?;
        let eligible = match &self.symbols.identity_of(symbol)?.kind {
            SemanticSourceTargetKind::Local => !self.for_item_taking.in_place_loop_bindings.contains(&symbol),
            SemanticSourceTargetKind::Parameter => !self
                .type_info
                .declarations
                .mut_param_marker(definition.span, &definition.name)
                .unwrap_or(false),
            _ => false,
        };
        eligible.then_some(ListDefinition {
            span: definition.span,
            scope: definition.scope,
        })
    }
}

/// Where a list binding a `for` loop takes items from was defined.
struct ListDefinition {
    /// The binding's definition.
    span: Span,
    /// The symbol-table scope that holds the binding.
    scope: usize,
}

/// Return an expression without the parentheses around it.
fn unparenthesized(expr: &Spanned<Expr>) -> &Expr {
    match &expr.node {
        Expr::Paren(inner) => unparenthesized(inner),
        other => other,
    }
}

/// Return the name a `for` iterable spells, parenthesized or not, when it is a plain name.
fn named_list(iter: &Spanned<Expr>) -> Option<&str> {
    match unparenthesized(iter) {
        Expr::Ident(name) => Some(name.as_str()),
        _ => None,
    }
}

/// Return the item type of a `list[T]` type.
fn list_item_type(ty: &ResolvedType) -> Option<&ResolvedType> {
    match ty {
        ResolvedType::Generic(name, args) if collection_type_id(name) == Some(CollectionTypeId::List) => args.first(),
        _ => None,
    }
}

/// Spell an item type for the refusal, leaving out type arguments the checker could not resolve.
fn item_type_display(ty: &ResolvedType) -> String {
    match ty {
        ResolvedType::Generic(name, args) if args.iter().any(|arg| matches!(arg, ResolvedType::Unknown)) => {
            name.clone()
        }
        ResolvedType::Generic(name, args) => {
            let args: Vec<String> = args.iter().map(item_type_display).collect();
            format!("{name}[{}]", args.join(", "))
        }
        ResolvedType::Tuple(items) => {
            let items: Vec<String> = items.iter().map(item_type_display).collect();
            format!("({})", items.join(", "))
        }
        other => other.to_string(),
    }
}

/// Collect the names a `for` pattern binds, each with the type it takes from an item of `item_ty`.
fn pattern_binding_types<'p>(
    pattern: &'p Pattern,
    item_ty: &ResolvedType,
    bindings: &mut Vec<(&'p str, ResolvedType)>,
) {
    match pattern {
        Pattern::Binding(name) => bindings.push((name.as_str(), item_ty.clone())),
        Pattern::Tuple(items) => {
            let element_types = match classify_tuple_shape(item_ty) {
                TupleShape::Tuple(types) if types.len() == items.len() => types,
                _ => vec![ResolvedType::Unknown; items.len()],
            };
            for (item, element_ty) in items.iter().zip(&element_types) {
                pattern_binding_types(&item.node, element_ty, bindings);
            }
        }
        _ => {}
    }
}

/// Whether an expression is exactly one of the `moving` bindings.
fn is_moving_binding(expr: &Spanned<Expr>, moving: &HashMap<&str, ResolvedType>) -> bool {
    match &expr.node {
        Expr::Ident(name) => moving.contains_key(name.as_str()),
        Expr::Paren(inner) => is_moving_binding(inner, moving),
        _ => false,
    }
}

/// Whether one of the call arguments is a `moving` binding.
fn args_hand_on(args: &[CallArg], moving: &HashMap<&str, ResolvedType>) -> bool {
    args.iter().any(|arg| match arg {
        CallArg::Positional(value) | CallArg::Named(_, value) => is_moving_binding(value, moving),
        CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => false,
    })
}

/// Whether an expression takes a `moving` binding by value: the operand of `await` or `yield`, an argument of a call
/// other than a builtin function, or an item of a new tuple, list, set or dict value.
fn expr_hands_on(expr: &Expr, moving: &HashMap<&str, ResolvedType>) -> bool {
    match expr {
        Expr::Surface(surface) => matches!(
            (&surface.key, &surface.payload),
            (
                SurfaceFeatureKey::SoftKeyword(KeywordId::Await),
                SurfaceExprPayload::PrefixUnary(operand),
            ) if is_moving_binding(operand, moving)
        ),
        Expr::Call(callee, _, args) => {
            let builtin = matches!(&callee.node, Expr::Ident(name) if builtins::from_str(name).is_some());
            !builtin && args_hand_on(args, moving)
        }
        Expr::MethodCall(_, _, _, args) | Expr::Constructor(_, args) => args_hand_on(args, moving),
        Expr::Yield(Some(value)) => is_moving_binding(value, moving),
        Expr::Tuple(items) | Expr::Set(items) => items.iter().any(|item| is_moving_binding(item, moving)),
        Expr::List(entries) => entries
            .iter()
            .any(|entry| matches!(entry, ListEntry::Element(value) if is_moving_binding(value, moving))),
        Expr::Dict(entries) => entries
            .iter()
            .any(|entry| matches!(entry, DictEntry::Pair(_, value) if is_moving_binding(value, moving))),
        _ => false,
    }
}
