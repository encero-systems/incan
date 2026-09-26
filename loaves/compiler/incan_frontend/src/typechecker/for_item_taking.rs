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
//! here, before the body is checked:
//!
//! - it records the loop for lowering ([`TypeCheckInfo::for_loop_takes_items`]), which iterates the list by value;
//! - it refuses (`INCAN-T0119`) a read of the list inside the loop body or after the loop, a read inside or after the
//!   loop of a closure bound to a name that captured the list before the loop, and a loop an enclosing loop repeats
//!   over a list built outside it, unless each pass assigns the list a new one before the loop. An assignment of a new
//!   list to the binding ends the watch only when it runs on every path after the loop: a statement of the block that
//!   holds the loop, or of a block around it.
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
    /// Lists whose items a checked `for` loop takes, keyed by the list's name.
    taken_lists: HashMap<String, TakenList>,
    /// Closures bound to a name that captured a list before a `for` loop took its items, keyed by that binding.
    capturing_closures: HashMap<SymbolId, CapturingClosure>,
    /// The pattern bindings of the loops that read their items in place; a nested loop never takes items from them.
    in_place_loop_bindings: HashSet<SymbolId>,
    /// The statement blocks around the statement being checked, outermost first.
    block_path: Vec<usize>,
    /// The last block number handed out in this body.
    last_block: usize,
    /// The name a closure is being bound to while the closure is checked.
    closure_holder: Option<String>,
    /// Bindings that a closure bound to a name reads from outside its own body, with that name.
    captures: Vec<(SymbolId, String)>,
    /// Every assignment to an existing binding checked so far, with the blocks around it.
    reassignments: Vec<(SymbolId, Vec<usize>)>,
}

/// A list whose items a `for` loop takes out.
#[derive(Debug, Clone)]
struct TakenList {
    /// The list's binding, so a different binding spelled the same is not mistaken for it.
    symbol: SymbolId,
    /// The loop's iterable, where the items are taken.
    loop_span: Span,
    /// Where the loop body ends: a read of the list before this offset is inside the loop.
    body_end: usize,
    /// The item type, as the refusal spells it.
    item_ty: String,
    /// The blocks around the loop: an assignment in one of them, after the loop, runs on every path after it.
    block_path: Vec<usize>,
}

/// A closure bound to a name that captured a list before a `for` loop took the list's items.
#[derive(Debug, Clone)]
struct CapturingClosure {
    /// The captured list's name.
    list: String,
    /// The loop that took the list's items.
    taken: TakenList,
}

impl TypeChecker {
    /// Decide, before its body is checked, whether a `for` loop takes the items out of the list it iterates, and
    /// return whether it does.
    ///
    /// A loop takes the items when its iterable names a list binding the loop may empty and the body hands on by value
    /// a pattern binding whose type can be neither copied nor cloned. The decision is recorded for lowering, and the
    /// list, and every closure bound to a name that captured it, are watched for reads until the function ends; an
    /// enclosing loop that repeats the loop over a list built outside it is refused here.
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
        let item_ty = item_type_display(item_ty);
        let repeated = self.loop_stack.last().is_some_and(|enclosing| {
            definition.scope < enclosing.scope && !self.reassigned_in_each_pass(symbol, enclosing.block_depth)
        });
        if repeated {
            self.errors.push(errors::taken_list_used_again(
                list,
                &item_ty,
                TakenListReuse::RepeatedLoop,
                for_stmt.iter.span,
                definition.span,
            ));
            return true;
        }
        let taken = TakenList {
            symbol,
            loop_span: for_stmt.iter.span,
            body_end: for_stmt
                .body
                .last()
                .map_or(for_stmt.iter.span.end, |stmt| stmt.span.end),
            item_ty,
            block_path: self.for_item_taking.block_path.clone(),
        };
        let holders: Vec<String> = self
            .for_item_taking
            .captures
            .iter()
            .filter(|(captured, _)| *captured == symbol)
            .map(|(_, holder)| holder.clone())
            .collect();
        for holder in holders {
            if let Some(holder_symbol) = self.symbols.lookup(&holder) {
                self.for_item_taking.capturing_closures.insert(
                    holder_symbol,
                    CapturingClosure {
                        list: list.to_string(),
                        taken: taken.clone(),
                    },
                );
            }
        }
        self.for_item_taking.taken_lists.insert(list.to_string(), taken);
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

    /// Account for a statement that assigns the existing binding `name` again.
    ///
    /// The assignment is remembered with the blocks around it, so a later `for` loop can tell that it refills the list
    /// in each pass of an enclosing loop. A list whose items a loop took stops being watched when the assignment runs
    /// on every path after that loop: it is a statement of the block that holds the loop, or of a block around that
    /// one. An assignment in any other block, such as a branch or the loop body, leaves the list watched.
    pub(super) fn note_list_reassignment(&mut self, name: &str) {
        let Some(symbol) = self.symbols.lookup(name) else {
            return;
        };
        let block_path = self.for_item_taking.block_path.clone();
        if let Some(taken) = self.for_item_taking.taken_lists.get(name)
            && taken.symbol == symbol
            && taken.block_path.starts_with(&block_path)
        {
            self.for_item_taking.taken_lists.remove(name);
        }
        self.for_item_taking.reassignments.push((symbol, block_path));
    }

    /// Whether the list binding `symbol` was assigned, before the loop being planned, by a statement that runs in each
    /// pass of the enclosing loop that reaches this loop: a statement of a block that holds this loop and lies inside
    /// the enclosing loop's body, whose blocks start after `enclosing_depth`.
    fn reassigned_in_each_pass(&self, symbol: SymbolId, enclosing_depth: usize) -> bool {
        let loop_path = &self.for_item_taking.block_path;
        self.for_item_taking
            .reassignments
            .iter()
            .any(|(assigned, path)| *assigned == symbol && path.len() > enclosing_depth && loop_path.starts_with(path))
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

    /// Note, while checking the value of an assignment, that the value is a closure bound to `name`; returns the name
    /// noted before, to restore with [`Self::finish_closure_binding`].
    pub(super) fn begin_closure_binding(&mut self, name: &str, value: &Spanned<Expr>) -> Option<String> {
        let holder = matches!(unparenthesized(value), Expr::Closure(..)).then(|| name.to_string());
        std::mem::replace(&mut self.for_item_taking.closure_holder, holder)
    }

    /// Restore the closure binding noted before [`Self::begin_closure_binding`].
    pub(super) fn finish_closure_binding(&mut self, previous: Option<String>) {
        self.for_item_taking.closure_holder = previous;
    }

    /// Account for a read of the binding `symbol` spelled `name` at `span`.
    ///
    /// A read from inside a closure bound to a name, of a binding held outside the closure, is a capture. A read of a
    /// list whose items a `for` loop takes, inside that loop's body or after it, is refused, and so is a read of a
    /// closure that captured such a list (`INCAN-T0119`). A read that comes before the loop's iterable in the source is
    /// neither: the header's own read, and a read an earlier statement makes when the checker visits it again.
    pub(super) fn observe_binding_read(&mut self, name: &str, symbol: SymbolId, span: Span) {
        if let Some(holder) = self.for_item_taking.closure_holder.clone()
            && self.symbols.get(symbol).is_some_and(|definition| {
                matches!(definition.kind, SymbolKind::Variable(_))
                    && self.symbols.read_crosses_callable_scope(definition.scope)
            })
        {
            self.for_item_taking.captures.push((symbol, holder));
        }
        let refusal = if let Some(taken) = self.for_item_taking.taken_lists.get(name)
            && taken.symbol == symbol
        {
            Some((name.to_string(), None, taken.clone()))
        } else {
            self.for_item_taking
                .capturing_closures
                .get(&symbol)
                .map(|closure| (closure.list.clone(), Some(name), closure.taken.clone()))
        };
        let Some((list, closure, taken)) = refusal else {
            return;
        };
        if span.start < taken.loop_span.end
            || self
                .errors
                .iter()
                .any(|error| error.span == span && error.stable_code() == Some("INCAN-T0119"))
        {
            return;
        }
        let inside = span.end <= taken.body_end;
        let reuse = match (closure, inside) {
            (None, true) => TakenListReuse::InsideLoop,
            (None, false) => TakenListReuse::AfterLoop,
            (Some(closure), true) => TakenListReuse::ClosureInsideLoop(closure),
            (Some(closure), false) => TakenListReuse::ClosureAfterLoop(closure),
        };
        let error = errors::taken_list_used_again(&list, &taken.item_ty, reuse, span, taken.loop_span);
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
                .mut_param_markers
                .get(&(definition.span.start, definition.span.end))
                .copied()
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
