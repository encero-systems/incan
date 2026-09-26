//! Building a builtin-family method call: the arguments of a call on module static storage, and the value a dict's
//! `get` returns.

use std::collections::HashSet;

use super::super::super::TypedExpr;
use super::super::super::expr::{
    CollectionMethodKind, IrCallArg, IrExprKind, IrInteropCoercionKind, MethodCallArgPolicy, MethodKind,
    StringMethodKind, VarAccess, VarRefKind,
};
use super::super::super::types::{IrType, is_string_storage_type};
use super::super::AstLowering;
use incan_frontend::ast::{self, Spanned};
use incan_frontend::typechecker::IdentKind;

impl AstLowering {
    /// Build a builtin-family method call at `call_span`, returning it with its IR type.
    ///
    /// On module static storage, each argument is prepared for the temporary emission binds it to before it enters
    /// the storage access (see [`Self::prepare_static_method_arg`]). A dict's `get` answers with the stored value
    /// (`Option[V]`, `result_ty`): on static storage the storage access copies the entry out. On any other dict the
    /// lookup reads the entry in place. When the checker recorded that the result is only read (a `match` or `if let`
    /// whose bindings are only read), the call stays that in-place read and its IR type says so (`Option[&V]` in Rust);
    /// otherwise it is completed with a copy of the entry it finds, `copied` for a `Copy` value and `cloned` otherwise.
    pub(in crate::lower) fn known_method_call(
        &self,
        call_span: ast::Span,
        receiver: TypedExpr,
        kind: MethodKind,
        mut args: Vec<IrCallArg>,
        result_ty: IrType,
    ) -> (IrExprKind, IrType) {
        let reads_static = Self::expr_reads_static_storage(&receiver);
        if reads_static {
            let mut receiver_reads = HashSet::new();
            Self::collect_read_names(&receiver, &mut receiver_reads);
            for arg in &mut args {
                Self::prepare_static_method_arg(&receiver, &kind, &mut arg.expr, &receiver_reads);
            }
        }
        let in_place_get_value = if !reads_static && matches!(kind, MethodKind::Collection(CollectionMethodKind::Get)) {
            Self::dict_value_type(&receiver.ty)
        } else {
            None
        };
        let call = IrExprKind::KnownMethodCall {
            receiver: Box::new(receiver),
            kind,
            args,
        };
        let only_read = self
            .type_info
            .as_ref()
            .is_some_and(|info| info.is_read_only_dict_lookup(call_span));
        match in_place_get_value {
            Some(value_ty) if only_read => (call, Self::found_entry_type(value_ty)),
            Some(value_ty) => (Self::copied_dict_entry(call, value_ty), result_ty),
            None => (call, result_ty),
        }
    }

    /// The Rust shape of an in-place dict lookup's result: the entry it finds, read where it is stored.
    fn found_entry_type(value_ty: IrType) -> IrType {
        IrType::Option(Box::new(IrType::Ref(Box::new(value_ty))))
    }

    /// Keep the arguments of a Rust collection method on module static storage readable after the call.
    ///
    /// A static `BTreeMap`, `HashMap` or `HashSet` reached through `rust::` is called as an ordinary method, and
    /// emission binds its arguments to temporaries before the storage access the same way. A lookup there passes its
    /// argument in the written shape, so an argument the program reads again (after the call, or in the receiver's
    /// path) is handed over as a copy of itself, as it is for a builtin method that stores its argument (#1793).
    pub(in crate::lower) fn keep_rust_collection_static_args_readable(receiver: &TypedExpr, args: &mut [IrCallArg]) {
        if !Self::expr_reads_static_storage(receiver)
            || Self::rust_collection_family_for_ir_type(&receiver.ty).is_none()
        {
            return;
        }
        let mut receiver_reads = HashSet::new();
        Self::collect_read_names(receiver, &mut receiver_reads);
        for arg in args {
            if Self::static_method_arg_is_read_elsewhere(&arg.expr, &receiver_reads) {
                let value = std::mem::replace(&mut arg.expr, TypedExpr::new(IrExprKind::Unit, IrType::Unit));
                arg.expr = Self::copied_static_method_arg(value);
            }
        }
    }

    /// Return the value type of a dict receiver, looking through the reference wrapper of a `mut` parameter.
    fn dict_value_type(ty: &IrType) -> Option<IrType> {
        match ty {
            IrType::Ref(inner) | IrType::RefMut(inner) => Self::dict_value_type(inner),
            IrType::Dict(_, value) => Some(value.as_ref().clone()),
            _ => None,
        }
    }

    /// Complete an in-place dict lookup with a copy of the entry it finds, so the call answers with `Option[V]`.
    fn copied_dict_entry(lookup: IrExprKind, value_ty: IrType) -> IrExprKind {
        let method = if value_ty.is_copy() { "copied" } else { "cloned" };
        IrExprKind::MethodCall {
            receiver: Box::new(TypedExpr::new(lookup, Self::found_entry_type(value_ty))),
            method: method.to_string(),
            dispatch: None,
            type_args: Vec::new(),
            args: Vec::new(),
            callable_signature: None,
            arg_policy: MethodCallArgPolicy::Default,
        }
    }

    /// Return whether an expression reads module static storage: a static, a local bound directly to one, or a field
    /// or index path rooted at either.
    ///
    /// These are the receivers whose method calls the emitter runs inside the static's storage access, after binding
    /// every argument to a temporary so an argument that reads the same static does not re-enter it.
    fn expr_reads_static_storage(expr: &TypedExpr) -> bool {
        match &expr.kind {
            IrExprKind::StaticRead { .. }
            | IrExprKind::Var {
                ref_kind: VarRefKind::StaticBinding,
                ..
            } => true,
            IrExprKind::Field { object, .. } | IrExprKind::Index { object, .. } => {
                Self::expr_reads_static_storage(object)
            }
            _ => false,
        }
    }

    /// Return whether an assignment target's object path is rooted in module static storage, judged from the source
    /// before any of it is lowered.
    ///
    /// An assignment through such a path evaluates its value first and reads the path and the index inside the
    /// static's storage access afterwards, so the caller lowers the value first and a variable's last read is the one
    /// the generated code performs last.
    pub(in crate::lower) fn ast_path_reads_static_storage(&self, expr: &Spanned<ast::Expr>) -> bool {
        match &expr.node {
            ast::Expr::Ident(name) => {
                matches!(self.ident_kind_for_lowering(expr), Some(IdentKind::Static)) || self.is_static_binding(name)
            }
            ast::Expr::Field(object, _) | ast::Expr::Index(object, _) | ast::Expr::Paren(object) => {
                self.ast_path_reads_static_storage(object)
            }
            _ => false,
        }
    }

    /// Collect the names of the local values an expression reads, such as the index variables of a storage path.
    fn collect_read_names(expr: &TypedExpr, names: &mut HashSet<String>) {
        match &expr.kind {
            IrExprKind::Var {
                name,
                ref_kind: VarRefKind::Value,
                ..
            } => {
                names.insert(name.clone());
            }
            IrExprKind::Field { object, .. } => Self::collect_read_names(object, names),
            IrExprKind::Index { object, index } => {
                Self::collect_read_names(object, names);
                Self::collect_read_names(index, names);
            }
            IrExprKind::BinOp { left, right, .. } => {
                Self::collect_read_names(left, names);
                Self::collect_read_names(right, names);
            }
            IrExprKind::UnaryOp { operand: inner, .. }
            | IrExprKind::Cast { expr: inner, .. }
            | IrExprKind::InteropCoerce { expr: inner, .. } => Self::collect_read_names(inner, names),
            IrExprKind::MethodCall { receiver, args, .. } | IrExprKind::KnownMethodCall { receiver, args, .. } => {
                Self::collect_read_names(receiver, names);
                for arg in args {
                    Self::collect_read_names(&arg.expr, names);
                }
            }
            IrExprKind::Call { args, .. } => {
                for arg in args {
                    Self::collect_read_names(&arg.expr, names);
                }
            }
            IrExprKind::Tuple(items) => {
                for item in items {
                    Self::collect_read_names(item, names);
                }
            }
            _ => {}
        }
    }

    /// Hand one argument of a builtin method on module static storage to the temporary the emitter binds it to.
    ///
    /// The emitter binds each argument to a temporary before it enters the static's storage access, and the method
    /// then reads that temporary by its type. Two argument shapes need preparing for it:
    ///
    /// - An argument that names a value the program still reads — a variable read again after the call or by the
    ///   receiver's own path (`table[key].get(key)`, whose path the storage access reads after the temporary is bound),
    ///   or a field of another value — would be taken over by the binding, but Incan source has no way to give a value
    ///   up to a call (#1793). A method that reads the argument in place (see
    ///   [`Self::static_method_reads_arg_in_place`]) is handed a view of it; any other method is handed a copy, keeping
    ///   the argument's type, so it reads the temporary exactly as it reads a moved value whatever Rust shape the
    ///   variable has (a loop variable is ordinarily a view of its element already).
    /// - A string literal is a `'static` string, and the temporary holds it as one. Typing it that way lets a method
    ///   that stores the argument (`counts.insert("a", 1)`, `names.append("a")`) make the owned `str` the static holds,
    ///   and a lookup read it as it is.
    ///
    /// Any other argument (a variable at its last read, a `Copy` value, a call result, an f-string) is handed over as
    /// it is.
    fn prepare_static_method_arg(
        receiver: &TypedExpr,
        kind: &MethodKind,
        arg: &mut TypedExpr,
        receiver_reads: &HashSet<String>,
    ) {
        if matches!(arg.kind, IrExprKind::String(_)) && matches!(arg.ty, IrType::String) {
            arg.ty = IrType::StaticStr;
            return;
        }
        if !Self::static_method_arg_is_read_elsewhere(arg, receiver_reads) {
            return;
        }
        let in_place = Self::static_method_reads_arg_in_place(receiver, kind, &arg.ty);
        let value = std::mem::replace(arg, TypedExpr::new(IrExprKind::Unit, IrType::Unit));
        *arg = if in_place {
            Self::viewed_static_method_arg(value)
        } else {
            Self::copied_static_method_arg(value)
        };
    }

    /// Whether binding this argument to a temporary would take over a value the program still reads: after the call,
    /// or in the receiver's own path.
    fn static_method_arg_is_read_elsewhere(expr: &TypedExpr, receiver_reads: &HashSet<String>) -> bool {
        if expr.ty.is_copy() || matches!(expr.ty, IrType::Unknown) {
            return false;
        }
        match &expr.kind {
            IrExprKind::Var {
                name, access, ref_kind, ..
            } => {
                *ref_kind == VarRefKind::Value && (!matches!(access, VarAccess::Move) || receiver_reads.contains(name))
            }
            IrExprKind::Field { .. } => true,
            _ => false,
        }
    }

    /// Whether the method reads this argument in place, so a view of it serves as well as the value.
    ///
    /// These are the methods whose runtime reads the argument as `str` text (a string key or element looked up with
    /// `get`, `in` or `contains`; `startswith`, `endswith` and `contains` on a static string) or as a list's items
    /// (`extend`, `join`). A method that stores the argument (`insert`, `append`, `add`), or compares a key or
    /// element that is not a string, is handed a copy.
    fn static_method_reads_arg_in_place(receiver: &TypedExpr, kind: &MethodKind, arg_ty: &IrType) -> bool {
        let mut receiver_ty = &receiver.ty;
        while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
            receiver_ty = inner.as_ref();
        }
        match kind {
            MethodKind::Collection(CollectionMethodKind::Get | CollectionMethodKind::Contains) => {
                let probed = match receiver_ty {
                    IrType::Dict(key, _) => key.as_ref(),
                    IrType::List(item) | IrType::Set(item) => item.as_ref(),
                    _ => return false,
                };
                is_string_storage_type(probed) && matches!(arg_ty, IrType::String)
            }
            MethodKind::Collection(CollectionMethodKind::Extend) => true,
            MethodKind::String(
                StringMethodKind::StartsWith
                | StringMethodKind::EndsWith
                | StringMethodKind::Contains
                | StringMethodKind::Join,
            ) => true,
            _ => false,
        }
    }

    /// Hand one argument over as a view of itself, keeping its type.
    fn viewed_static_method_arg(value: TypedExpr) -> TypedExpr {
        let ty = value.ty.clone();
        let span = value.span;
        TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(value),
                from_ty: ty.clone(),
                to_ty: ty.clone(),
                kind: IrInteropCoercionKind::RustBorrow { mutable: false },
            },
            ty,
        )
        .with_span(span)
    }

    /// Hand one argument over as a `.clone()` of itself, keeping its type.
    fn copied_static_method_arg(value: TypedExpr) -> TypedExpr {
        let ty = value.ty.clone();
        let span = value.span;
        TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(value),
                method: "clone".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            ty,
        )
        .with_span(span)
    }
}
