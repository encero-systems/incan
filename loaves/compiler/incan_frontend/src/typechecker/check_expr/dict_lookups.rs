//! What a `dict.get(key)` result is used for.
//!
//! `get` answers with `Option[V]`, the stored value, on every dict. Lowering reads the entry in place when the result
//! is only read, and completes the lookup with a copy of the entry otherwise. The checker decides which: a lookup that
//! feeds a `match`, `if let` or `while let` whose bindings are only read is recorded as read-only (see
//! [`crate::typechecker::TypeCheckInfo::is_read_only_dict_lookup`]); every other lookup keeps its own value, and one
//! whose value type is proven unable to be copied is refused.

use crate::ast::{CallArg, Expr, FStringPart, MatchArm, MatchBody, Pattern, PatternArg, Span, Spanned, Statement};
use crate::ast_walk::{any_expr_in_body, any_expr_in_expr};
use crate::diagnostics::CompileError;
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
            self.errors.push(
                CompileError::type_error(
                    format!("`get` here has to return its own `{value}`, and `{value}` cannot be copied"),
                    span,
                )
                .with_hint(
                    "A lookup that only reads the value works: `match table.get(key):` or `if let Some(item) = table.get(key):` with arms that only read `item`",
                )
                .with_note("`get` returns the stored value; the result is kept here, so the lookup needs a copy of it"),
            );
        }
    }

    /// Record `value` as read-only when it is an in-place dict lookup and every arm only reads its bindings.
    fn note_dict_lookup_arms(&mut self, value: &Spanned<Expr>, arms: &[LookupArm<'_>]) {
        if !self.is_in_place_dict_lookup(value) {
            return;
        }
        let only_read = arms.iter().all(|arm| {
            let mut names = Vec::new();
            collect_pattern_bindings(&arm.pattern.node, &mut names);
            names.iter().all(|name| self.binding_is_only_read(name, arm))
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

    /// Whether an already-checked expression reads module static storage: an identifier that resolved to a `static`,
    /// or a field or index path rooted at one.
    fn expr_reads_static_storage(&self, expr: &Spanned<Expr>) -> bool {
        match &expr.node {
            Expr::Ident(_) => self.type_info.ident_kind(expr.span) == Some(IdentKind::Static),
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

    /// Whether inspected Rust metadata proves that `ty` cannot be copied (it implements no `Clone`).
    ///
    /// Incan types are always copyable, a type parameter is given the bound where a copy is made, and a Rust type
    /// without trait metadata is not refused.
    fn value_type_cannot_be_copied(&self, ty: &ResolvedType) -> bool {
        let ResolvedType::RustPath(path) = ty else {
            return false;
        };
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
