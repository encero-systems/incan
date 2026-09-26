//! The arguments of a builtin method called on module static storage: read again after the call, and stored in the
//! static's own element type.

use super::super::super::TypedExpr;
use super::super::super::expr::{IrCallArg, IrExprKind, MethodCallArgPolicy, MethodKind, VarAccess, VarRefKind};
use super::super::super::types::IrType;
use super::super::AstLowering;

impl AstLowering {
    /// Build a builtin-family method call, preparing its arguments when the receiver is static storage.
    pub(in crate::lower) fn known_method_call(
        receiver: TypedExpr,
        kind: MethodKind,
        mut args: Vec<IrCallArg>,
    ) -> IrExprKind {
        if Self::expr_reads_static_storage(&receiver) {
            for arg in &mut args {
                Self::prepare_static_method_arg(&mut arg.expr);
            }
        }
        IrExprKind::KnownMethodCall {
            receiver: Box::new(receiver),
            kind,
            args,
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

    /// Hand one argument of a builtin method on module static storage to the temporary the emitter binds it to.
    ///
    /// The emitter binds each argument to a temporary before it enters the static's storage access, and the method
    /// then reads that temporary by its type. Two argument shapes need preparing for it:
    ///
    /// - An argument that names a value still read after the call — a variable before its last read, or a field of
    ///   another value — would be taken over by the binding, but Incan source has no way to give a value up to a call:
    ///   a parameter handed to `counts.get(name)` or `names.append(name)` is read again afterwards like any other value
    ///   (#1793). It is handed over as a copy of itself, keeping its type, so the method reads the temporary exactly as
    ///   it reads a moved value whatever Rust shape the variable has (a loop variable is ordinarily a borrow of its
    ///   element already), which a borrow would not.
    /// - A string literal is a `'static` string, and the temporary holds it as one. Typing it that way lets a method
    ///   that stores the argument (`counts.insert("a", 1)`, `names.append("a")`) make the owned `str` the static holds,
    ///   and a lookup read it as it is.
    ///
    /// A variable at its last read, a `Copy` value, and any other temporary (a call result, an f-string) are handed
    /// over as they are.
    fn prepare_static_method_arg(arg: &mut TypedExpr) {
        if matches!(arg.kind, IrExprKind::String(_)) && matches!(arg.ty, IrType::String) {
            arg.ty = IrType::StaticStr;
        } else if Self::static_method_arg_is_read_later(arg) {
            let value = std::mem::replace(arg, TypedExpr::new(IrExprKind::Unit, IrType::Unit));
            *arg = Self::copied_static_method_arg(value);
        }
    }

    /// Whether binding this argument to a temporary would take over a value the program still reads.
    fn static_method_arg_is_read_later(expr: &TypedExpr) -> bool {
        if expr.ty.is_copy() || matches!(expr.ty, IrType::Unknown) {
            return false;
        }
        match &expr.kind {
            IrExprKind::Var { access, ref_kind, .. } => {
                *ref_kind == VarRefKind::Value && !matches!(access, VarAccess::Move)
            }
            IrExprKind::Field { .. } => true,
            _ => false,
        }
    }

    /// Wrap one argument in a `.clone()` of itself, keeping its type.
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
