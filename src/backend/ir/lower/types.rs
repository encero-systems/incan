//! Type lowering utilities for AST to IR conversion.
//!
//! This module contains helper functions for converting AST types, operators,
//! and performing variable lookups during the lowering pass.
//!
//! Numeric semantics follow Python-like rules (via `incan_core`):
//! - `/` always yields `Float` (even `int / int`)
//! - `%` supports floats with Python remainder semantics
//! - `**` yields `Int` only for non-negative int literal exponents; otherwise `Float`

use super::super::expr::BinOp;
use super::super::types::IrType;
use super::AstLowering;
use crate::frontend::ast;
use crate::frontend::symbols::ResolvedType;
use crate::numeric_adapters::{ir_type_to_numeric_ty, numeric_op_from_ast};
use incan_core::lang::conventions;
use incan_core::lang::types::collections::{self, CollectionTypeId};
use incan_core::lang::types::numerics::{self, NumericTypeId};
use incan_core::lang::types::stringlike::{self, StringLikeId};
use incan_core::{NumericTy, PowExponentKind, result_numeric_type};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenericBaseKind {
    Collection(CollectionTypeId),
    Other,
}

fn classify_generic_base(name: &str) -> GenericBaseKind {
    if let Some(id) = collections::from_str(name) {
        return GenericBaseKind::Collection(id);
    }
    GenericBaseKind::Other
}

impl AstLowering {
    /// Lower an AST type in a `const` context, applying RFC 008 freezing rules.
    ///
    /// Maps container/string annotations to their frozen/static IR equivalents:
    /// - `str` -> `StaticStr`
    /// - `bytes` -> `StaticBytes`
    /// - `List[T]` -> `NamedGeneric(FrozenList, [T])`
    /// - `Dict[K, V]` -> `NamedGeneric(FrozenDict, [K, V])`
    /// - `Set[T]` -> `NamedGeneric(FrozenSet, [T])`
    pub(super) fn lower_const_annotation_type(&self, ty: &ast::Type) -> IrType {
        match ty {
            ast::Type::Simple(name) => {
                let n = name.as_str();

                if n == conventions::NONE_TYPE_NAME || n == conventions::UNIT_TYPE_NAME {
                    return IrType::Unit;
                }

                if let Some(id) = numerics::from_str(n) {
                    return match id {
                        NumericTypeId::Int => IrType::Int,
                        NumericTypeId::Float => IrType::Float,
                        NumericTypeId::Bool => IrType::Bool,
                    };
                }

                if let Some(id) = stringlike::from_str(n) {
                    return match id {
                        // In a const context, strings/bytes map to their `'static` IR equivalents.
                        StringLikeId::Str | StringLikeId::FString => IrType::StaticStr,
                        StringLikeId::Bytes => IrType::StaticBytes,
                        StringLikeId::FrozenStr => IrType::FrozenStr,
                        StringLikeId::FrozenBytes => IrType::FrozenBytes,
                    };
                }

                if let Some(enum_ty) = self.enum_names.get(name) {
                    enum_ty.clone()
                } else {
                    IrType::Struct(name.clone())
                }
            }
            ast::Type::Generic(base, params) => {
                let params_lowered: Vec<_> = params
                    .iter()
                    .map(|p| self.lower_const_annotation_type(&p.node))
                    .collect();
                match classify_generic_base(base.as_str()) {
                    GenericBaseKind::Collection(CollectionTypeId::List) => IrType::NamedGeneric(
                        collections::as_str(CollectionTypeId::FrozenList).to_string(),
                        params_lowered,
                    ),
                    GenericBaseKind::Collection(CollectionTypeId::Dict) => IrType::NamedGeneric(
                        collections::as_str(CollectionTypeId::FrozenDict).to_string(),
                        params_lowered,
                    ),
                    GenericBaseKind::Collection(CollectionTypeId::Set) => IrType::NamedGeneric(
                        collections::as_str(CollectionTypeId::FrozenSet).to_string(),
                        params_lowered,
                    ),
                    GenericBaseKind::Collection(
                        CollectionTypeId::FrozenList | CollectionTypeId::FrozenSet | CollectionTypeId::FrozenDict,
                    ) => {
                        let Some(id) = collections::from_str(base.as_str()) else {
                            // Should not happen: `classify_generic_base()` told us this is a collection type.
                            // Fall back to preserving the user spelling to avoid panicking during lowering.
                            return IrType::NamedGeneric(base.clone(), params_lowered);
                        };
                        IrType::NamedGeneric(collections::as_str(id).to_string(), params_lowered)
                    }
                    _ => IrType::NamedGeneric(base.clone(), params.iter().map(|p| self.lower_type(&p.node)).collect()),
                }
            }
            // Delegate function/tuple/unit/self handling to regular lowering
            other => self.lower_type(other),
        }
    }
    /// Convert a frontend `ResolvedType` to an IR type.
    ///
    /// This is used when lowering is driven by the typechecker output rather than AST heuristics.
    #[allow(clippy::only_used_in_recursion)]
    pub(super) fn lower_resolved_type(&self, ty: &ResolvedType) -> IrType {
        match ty {
            ResolvedType::Int => IrType::Int,
            ResolvedType::Float => IrType::Float,
            ResolvedType::Bool => IrType::Bool,
            ResolvedType::Str => IrType::String,
            ResolvedType::Bytes => IrType::Unknown,
            ResolvedType::FrozenStr => IrType::FrozenStr,
            ResolvedType::FrozenBytes => IrType::FrozenBytes,
            ResolvedType::FrozenList(elem) => IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::FrozenList).to_string(),
                vec![self.lower_resolved_type(elem)],
            ),
            ResolvedType::FrozenSet(elem) => IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::FrozenSet).to_string(),
                vec![self.lower_resolved_type(elem)],
            ),
            ResolvedType::FrozenDict(k, v) => IrType::NamedGeneric(
                collections::as_str(CollectionTypeId::FrozenDict).to_string(),
                vec![self.lower_resolved_type(k), self.lower_resolved_type(v)],
            ),
            ResolvedType::Unit => IrType::Unit,
            ResolvedType::Named(name) => IrType::Struct(name.clone()),
            ResolvedType::Ref(inner) => IrType::Ref(Box::new(self.lower_resolved_type(inner))),
            ResolvedType::Generic(name, args) => match classify_generic_base(name.as_str()) {
                GenericBaseKind::Collection(CollectionTypeId::List) => IrType::List(Box::new(
                    args.first()
                        .map(|t| self.lower_resolved_type(t))
                        .unwrap_or(IrType::Unknown),
                )),
                GenericBaseKind::Collection(CollectionTypeId::Dict) => IrType::Dict(
                    Box::new(
                        args.first()
                            .map(|t| self.lower_resolved_type(t))
                            .unwrap_or(IrType::Unknown),
                    ),
                    Box::new(
                        args.get(1)
                            .map(|t| self.lower_resolved_type(t))
                            .unwrap_or(IrType::Unknown),
                    ),
                ),
                GenericBaseKind::Collection(CollectionTypeId::Set) => IrType::Set(Box::new(
                    args.first()
                        .map(|t| self.lower_resolved_type(t))
                        .unwrap_or(IrType::Unknown),
                )),
                GenericBaseKind::Collection(CollectionTypeId::Option) => IrType::Option(Box::new(
                    args.first()
                        .map(|t| self.lower_resolved_type(t))
                        .unwrap_or(IrType::Unknown),
                )),
                GenericBaseKind::Collection(CollectionTypeId::Result) => IrType::Result(
                    Box::new(
                        args.first()
                            .map(|t| self.lower_resolved_type(t))
                            .unwrap_or(IrType::Unknown),
                    ),
                    Box::new(
                        args.get(1)
                            .map(|t| self.lower_resolved_type(t))
                            .unwrap_or(IrType::Unknown),
                    ),
                ),
                GenericBaseKind::Collection(CollectionTypeId::Tuple) => {
                    IrType::Tuple(args.iter().map(|t| self.lower_resolved_type(t)).collect())
                }
                GenericBaseKind::Collection(
                    CollectionTypeId::FrozenList | CollectionTypeId::FrozenSet | CollectionTypeId::FrozenDict,
                ) => {
                    // Normalize to canonical spelling from incan_core.
                    let Some(id) = collections::from_str(name.as_str()) else {
                        // Should not happen: `classify_generic_base()` told us this is a collection type.
                        // Preserve the type name rather than panicking during lowering.
                        return IrType::NamedGeneric(
                            name.clone(),
                            args.iter().map(|t| self.lower_resolved_type(t)).collect(),
                        );
                    };
                    IrType::NamedGeneric(
                        collections::as_str(id).to_string(),
                        args.iter().map(|t| self.lower_resolved_type(t)).collect(),
                    )
                }
                GenericBaseKind::Other => IrType::Struct(name.clone()),
            },
            ResolvedType::Function(params, ret) => IrType::Function {
                params: params.iter().map(|p| self.lower_resolved_type(p)).collect(),
                ret: Box::new(self.lower_resolved_type(ret)),
            },
            ResolvedType::Tuple(items) => IrType::Tuple(items.iter().map(|t| self.lower_resolved_type(t)).collect()),
            ResolvedType::TypeVar(name) => IrType::Generic(name.clone()),
            ResolvedType::SelfType => IrType::SelfType,
            ResolvedType::Unknown => IrType::Unknown,
        }
    }

    /// Lower an AST type to an IR type.
    ///
    /// # Parameters
    ///
    /// * `ty` - The AST type to lower
    ///
    /// # Returns
    ///
    /// The corresponding IR type representation.
    pub(super) fn lower_type(&self, ty: &ast::Type) -> IrType {
        self.lower_type_with_type_params(ty, None)
    }

    pub(super) fn lower_type_with_type_params(
        &self,
        ty: &ast::Type,
        type_param_names: Option<&std::collections::HashSet<&str>>,
    ) -> IrType {
        match ty {
            ast::Type::Simple(name) => {
                let n = name.as_str();

                if type_param_names.is_some_and(|params| params.contains(n)) {
                    return IrType::Generic(name.clone());
                }

                if n == conventions::NONE_TYPE_NAME || n == conventions::UNIT_TYPE_NAME {
                    return IrType::Unit;
                }

                if let Some(id) = numerics::from_str(n) {
                    return match id {
                        NumericTypeId::Int => IrType::Int,
                        NumericTypeId::Float => IrType::Float,
                        NumericTypeId::Bool => IrType::Bool,
                    };
                }

                if let Some(id) = stringlike::from_str(n) {
                    return match id {
                        StringLikeId::Str | StringLikeId::FString => IrType::String,
                        // NOTE: runtime `bytes` is not yet a dedicated IR type; keep it as unknown for now.
                        StringLikeId::Bytes => IrType::Unknown,
                        StringLikeId::FrozenStr => IrType::FrozenStr,
                        StringLikeId::FrozenBytes => IrType::FrozenBytes,
                    };
                }

                // Check if this is a known enum
                if let Some(enum_ty) = self.enum_names.get(name) {
                    enum_ty.clone()
                } else {
                    // Default to struct
                    IrType::Struct(name.clone())
                }
            }
            ast::Type::Generic(base, params) => {
                let lowered_params: Vec<_> = params
                    .iter()
                    .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                    .collect();
                match classify_generic_base(base.as_str()) {
                    GenericBaseKind::Collection(CollectionTypeId::List) => IrType::List(Box::new(
                        params
                            .first()
                            .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                            .unwrap_or(IrType::Unknown),
                    )),
                    GenericBaseKind::Collection(CollectionTypeId::Dict) => IrType::Dict(
                        Box::new(
                            params
                                .first()
                                .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                                .unwrap_or(IrType::Unknown),
                        ),
                        Box::new(
                            params
                                .get(1)
                                .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                                .unwrap_or(IrType::Unknown),
                        ),
                    ),
                    GenericBaseKind::Collection(CollectionTypeId::Set) => IrType::Set(Box::new(
                        params
                            .first()
                            .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                            .unwrap_or(IrType::Unknown),
                    )),
                    GenericBaseKind::Collection(CollectionTypeId::Option) => IrType::Option(Box::new(
                        params
                            .first()
                            .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                            .unwrap_or(IrType::Unknown),
                    )),
                    GenericBaseKind::Collection(CollectionTypeId::Result) => IrType::Result(
                        Box::new(
                            params
                                .first()
                                .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                                .unwrap_or(IrType::Unknown),
                        ),
                        Box::new(
                            params
                                .get(1)
                                .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                                .unwrap_or(IrType::Unknown),
                        ),
                    ),
                    GenericBaseKind::Collection(CollectionTypeId::Tuple) => IrType::Tuple(lowered_params),
                    GenericBaseKind::Collection(
                        CollectionTypeId::FrozenList | CollectionTypeId::FrozenSet | CollectionTypeId::FrozenDict,
                    ) => {
                        let Some(id) = collections::from_str(base.as_str()) else {
                            // Should not happen: `classify_generic_base()` told us this is a collection type.
                            // Fall back to preserving the user spelling to avoid panicking during lowering.
                            return IrType::NamedGeneric(base.clone(), lowered_params);
                        };
                        IrType::NamedGeneric(collections::as_str(id).to_string(), lowered_params)
                    }
                    GenericBaseKind::Other => IrType::NamedGeneric(base.clone(), lowered_params),
                }
            }
            ast::Type::Function(params, ret) => IrType::Function {
                params: params
                    .iter()
                    .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                    .collect(),
                ret: Box::new(self.lower_type_with_type_params(&ret.node, type_param_names)),
            },
            ast::Type::Unit => IrType::Unit,
            ast::Type::Tuple(items) => IrType::Tuple(
                items
                    .iter()
                    .map(|t| self.lower_type_with_type_params(&t.node, type_param_names))
                    .collect(),
            ),
            ast::Type::SelfType => IrType::SelfType,
        }
    }

    /// Lower a binary operator from AST to IR.
    ///
    /// # Parameters
    ///
    /// * `op` - The AST binary operator
    ///
    /// # Returns
    ///
    /// The corresponding IR binary operator.
    pub(super) fn lower_binop(&self, op: &ast::BinaryOp) -> BinOp {
        match op {
            ast::BinaryOp::Add => BinOp::Add,
            ast::BinaryOp::Sub => BinOp::Sub,
            ast::BinaryOp::Mul => BinOp::Mul,
            ast::BinaryOp::Div => BinOp::Div,
            ast::BinaryOp::FloorDiv => BinOp::FloorDiv,
            ast::BinaryOp::Mod => BinOp::Mod,
            ast::BinaryOp::Pow => BinOp::Pow,
            ast::BinaryOp::Eq => BinOp::Eq,
            ast::BinaryOp::NotEq => BinOp::Ne,
            ast::BinaryOp::Lt => BinOp::Lt,
            ast::BinaryOp::LtEq => BinOp::Le,
            ast::BinaryOp::Gt => BinOp::Gt,
            ast::BinaryOp::GtEq => BinOp::Ge,
            ast::BinaryOp::And => BinOp::And,
            ast::BinaryOp::Or => BinOp::Or,
            ast::BinaryOp::In | ast::BinaryOp::NotIn | ast::BinaryOp::Is => BinOp::Eq,
        }
    }

    /// Determine the result type of a binary operation using Python-like numeric semantics.
    ///
    /// ## Parameters
    ///
    /// - `left`: The type of the left operand
    /// - `right`: The type of the right operand
    /// - `op`: The binary operator
    /// - `pow_exp_kind`: For `Pow` operations, describes whether the exponent is a non-negative int literal (yields
    ///   `Int`) or something else (yields `Float`)
    ///
    /// ## Returns
    ///
    /// The result type of the operation.
    pub(super) fn binary_result_type(
        &self,
        left: &IrType,
        right: &IrType,
        op: &ast::BinaryOp,
        pow_exp_kind: Option<PowExponentKind>,
    ) -> IrType {
        match op {
            ast::BinaryOp::Eq
            | ast::BinaryOp::NotEq
            | ast::BinaryOp::Lt
            | ast::BinaryOp::LtEq
            | ast::BinaryOp::Gt
            | ast::BinaryOp::GtEq
            | ast::BinaryOp::And
            | ast::BinaryOp::Or
            | ast::BinaryOp::In
            | ast::BinaryOp::NotIn
            | ast::BinaryOp::Is => IrType::Bool,
            ast::BinaryOp::Add
            | ast::BinaryOp::Sub
            | ast::BinaryOp::Mul
            | ast::BinaryOp::Div
            | ast::BinaryOp::FloorDiv
            | ast::BinaryOp::Mod
            | ast::BinaryOp::Pow => {
                // Convert to NumericTy
                let lhs_num = ir_type_to_numeric_ty(left);
                let rhs_num = ir_type_to_numeric_ty(right);

                match (lhs_num, rhs_num) {
                    (Some(lhs), Some(rhs)) => {
                        if let Some(num_op) = numeric_op_from_ast(op) {
                            let result = result_numeric_type(num_op, lhs, rhs, pow_exp_kind);
                            match result {
                                NumericTy::Int => IrType::Int,
                                NumericTy::Float => IrType::Float,
                            }
                        } else {
                            IrType::Unknown
                        }
                    }
                    _ => left.clone(),
                }
            }
        }
    }

    /// Look up a variable type in the current scope chain.
    ///
    /// Searches from innermost to outermost scope.
    ///
    /// # Parameters
    ///
    /// * `name` - The variable name to look up
    ///
    /// # Returns
    ///
    /// The type of the variable, or `IrType::Unknown` if not found.
    pub(super) fn lookup_var(&self, name: &str) -> IrType {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return ty.clone();
            }
        }
        IrType::Unknown
    }
}
