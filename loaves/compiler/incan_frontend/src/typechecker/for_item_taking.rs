//! `for` loops that take the items out of the list they iterate (#1844).
//!
//! A `for` loop over a list reads each item where it stays, so the list keeps its items. When the loop body hands the
//! item on by value -- it awaits the item, returns it, assigns it to another name, passes it to a call or places it
//! in a new value -- a copyable or cloneable item is copied, and an item that can be neither, such as a
//! `JoinHandle[T]`, has to leave the list. The loop then takes every item out of the list. That keeps the program's
//! behavior only while nothing reads the list again, so the checker decides it here, before the body is checked:
//!
//! - it records the loop for lowering ([`TypeCheckInfo::for_loop_takes_items`]), which iterates the list by value;
//! - it refuses a read of the list inside the loop body or after the loop, and a loop an enclosing loop repeats over a
//!   list built outside it (`INCAN-T0119`).
//!
//! The list is a local binding or a parameter whose changes do not reach the caller; a field, a static, a loop binding
//! of an enclosing loop or any other expression keeps the ordinary iteration.
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

/// The `for` loop state of the function body being checked.
#[derive(Debug, Default, Clone)]
pub(super) struct ForItemTaking {
    /// Lists whose items a checked `for` loop takes, keyed by the list's name.
    taken_lists: HashMap<String, TakenList>,
    /// The bindings of the `for` patterns checked so far in this body.
    loop_bindings: HashSet<SymbolId>,
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
}

impl TypeChecker {
    /// Decide, before its body is checked, whether a `for` loop takes the items out of the list it iterates.
    ///
    /// A loop takes the items when its iterable names a list binding the loop may empty, the item type can be neither
    /// copied nor cloned, and the body hands a pattern binding on by value. The decision is recorded for lowering, and
    /// the list is watched for reads until the function ends; an enclosing loop that repeats the loop over a list
    /// built outside it is refused here.
    pub(super) fn plan_for_item_taking(&mut self, for_stmt: &ForStmt, item_ty: &ResolvedType) {
        let Expr::Ident(list) = &for_stmt.iter.node else {
            return;
        };
        if !self.item_leaves_list_by_value(item_ty) {
            return;
        }
        let mut names = HashSet::new();
        pattern_binding_names(&for_stmt.pattern.node, &mut names);
        if !body_hands_on(&for_stmt.body, &names) {
            return;
        }
        let Some(symbol) = self.symbols.lookup(list) else {
            return;
        };
        let Some(definition) = self.list_binding_definition(symbol) else {
            return;
        };
        self.type_info.record_for_loop_takes_items(for_stmt.iter.span);
        let repeated = self
            .loop_stack
            .last()
            .is_some_and(|enclosing| definition.scope < enclosing.scope);
        if repeated {
            self.errors.push(errors::taken_list_used_again(
                list,
                &item_ty.to_string(),
                TakenListReuse::RepeatedLoop,
                for_stmt.iter.span,
                definition.span,
            ));
            return;
        }
        self.for_item_taking.taken_lists.insert(
            list.clone(),
            TakenList {
                symbol,
                loop_span: for_stmt.iter.span,
                body_end: for_stmt
                    .body
                    .last()
                    .map_or(for_stmt.iter.span.end, |stmt| stmt.span.end),
                item_ty: item_ty.to_string(),
            },
        );
    }

    /// Remember the bindings a `for` pattern introduced, so a nested loop over one of them keeps ordinary iteration.
    pub(super) fn remember_for_pattern_bindings(&mut self, pattern: &Pattern) {
        let mut names = HashSet::new();
        pattern_binding_names(pattern, &mut names);
        for name in names {
            if let Some(symbol) = self.symbols.lookup(name) {
                self.for_item_taking.loop_bindings.insert(symbol);
            }
        }
    }

    /// Stop watching a list binding that a statement assigns again.
    pub(super) fn forget_taken_list(&mut self, name: &str) {
        self.for_item_taking.taken_lists.remove(name);
    }

    /// Start a fresh `for` loop state for a function body, returning the enclosing body's state to restore after it.
    pub(super) fn enter_for_item_taking_scope(&mut self) -> ForItemTaking {
        std::mem::take(&mut self.for_item_taking)
    }

    /// Restore the enclosing body's `for` loop state once a function body has been checked.
    pub(super) fn exit_for_item_taking_scope(&mut self, previous: ForItemTaking) {
        self.for_item_taking = previous;
    }

    /// Refuse a read of a list whose items a `for` loop takes, inside that loop's body or after it (`INCAN-T0119`).
    ///
    /// Only a read that follows the loop's iterable in the source is one: the header's own read, and a read an earlier
    /// statement makes when the checker visits it again, come before it.
    pub(super) fn refuse_read_of_taken_list(&mut self, name: &str, symbol: SymbolId, span: Span) {
        let Some(taken) = self.for_item_taking.taken_lists.get(name) else {
            return;
        };
        if taken.symbol != symbol
            || span.start < taken.loop_span.end
            || self
                .errors
                .iter()
                .any(|error| error.span == span && error.stable_code() == Some("INCAN-T0119"))
        {
            return;
        }
        let reuse = if span.end <= taken.body_end {
            TakenListReuse::InsideLoop
        } else {
            TakenListReuse::AfterLoop
        };
        let error = errors::taken_list_used_again(name, &taken.item_ty, reuse, span, taken.loop_span);
        self.errors.push(error);
    }

    /// Whether a `for` item of this type leaves the list when the loop body hands it on by value: an item the checker
    /// knows can be neither copied nor cloned.
    ///
    /// Only a compiler-known surface type the clone check refuses, such as `JoinHandle[T]`, `Mutex` or `Sender[T]`, is
    /// such an item, alone or inside a tuple, a collection or a declared generic type. A declared class, model, newtype
    /// or enum derives `Clone`, even when a surface type shares its name; a type parameter gets a `Clone` bound from
    /// the backend when an item is copied out; and a Rust type's `Clone` is not known to the checker. Items of
    /// those types keep being copied out of the list, as they always were.
    fn item_leaves_list_by_value(&self, item_ty: &ResolvedType) -> bool {
        match item_ty {
            ResolvedType::Tuple(items) => items.iter().any(|item| self.item_leaves_list_by_value(item)),
            ResolvedType::Named(name) if self.is_declared_value_type(name) => false,
            ResolvedType::Generic(name, args)
                if self.is_declared_value_type(name)
                    || collection_type_id(name).is_some()
                    || matches!(
                        surface_types::from_str(name),
                        Some(SurfaceTypeId::Vec | SurfaceTypeId::HashMap)
                    ) =>
            {
                args.iter().any(|arg| self.item_leaves_list_by_value(arg))
            }
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) if surface_types::from_str(name).is_some() => {
                !self.is_clone_type(item_ty)
            }
            _ => false,
        }
    }

    /// Whether `name` resolves to a declared class, model, newtype or enum, all of which derive `Clone`.
    fn is_declared_value_type(&self, name: &str) -> bool {
        matches!(
            self.lookup_type_info(name),
            Some(TypeInfo::Class(_) | TypeInfo::Model(_) | TypeInfo::Newtype(_) | TypeInfo::Enum(_))
        )
    }

    /// Return the list binding a `for` loop may take items from: a local list binding other than an enclosing loop's
    /// binding, or a list parameter whose changes do not reach the caller.
    fn list_binding_definition(&self, symbol: SymbolId) -> Option<ListDefinition> {
        let definition = self.symbols.get(symbol)?;
        let SymbolKind::Variable(info) = &definition.kind else {
            return None;
        };
        let ResolvedType::Generic(name, _) = &info.ty else {
            return None;
        };
        if collection_type_id(name) != Some(CollectionTypeId::List) {
            return None;
        }
        let eligible = match &self.symbols.identity_of(symbol)?.kind {
            SemanticSourceTargetKind::Local => !self.for_item_taking.loop_bindings.contains(&symbol),
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

/// Collect the names a `for` pattern binds.
fn pattern_binding_names<'a>(pattern: &'a Pattern, names: &mut HashSet<&'a str>) {
    match pattern {
        Pattern::Binding(name) => {
            names.insert(name.as_str());
        }
        Pattern::Tuple(items) => {
            for item in items {
                pattern_binding_names(&item.node, names);
            }
        }
        _ => {}
    }
}

/// Whether a `for` body hands one of the loop's bindings on by value anywhere, nested blocks included.
fn body_hands_on(body: &[Spanned<Statement>], names: &HashSet<&str>) -> bool {
    !names.is_empty()
        && (statements_hand_on(body, names)
            || any_expr_in_body(body, |expr| {
                expr_hands_on(expr, names) || expr_blocks_hand_on(expr, names)
            }))
}

/// Whether an expression is exactly one of the loop's bindings.
fn is_loop_binding(expr: &Spanned<Expr>, names: &HashSet<&str>) -> bool {
    match &expr.node {
        Expr::Ident(name) => names.contains(name.as_str()),
        Expr::Paren(inner) => is_loop_binding(inner, names),
        _ => false,
    }
}

/// Whether one of the call arguments is a loop binding.
fn args_hand_on(args: &[CallArg], names: &HashSet<&str>) -> bool {
    args.iter().any(|arg| match arg {
        CallArg::Positional(value) | CallArg::Named(_, value) => is_loop_binding(value, names),
        CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => false,
    })
}

/// Whether an expression takes a loop binding by value: the operand of `await` or `yield`, an argument of a call
/// other than a builtin function, or an item of a new tuple, list, set or dict value.
fn expr_hands_on(expr: &Expr, names: &HashSet<&str>) -> bool {
    match expr {
        Expr::Surface(surface) => matches!(
            (&surface.key, &surface.payload),
            (
                SurfaceFeatureKey::SoftKeyword(KeywordId::Await),
                SurfaceExprPayload::PrefixUnary(operand),
            ) if is_loop_binding(operand, names)
        ),
        Expr::Call(callee, _, args) => {
            let builtin = matches!(&callee.node, Expr::Ident(name) if builtins::from_str(name).is_some());
            !builtin && args_hand_on(args, names)
        }
        Expr::MethodCall(_, _, _, args) | Expr::Constructor(_, args) => args_hand_on(args, names),
        Expr::Yield(Some(value)) => is_loop_binding(value, names),
        Expr::Tuple(items) | Expr::Set(items) => items.iter().any(|item| is_loop_binding(item, names)),
        Expr::List(entries) => entries
            .iter()
            .any(|entry| matches!(entry, ListEntry::Element(value) if is_loop_binding(value, names))),
        Expr::Dict(entries) => entries
            .iter()
            .any(|entry| matches!(entry, DictEntry::Pair(_, value) if is_loop_binding(value, names))),
        _ => false,
    }
}

/// Whether a statement block held by an expression (a `match` arm, an `if` or `loop:` expression, a `race for` arm)
/// hands a loop binding on at statement level.
fn expr_blocks_hand_on(expr: &Expr, names: &HashSet<&str>) -> bool {
    match expr {
        Expr::Match(_, arms) => arms
            .iter()
            .any(|arm| matches!(&arm.node.body, MatchBody::Block(body) if statements_hand_on(body, names))),
        Expr::If(if_expr) => {
            statements_hand_on(&if_expr.then_body, names)
                || if_expr
                    .else_body
                    .as_ref()
                    .is_some_and(|else_body| statements_hand_on(else_body, names))
        }
        Expr::Loop(loop_expr) => statements_hand_on(&loop_expr.body, names),
        Expr::Surface(surface) => match &surface.payload {
            SurfaceExprPayload::RaceFor(race) => race
                .arms
                .iter()
                .any(|arm| matches!(&arm.body, RaceForBody::Block(body) if statements_hand_on(body, names))),
            _ => false,
        },
        _ => false,
    }
}

/// Whether a statement list, or a statement block nested in it, returns, breaks with, or assigns a loop binding.
///
/// Blocks held by expressions are reached through [`expr_blocks_hand_on`] from the expression walk.
fn statements_hand_on(body: &[Spanned<Statement>], names: &HashSet<&str>) -> bool {
    body.iter().any(|stmt| match &stmt.node {
        Statement::Return(Some(value)) | Statement::Break(Some(value)) => is_loop_binding(value, names),
        Statement::Assignment(assign) => is_loop_binding(&assign.value, names),
        Statement::If(if_stmt) => {
            statements_hand_on(&if_stmt.then_body, names)
                || if_stmt
                    .elif_branches
                    .iter()
                    .any(|(_, branch)| statements_hand_on(branch, names))
                || if_stmt
                    .else_body
                    .as_ref()
                    .is_some_and(|else_body| statements_hand_on(else_body, names))
        }
        Statement::Loop(loop_stmt) => statements_hand_on(&loop_stmt.body, names),
        Statement::While(while_stmt) => statements_hand_on(&while_stmt.body, names),
        Statement::For(for_stmt) => statements_hand_on(&for_stmt.body, names),
        Statement::Unsafe(unsafe_stmt) => statements_hand_on(&unsafe_stmt.body, names),
        _ => false,
    })
}
