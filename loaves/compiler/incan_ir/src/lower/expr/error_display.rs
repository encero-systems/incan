//! Display of `Error` adopters that define no `__str__` (#1778).
//!
//! A model or class has a Rust `Display` only through `__str__`. An `Error` adopter carries its human-readable text in
//! `message()` instead, so the typechecker records every display operand of such a type -- an f-string `{value}` part,
//! the argument of `str(value)`, and each `print`/`println` argument, whichever way the builtin is spelled -- together
//! with the `message()` call it resolved for it. Lowering hands the emitter that call in the operand's place, a `str`,
//! which every display position already renders.
//!
//! Such an adopter also satisfies a `Display` bound, which a generic body displays through Rust's `Display` rather
//! than through a rewritten operand. The typechecker therefore records the same `message()` call once on each such type
//! the module declares, and lowering gives the type a `__str__` that returns it: the emitter implements `Display` for
//! every type with a `__str__`, so the type's Rust `Display` writes its `message()`.

use super::super::super::Mutability;
use super::super::super::TypedExpr;
use super::super::super::decl::{FunctionParam, IrFunction, Visibility};
use super::super::super::expr::{IrExprKind, MethodCallArgPolicy, VarAccess, VarRefKind};
use super::super::super::stmt::{IrStmt, IrStmtKind};
use super::super::super::types::IrType;
use super::super::AstLowering;
use incan_frontend::ast;
use incan_frontend::typechecker::ErrorMessageDisplay;
use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::magic_methods::{self, MagicMethodId};

/// The `Error` method whose text a displayed adopter renders.
const ERROR_MESSAGE_METHOD: &str = "message";

impl AstLowering {
    /// Replace one display operand with its `message()` call when the typechecker recorded that it renders that way.
    ///
    /// Inside a trait default method, `{self}` is recorded against the trait's `Self`; the default is expanded into
    /// each adopter, and an adopter the checker listed as having a `Display` of its own (by the same rule it applies
    /// to any displayed value) keeps displaying itself there. Every other operand is returned unchanged.
    pub(in crate::lower) fn display_operand_through_error_message(
        &self,
        operand: TypedExpr,
        operand_span: ast::Span,
    ) -> TypedExpr {
        let Some(display) = self
            .type_info
            .as_ref()
            .and_then(|info| info.error_message_display(operand_span))
            .cloned()
        else {
            return operand;
        };
        // `{self}` in a trait default is decided per adopter: one with a `Display` of its own displays itself.
        if display.receiver_is_trait_self
            && self
                .current_impl_type
                .as_ref()
                .is_some_and(|adopter| display.self_displaying_adopters.contains(adopter))
        {
            return operand;
        }
        self.error_message_call(operand, display)
    }

    /// Whether the typechecker recorded that the declared type `type_name` displays through `message()`.
    pub(in crate::lower) fn type_displays_through_error_message(&self, type_name: &str) -> bool {
        self.type_info
            .as_ref()
            .is_some_and(|info| info.error_message_display_type(type_name).is_some())
    }

    /// Return the `__str__` a declared type that displays through `message()` is given, or `None` for any other type.
    ///
    /// The method returns the `message()` call the typechecker resolved on the type, so the `Display` the emitter
    /// derives from a `__str__` writes exactly what `message()` returns; that `Display` is what lets the type satisfy
    /// a `Display` bound in the generated Rust. The receiver is typed as the declared type, with its own type
    /// parameters, as a displayed value of that type is. The method is private: nothing but that `Display` calls it,
    /// and the type declares no `__str__` of its own for it to shadow.
    pub(in crate::lower) fn error_message_str_method(
        &self,
        type_name: &str,
        type_params: &[ast::TypeParam],
    ) -> Option<IrFunction> {
        let display = self
            .type_info
            .as_ref()
            .and_then(|info| info.error_message_display_type(type_name))
            .cloned()?;
        let self_name = keywords::as_str(KeywordId::SelfKw).to_string();
        let receiver = TypedExpr::new(
            IrExprKind::Var {
                name: self_name.clone(),
                access: VarAccess::Borrow,
                ref_kind: VarRefKind::Value,
            },
            Self::trait_impl_owner_type(type_name, type_params),
        );
        let message = self.error_message_call(receiver, display);
        Some(IrFunction {
            name: magic_methods::as_str(MagicMethodId::Str).to_string(),
            docstring: None,
            params: vec![FunctionParam {
                name: self_name,
                // A method's `self` is typed by its impl, as in every lowered method.
                ty: IrType::Unknown,
                mutability: Mutability::Immutable,
                is_self: true,
                kind: ast::ParamKind::Normal,
                default: None,
            }],
            return_type: IrType::String,
            body: vec![IrStmt::new(IrStmtKind::Return(Some(message)))],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        })
    }

    /// Build the `message()` call on `receiver` the typechecker resolved in `display`.
    ///
    /// The checker resolved that call exactly as a written `value.message()` is resolved, so it is lowered the same
    /// way: the recorded trait dispatch (a trait default the adopter inherits, or the `Error` bound of a type
    /// parameter) becomes the call's dispatch, and the method is spelled through
    /// [`Self::project_method_target_for_identity`] with the recorded identity -- the declaration's own name for
    /// another library's type, the provider's export for a compiled standard-library type, and the recoverable
    /// projection for a type this compilation declares.
    fn error_message_call(&self, receiver: TypedExpr, display: ErrorMessageDisplay) -> TypedExpr {
        let dispatch = display
            .dispatch
            .map(|dispatch| self.lower_resolved_method_dispatch(dispatch, &receiver));
        let (method, dispatch) = self.project_method_target_for_identity(
            display.identity.as_ref(),
            ERROR_MESSAGE_METHOD,
            &receiver,
            dispatch,
        );
        TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(receiver),
                method,
                dispatch,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::String,
        )
    }
}
