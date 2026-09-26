//! What a `dict.get(key)` result is used for.
//!
//! `get` answers with `Option[V]`, the stored value, on every dict. Lowering reads the entry in place when the result
//! is only read, and completes the lookup with a copy of the entry otherwise. The checker decides which: a lookup that
//! feeds a `match`, `if let` or `while let` whose bindings are only read, while the dict itself is left alone, is
//! recorded as read-only (see [`crate::typechecker::TypeCheckInfo::is_read_only_dict_lookup`]); every other lookup
//! keeps its own value, and one whose value type cannot be copied is refused with `INCAN-T0118`.

use crate::ast::{
    AssignmentStmt, CallArg, Expr, FStringPart, MatchArm, MatchBody, Pattern, PatternArg, Span, Spanned, Statement,
};
use crate::ast_walk::{any_expr_in_body, any_expr_in_expr};
use crate::diagnostics::errors;
use crate::symbols::ResolvedType;
use crate::typechecker::IdentKind;
use crate::typechecker::helpers::collection_type_id;
use incan_lang::interop::RustItemKind;
use incan_lang::lang::traits::{self, TraitId};
use incan_lang::lang::types::collections::CollectionTypeId;

use super::TypeChecker;

/// Where the bindings of one arm are read: its guard, when it has one, and its body.
struct LookupArm<'a> {
    pattern: &'a Spanned<Pattern>,
    guard: Option<&'a Spanned<Expr>>,
    body: LookupArmBody<'a>,
}

/// The body of one arm over a lookup result.
enum LookupArmBody<'a> {
    Block(&'a [Spanned<Statement>]),
    Expr(&'a Spanned<Expr>),
}

/// The value a dict lookup reads from: a named local or parameter, or `self` for a field path of the receiver.
enum LookupRoot {
    Name(String),
    SelfValue,
}

impl TypeChecker {
    /// After a `match` over `subject` is checked, record `subject` as a read-only dict lookup when it is
    /// `dict.get(key)` and every binding its arms introduce is only read.
    pub(in crate::typechecker) fn note_dict_lookup_match(
        &mut self,
        subject: &Spanned<Expr>,
        arms: &[Spanned<MatchArm>],
    ) {
        let arms = arms
            .iter()
            .map(|arm| LookupArm {
                pattern: &arm.node.pattern,
                guard: arm.node.guard.as_ref(),
                body: match &arm.node.body {
                    MatchBody::Expr(expr) => LookupArmBody::Expr(expr),
                    MatchBody::Block(stmts) => LookupArmBody::Block(stmts),
                },
            })
            .collect::<Vec<_>>();
        self.note_dict_lookup_arms(subject, &arms);
    }

    /// After an `if let` or `while let` over `value` is checked, record `value` as a read-only dict lookup when it is
    /// `dict.get(key)` and every binding `pattern` introduces is only read in `body`.
    pub(in crate::typechecker) fn note_dict_lookup_let(
        &mut self,
        value: &Spanned<Expr>,
        pattern: &Spanned<Pattern>,
        body: &[Spanned<Statement>],
    ) {
        self.note_dict_lookup_arms(
            value,
            &[LookupArm {
                pattern,
                guard: None,
                body: LookupArmBody::Block(body),
            }],
        );
    }

    /// Remember a `dict.get(key)` call whose value type is proven unable to be copied, so it is refused at the end of
    /// checking unless its result turns out to be only read.
    pub(in crate::typechecker) fn note_dict_lookup_value(&mut self, call_span: Span, value_ty: &ResolvedType) {
        if self.value_type_cannot_be_copied(value_ty) {
            self.pending_uncopyable_dict_lookups
                .push((call_span, value_ty.to_string()));
        }
    }

    /// Refuse every remembered lookup that keeps its own copy of a value that cannot be copied.
    ///
    /// Such a lookup would complete with a copy of the entry, which the value type does not provide; a lookup whose
    /// result is only read is recorded read-only by then and is not refused.
    pub(in crate::typechecker) fn refuse_copying_dict_lookups_of_uncopyable_values(&mut self) {
        for (span, value) in std::mem::take(&mut self.pending_uncopyable_dict_lookups) {
            if self.type_info.is_read_only_dict_lookup(span) {
                continue;
            }
            self.errors
                .push(errors::kept_dict_lookup_value_cannot_be_copied(&value, span));
        }
    }

    /// Remember a local bound directly to a module static (`live = counts`).
    ///
    /// Lowering reads such a local through the static's storage access like the static itself, so its lookups always
    /// copy the entry out and are never recorded as read-only.
    pub(in crate::typechecker) fn note_static_alias_binding(&mut self, assign: &AssignmentStmt) {
        if matches!(assign.value.node, Expr::Ident(_))
            && self.type_info.ident_kind(assign.value.span) == Some(IdentKind::Static)
            && let Some(id) = self.symbols.lookup(&assign.name)
        {
            self.static_alias_bindings.insert(id);
        }
    }

    /// Record `value` as read-only when it is an in-place dict lookup, every arm only reads its bindings, and no arm
    /// touches the dict while a binding is still read.
    fn note_dict_lookup_arms(&mut self, value: &Spanned<Expr>, arms: &[LookupArm<'_>]) {
        if !self.is_in_place_dict_lookup(value) {
            return;
        }
        let root = match &value.node {
            Expr::MethodCall(base, ..) => lookup_root(base),
            _ => None,
        };
        let only_read = arms.iter().all(|arm| {
            let mut names = Vec::new();
            collect_pattern_bindings(&arm.pattern.node, &mut names);
            names.iter().all(|name| self.binding_is_only_read(name, arm))
                && !root_touched_while_bound(root.as_ref(), &names, arm)
        });
        if only_read {
            self.type_info.record_read_only_dict_lookup(value.span);
        }
    }

    /// Whether `expr` is `dict.get(key)` on a builtin dict that is not module static storage.
    ///
    /// A static dict is read through its storage access, which always copies the entry out.
    fn is_in_place_dict_lookup(&self, expr: &Spanned<Expr>) -> bool {
        let Expr::MethodCall(base, method, _, args) = &expr.node else {
            return false;
        };
        method == "get" && args.len() == 1 && self.expr_is_builtin_dict(base) && !self.expr_reads_static_storage(base)
    }

    /// Whether the checked type of `expr` is a builtin dict, looking through a `mut` parameter's wrapper.
    fn expr_is_builtin_dict(&self, expr: &Spanned<Expr>) -> bool {
        let mut ty = self.type_info.expr_type(expr.span);
        while let Some(ResolvedType::Ref(inner) | ResolvedType::RefMut(inner)) = ty {
            ty = Some(inner.as_ref());
        }
        matches!(ty, Some(ResolvedType::Generic(name, _)) if collection_type_id(name.as_str()) == Some(CollectionTypeId::Dict))
    }

    /// Whether an already-checked expression reads module static storage: an identifier that resolved to a `static`
    /// or to a local bound directly to one, or a field or index path rooted at either.
    fn expr_reads_static_storage(&self, expr: &Spanned<Expr>) -> bool {
        match &expr.node {
            Expr::Ident(name) => {
                self.type_info.ident_kind(expr.span) == Some(IdentKind::Static)
                    || self
                        .symbols
                        .lookup(name)
                        .is_some_and(|id| self.static_alias_bindings.contains(&id))
            }
            Expr::Field(object, _) | Expr::Index(object, _) | Expr::Paren(object) => {
                self.expr_reads_static_storage(object)
            }
            _ => false,
        }
    }

    /// Whether the binding `name` is only read in `arm`: every use is the argument of `len(...)`, `print(...)` or
    /// `println(...)`, an f-string interpolation, or the receiver of a Rust method that takes its receiver shared.
    fn binding_is_only_read(&self, name: &str, arm: &LookupArm<'_>) -> bool {
        let mut uses = 0usize;
        let mut reads = 0usize;
        let mut count = |expr: &Expr| {
            self.count_binding_uses(name, expr, &mut uses, &mut reads);
            false
        };
        if let Some(guard) = arm.guard {
            any_expr_in_expr(&guard.node, &mut count);
        }
        match arm.body {
            LookupArmBody::Block(stmts) => {
                any_expr_in_body(stmts, &mut count);
            }
            LookupArmBody::Expr(expr) => {
                any_expr_in_expr(&expr.node, &mut count);
            }
        }
        uses == reads
    }

    /// Count the uses of `name` directly in `expr` (`uses`) and the uses of those that only read it (`reads`).
    fn count_binding_uses(&self, name: &str, expr: &Expr, uses: &mut usize, reads: &mut usize) {
        let names_binding = |expr: &Spanned<Expr>| matches!(&expr.node, Expr::Ident(ident) if ident == name);
        match expr {
            Expr::Ident(ident) if ident == name => *uses += 1,
            Expr::Call(callee, _, args) => {
                let Expr::Ident(callee_name) = &callee.node else {
                    return;
                };
                let direct = args
                    .iter()
                    .filter(|arg| matches!(arg, CallArg::Positional(arg) if names_binding(arg)))
                    .count();
                match callee_name.as_str() {
                    "len" if args.len() == 1 => *reads += direct,
                    "print" | "println" => *reads += direct,
                    _ => {}
                }
            }
            Expr::FString(parts) => {
                *reads += parts
                    .iter()
                    .filter(|part| matches!(part, FStringPart::Expr { expr, .. } if names_binding(expr)))
                    .count();
            }
            Expr::MethodCall(base, _, _, _) if names_binding(base) && self.rust_receiver_is_shared(base.span) => {
                *reads += 1;
            }
            _ => {}
        }
    }

    /// Whether the Rust method called on the receiver at `receiver_span` takes its receiver shared.
    ///
    /// The receiver contract is keyed by the call's span, which starts where its receiver starts; the innermost call
    /// on that receiver is the one that ends first after it.
    fn rust_receiver_is_shared(&self, receiver_span: Span) -> bool {
        self.type_info
            .rust
            .receiver_contracts
            .iter()
            .filter(|((start, end), _)| *start == receiver_span.start && *end > receiver_span.end)
            .min_by_key(|((_, end), _)| *end)
            .is_some_and(|(_, contract)| contract.shared)
    }

    /// Whether a value of type `ty` cannot be copied, so a lookup that keeps one is refused.
    ///
    /// This asks the relation `is_clone_type` answers, looking through containers, with two differences: a type
    /// parameter is given the `Clone` bound where a lookup copies it (see trait bound inference), and a Rust type
    /// counts as not copyable only when its inspected metadata proves it implements no `Clone` (`is_clone_type`
    /// does not ask Rust, so it answers `false` for every Rust type).
    fn value_type_cannot_be_copied(&self, ty: &ResolvedType) -> bool {
        match ty {
            ResolvedType::RustPath(path) => self.rust_type_proven_not_clone(path),
            ResolvedType::TypeVar(_) => false,
            ResolvedType::Generic(name, _)
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Generator) =>
            {
                !self.is_clone_type(ty)
            }
            ResolvedType::Generic(_, args) | ResolvedType::Tuple(args) => {
                args.iter().any(|arg| self.value_type_cannot_be_copied(arg))
            }
            ResolvedType::FrozenList(inner) | ResolvedType::FrozenSet(inner) => self.value_type_cannot_be_copied(inner),
            ResolvedType::FrozenDict(key, value) => {
                self.value_type_cannot_be_copied(key) || self.value_type_cannot_be_copied(value)
            }
            other => !self.is_clone_type(other),
        }
    }

    /// Whether inspected Rust metadata proves the Rust type at `path` implements no `Clone`.
    fn rust_type_proven_not_clone(&self, path: &str) -> bool {
        let base = path.split('<').next().unwrap_or(path).trim();
        let Some(metadata) = self.rust_item_metadata_for_path(base) else {
            return false;
        };
        let RustItemKind::Type(info) = &metadata.kind else {
            return false;
        };
        if !info.metadata_completeness.has_trait_impls() {
            return false;
        }
        let is_clone =
            |path: &str| traits::rust_paths(TraitId::Clone).contains(&path) || path == traits::as_str(TraitId::Clone);
        !(info
            .implemented_traits
            .iter()
            .any(|implemented| is_clone(&implemented.path))
            || info
                .expanded_derive_traits
                .iter()
                .any(|implemented| is_clone(&implemented.path)))
    }
}

/// The value the receiver of `dict.get` reads from, when it is a named local, a parameter, or `self`.
fn lookup_root(receiver: &Spanned<Expr>) -> Option<LookupRoot> {
    match &receiver.node {
        Expr::Ident(name) if name == "self" => Some(LookupRoot::SelfValue),
        Expr::Ident(name) => Some(LookupRoot::Name(name.clone())),
        Expr::SelfExpr => Some(LookupRoot::SelfValue),
        Expr::Field(object, _) | Expr::Index(object, _) | Expr::Paren(object) => lookup_root(object),
        _ => None,
    }
}

/// Whether an arm touches the looked-up dict before its bindings' last read.
///
/// A binding that only reads the entry in place keeps the dict read for as long as the binding is read, so any use of
/// the dict's root in that stretch (assigning, indexing into, calling a method on, or passing it on) needs the entry
/// to be the lookup's own copy. The stretch runs from the arm's guard through the last statement that reads a binding;
/// statements after that, and arms with no binding read, are free to change the dict. A lookup whose root is not a
/// named value or `self` is always read from a temporary and never conflicts.
fn root_touched_while_bound(root: Option<&LookupRoot>, names: &[String], arm: &LookupArm<'_>) -> bool {
    let Some(root) = root else {
        return false;
    };
    let reads_binding = |expr: &Expr| matches!(expr, Expr::Ident(ident) if names.iter().any(|name| name == ident));
    let touches_root = |expr: &Expr| {
        match (root, expr) {
        (LookupRoot::Name(name), Expr::Ident(ident)) => ident == name,
        (LookupRoot::SelfValue, Expr::SelfExpr) => true,
        (LookupRoot::SelfValue, Expr::Ident(ident)) => ident == "self",
        (LookupRoot::Name(name), Expr::Match(_, arms)) => arms
            .iter()
            .any(|arm| matches!(&arm.node.body, MatchBody::Block(stmts) if stmts.iter().any(|stmt| stmt_assigns_name(&stmt.node, name)))),
        _ => false,
    }
    };
    let stmt_touches_root = |stmt: &Spanned<Statement>| {
        any_expr_in_body(std::slice::from_ref(stmt), &touches_root)
            || matches!(root, LookupRoot::Name(name) if stmt_assigns_name(&stmt.node, name))
    };
    match arm.body {
        LookupArmBody::Block(stmts) => {
            let Some(last_read) = stmts
                .iter()
                .rposition(|stmt| any_expr_in_body(std::slice::from_ref(stmt), &reads_binding))
            else {
                return false;
            };
            arm.guard
                .is_some_and(|guard| any_expr_in_expr(&guard.node, &touches_root))
                || stmts.iter().take(last_read + 1).any(stmt_touches_root)
        }
        LookupArmBody::Expr(expr) => {
            let binding_read = any_expr_in_expr(&expr.node, &reads_binding)
                || arm
                    .guard
                    .is_some_and(|guard| any_expr_in_expr(&guard.node, &reads_binding));
            binding_read
                && (any_expr_in_expr(&expr.node, &touches_root)
                    || arm
                        .guard
                        .is_some_and(|guard| any_expr_in_expr(&guard.node, &touches_root)))
        }
    }
}

/// Whether a statement, or a statement nested in its blocks, assigns to the local `name`.
fn stmt_assigns_name(stmt: &Statement, name: &str) -> bool {
    let block_assigns = |body: &[Spanned<Statement>]| body.iter().any(|stmt| stmt_assigns_name(&stmt.node, name));
    match stmt {
        Statement::Assignment(assign) => assign.name == name,
        Statement::CompoundAssignment(assign) => assign.name == name,
        Statement::ChainedAssignment(assign) => assign.targets.iter().any(|target| target == name),
        Statement::TupleUnpack(unpack) => unpack.names.iter().any(|target| target == name),
        Statement::If(if_stmt) => {
            block_assigns(&if_stmt.then_body)
                || if_stmt.elif_branches.iter().any(|(_, body)| block_assigns(body))
                || if_stmt.else_body.as_deref().is_some_and(block_assigns)
        }
        Statement::While(while_stmt) => block_assigns(&while_stmt.body),
        Statement::For(for_stmt) => block_assigns(&for_stmt.body),
        Statement::Loop(loop_stmt) => block_assigns(&loop_stmt.body),
        Statement::Unsafe(unsafe_stmt) => block_assigns(&unsafe_stmt.body),
        _ => false,
    }
}

/// Collect the names a pattern binds.
fn collect_pattern_bindings(pattern: &Pattern, names: &mut Vec<String>) {
    match pattern {
        Pattern::Binding(name) => names.push(name.clone()),
        Pattern::Constructor(_, args) => {
            for arg in args {
                match arg {
                    PatternArg::Positional(inner) | PatternArg::Named(_, inner) => {
                        collect_pattern_bindings(&inner.node, names);
                    }
                }
            }
        }
        Pattern::Tuple(items) | Pattern::Or(items) => {
            for item in items {
                collect_pattern_bindings(&item.node, names);
            }
        }
        Pattern::Group(inner) => collect_pattern_bindings(&inner.node, names),
        Pattern::Wildcard | Pattern::Literal(_) => {}
    }
}
