//! Rust `Eq`, `PartialOrd` and `Ord` for a type that adopts `std.derives.comparison.Eq` or `Ord` (#1561).
//!
//! Adopting `Eq` and defining `__eq__` provides `Eq`, and adopting `Ord` and defining `__eq__` and `__lt__` provides
//! `Ord` (comparison reference). The backend implements `PartialEq` from `__eq__`; what a set element, a dict key,
//! `sorted(values)`, `<` and the `T with Eq` and `T with Ord` bounds need beside it are Rust's `Eq`, `PartialOrd` and
//! `Ord`, which a derive would give the type and this lowers from the adoption instead:
//!
//! - `Eq` has no method, so its impl is empty.
//! - `Ord::cmp` orders by the type's own `Ord` impl: `a` is less than `b` when `a.__lt__(b)`, greater when
//!   `b.__lt__(a)`, and equal otherwise.
//! - `PartialOrd` is `Some` of that order, and its `<`, `<=`, `>` and `>=` call `__lt__`, `__le__`, `__gt__` and
//!   `__ge__` of the `Ord` impl, so a type that defines one of them compares with its own.

use super::super::super::decl::{FunctionParam, IrFunction, IrImpl, Visibility};
use super::super::super::expr::{
    IrCallArg, IrCallArgKind, IrExprKind, IrMethodDispatch, IrTraitDispatch, MethodCallArgPolicy, UnaryOp, VarAccess,
    VarRefKind,
};
use super::super::super::stmt::{IrStmt, IrStmtKind};
use super::super::super::types::IrType;
use super::super::super::{IrSpan, Mutability, TypedExpr};
use super::super::AstLowering;
use incan_frontend::ast;
use incan_lang::lang::derives::{self, DeriveId};
use incan_lang::lang::keywords::{self, KeywordId};
use incan_lang::lang::surface::constructors::{self, ConstructorId};
use incan_lang::lang::traits::{self as core_traits, TraitId};

/// Rust's path for the ordering `Ord::cmp` returns.
const RUST_ORDERING: &str = "std::cmp::Ordering";
/// Rust's `Ord` trait, spelled by path so the impl reaches it whatever the module binds `Ord` to.
const RUST_ORD_PATH: &str = "std::cmp::Ord";
/// Rust's `PartialOrd` trait, spelled by path.
const RUST_PARTIAL_ORD_PATH: &str = "std::cmp::PartialOrd";
/// Rust's `Eq` trait, spelled by path.
const RUST_EQ_PATH: &str = "std::cmp::Eq";

/// The source comparison traits one declaration adopts.
struct ComparisonAdoption {
    /// The generated path of the adopted `std.derives.comparison.Ord`, when the type adopts it.
    ord_path: Option<String>,
}

impl AstLowering {
    /// Return the Rust `Eq`, `PartialOrd` and `Ord` impls of `type_name` for its adoption of the stdlib source
    /// comparison traits among `impl_targets`, or none when it adopts neither or already derives the capability.
    pub(in crate::lower) fn lower_comparison_capability_impls(
        &self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        impl_targets: &[(String, Vec<IrType>)],
        derived: &[String],
    ) -> Vec<IrImpl> {
        let Some(adoption) = self.comparison_adoption(impl_targets) else {
            return Vec::new();
        };
        let derives_capability = derived.iter().any(|name| {
            matches!(
                derives::from_str(name),
                Some(DeriveId::Eq | DeriveId::Ord | DeriveId::PartialOrd)
            )
        });
        if derives_capability {
            return Vec::new();
        }
        let owner = Self::trait_impl_owner_type(type_name, type_params);
        let mut impls = vec![self.comparison_capability_impl(type_name, type_params, RUST_EQ_PATH, Vec::new())];
        if let Some(ord_path) = adoption.ord_path.as_deref() {
            impls.push(self.comparison_capability_impl(
                type_name,
                type_params,
                RUST_PARTIAL_ORD_PATH,
                Self::partial_ord_methods(ord_path, &owner),
            ));
            impls.push(self.comparison_capability_impl(
                type_name,
                type_params,
                RUST_ORD_PATH,
                vec![Self::ord_cmp_method(ord_path, &owner)],
            ));
        }
        impls
    }

    /// Return which stdlib source comparison traits `impl_targets` adopt, or `None` for neither.
    fn comparison_adoption(&self, impl_targets: &[(String, Vec<IrType>)]) -> Option<ComparisonAdoption> {
        let mut adopts_eq = false;
        let mut ord_path = None;
        for (trait_name, _) in impl_targets {
            let Some(path) = self.source_owned_builtin_trait_path(trait_name) else {
                continue;
            };
            let (_, source_name) = self.canonical_trait_identity(trait_name);
            match source_name.as_deref().and_then(core_traits::from_str) {
                Some(TraitId::Eq) => adopts_eq = true,
                Some(TraitId::Ord) => ord_path = Some(path),
                _ => {}
            }
        }
        (adopts_eq || ord_path.is_some()).then_some(ComparisonAdoption { ord_path })
    }

    /// Build one impl of the Rust trait at `trait_path` for the declared type, with the declaration's type parameters.
    fn comparison_capability_impl(
        &self,
        type_name: &str,
        type_params: &[ast::TypeParam],
        trait_path: &str,
        methods: Vec<IrFunction>,
    ) -> IrImpl {
        IrImpl {
            target_type: type_name.to_string(),
            type_params: self.lower_type_params(type_params),
            trait_name: Some(trait_path.to_string()),
            trait_module_path: None,
            trait_source_name: None,
            trait_type_args: Vec::new(),
            associated_types: Vec::new(),
            methods,
            method_projections: Vec::new(),
            source_method_projections: Vec::new(),
        }
    }

    /// Return `partial_cmp`, `lt`, `le`, `gt` and `ge` of the `PartialOrd` impl.
    fn partial_ord_methods(ord_path: &str, owner: &IrType) -> Vec<IrFunction> {
        let ordering = IrType::Option(Box::new(IrType::RustDisplay(RUST_ORDERING.to_string())));
        let cmp = Self::trait_call(
            RUST_ORD_PATH,
            "cmp",
            Self::borrowed_param("self", owner),
            Self::borrowed_param("other", owner),
            IrType::RustDisplay(RUST_ORDERING.to_string()),
        );
        let some = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: constructors::as_str(ConstructorId::Some).to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::ExternalRustName,
                    },
                    IrType::Unknown,
                )),
                type_args: Vec::new(),
                args: vec![Self::positional(cmp)],
                callable_signature: None,
                canonical_path: None,
            },
            ordering.clone(),
        );
        let mut methods = vec![Self::comparison_method("partial_cmp", ordering, some)];
        for (rust_method, dunder) in [("lt", "__lt__"), ("le", "__le__"), ("gt", "__gt__"), ("ge", "__ge__")] {
            let body = Self::source_ord_call(ord_path, dunder, "self", "other", owner);
            methods.push(Self::comparison_method(rust_method, IrType::Bool, body));
        }
        methods
    }

    /// Return `cmp` of the `Ord` impl: `other.__lt__(self)` compared with `self.__lt__(other)` as booleans, which is
    /// `Less` when only `self.__lt__(other)` holds, `Greater` when only `other.__lt__(self)` does, and `Equal` when
    /// neither does.
    fn ord_cmp_method(ord_path: &str, owner: &IrType) -> IrFunction {
        let other_before = Self::source_ord_call(ord_path, "__lt__", "other", "self", owner);
        let self_before = Self::source_ord_call(ord_path, "__lt__", "self", "other", owner);
        let self_before = TypedExpr::new(
            IrExprKind::UnaryOp {
                op: UnaryOp::Ref,
                operand: Box::new(self_before),
            },
            IrType::Ref(Box::new(IrType::Bool)),
        );
        let ordering = IrType::RustDisplay(RUST_ORDERING.to_string());
        let body = Self::trait_call(RUST_ORD_PATH, "cmp", other_before, self_before, ordering.clone());
        Self::comparison_method("cmp", ordering, body)
    }

    /// Call the dunder `method` of the adopted source `Ord` on the parameter `receiver`, passing a copy of the
    /// parameter `argument`, whose slot takes its operand by value.
    fn source_ord_call(ord_path: &str, method: &str, receiver: &str, argument: &str, owner: &IrType) -> TypedExpr {
        let argument = Self::borrowed_param(argument, owner);
        let copied = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(argument),
                method: "clone".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: Vec::new(),
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            owner.clone(),
        );
        Self::trait_call(
            ord_path,
            method,
            Self::borrowed_param(receiver, owner),
            copied,
            IrType::Bool,
        )
    }

    /// Call `trait_path`'s `method` on `receiver` with one argument, by the trait's path.
    fn trait_call(trait_path: &str, method: &str, receiver: TypedExpr, argument: TypedExpr, ty: IrType) -> TypedExpr {
        let trait_name = trait_path.rsplit("::").next().unwrap_or(trait_path);
        TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(receiver),
                method: method.to_string(),
                dispatch: Some(IrMethodDispatch::Trait(Box::new(IrTraitDispatch {
                    trait_source_name: trait_name.to_string(),
                    trait_module_path: None,
                    implementation_type_params: Vec::new(),
                    trait_path: trait_path.to_string(),
                    type_args: Vec::new(),
                    receiver_is_mutable: false,
                }))),
                type_args: Vec::new(),
                args: vec![Self::positional(argument)],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::PreserveShape,
            },
            ty,
        )
    }

    /// Read the method parameter `name`, which a comparison method takes by reference to the owner type.
    fn borrowed_param(name: &str, owner: &IrType) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Var {
                name: name.to_string(),
                access: VarAccess::Borrow,
                ref_kind: VarRefKind::Value,
            },
            IrType::Ref(Box::new(owner.clone())),
        )
    }

    /// Wrap an expression as a positional call argument.
    fn positional(expr: TypedExpr) -> IrCallArg {
        IrCallArg {
            name: None,
            kind: IrCallArgKind::Positional,
            expr,
        }
    }

    /// Build a comparison method `name(&self, other: &Self) -> return_type` that returns `value`.
    fn comparison_method(name: &str, return_type: IrType, value: TypedExpr) -> IrFunction {
        let self_param = FunctionParam {
            name: keywords::as_str(KeywordId::SelfKw).to_string(),
            ty: IrType::SelfType,
            mutability: Mutability::Immutable,
            is_self: true,
            kind: ast::ParamKind::Normal,
            default: None,
        };
        let other_param = FunctionParam {
            name: "other".to_string(),
            ty: IrType::Ref(Box::new(IrType::SelfType)),
            mutability: Mutability::Immutable,
            is_self: false,
            kind: ast::ParamKind::Normal,
            default: None,
        };
        IrFunction {
            name: name.to_string(),
            docstring: None,
            params: vec![self_param, other_param],
            return_type,
            body: vec![IrStmt {
                kind: IrStmtKind::Return(Some(value)),
                span: IrSpan::default(),
            }],
            is_async: false,
            is_generator: false,
            visibility: Visibility::Private,
            type_params: Vec::new(),
            is_extern: false,
            rust_extern_name: None,
            rust_attributes: Vec::new(),
            lint_allows: Vec::new(),
        }
    }
}
