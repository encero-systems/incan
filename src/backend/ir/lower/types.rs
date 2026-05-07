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
use super::super::types::{IR_UNION_TYPE_NAME, IrType};
use super::AstLowering;
use super::errors::LoweringError;
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

fn lowered_generic_arg_or_unknown(lowered_params: &[IrType], idx: usize) -> IrType {
    lowered_params.get(idx).cloned().unwrap_or(IrType::Unknown)
}

/// Lower a resolved decimal generic type into its runtime IR representation.
fn decimal_ir_type(name: &str, args: &[ResolvedType]) -> Option<IrType> {
    if numerics::decimal_constructor_from_str(name).is_none() || args.len() != 2 {
        return None;
    }
    let precision = decimal_type_arg_u8(&args[0])?;
    let scale = decimal_type_arg_u8(&args[1])?;
    Some(IrType::Decimal { precision, scale })
}

/// Extract a checked decimal precision or scale argument from a resolved type placeholder.
fn decimal_type_arg_u8(ty: &ResolvedType) -> Option<u8> {
    match ty {
        ResolvedType::TypeVar(value) => value.parse().ok(),
        _ => None,
    }
}

/// Lower a decimal AST annotation into its runtime IR representation.
fn ast_decimal_ir_type(name: &str, params: &[ast::Spanned<ast::Type>]) -> Option<IrType> {
    if numerics::decimal_constructor_from_str(name).is_none() || params.len() != 2 {
        return None;
    }
    let precision = ast_decimal_type_arg_u8(&params[0].node)?;
    let scale = ast_decimal_type_arg_u8(&params[1].node)?;
    Some(IrType::Decimal { precision, scale })
}

/// Extract a decimal precision or scale argument from a type-position integer literal.
fn ast_decimal_type_arg_u8(ty: &ast::Type) -> Option<u8> {
    match ty {
        ast::Type::IntLiteral(value) => value.value.try_into().ok(),
        _ => None,
    }
}

/// Construct the canonical IR shape for an anonymous union.
fn union_ir_type(members: Vec<IrType>) -> IrType {
    let mut has_none = false;
    let mut flattened = Vec::new();

    for member in members {
        match member {
            IrType::Unit => has_none = true,
            IrType::NamedGeneric(name, nested) if name == IR_UNION_TYPE_NAME => flattened.extend(nested),
            other => flattened.push(other),
        }
    }

    flattened.sort_by_key(IrType::rust_name);
    flattened.dedup();

    if has_none {
        return match flattened.as_slice() {
            [] => IrType::Unit,
            [only] => IrType::Option(Box::new(only.clone())),
            _ => IrType::Option(Box::new(IrType::NamedGeneric(
                IR_UNION_TYPE_NAME.to_string(),
                flattened,
            ))),
        };
    }

    match flattened.as_slice() {
        [] => IrType::Unknown,
        [only] => only.clone(),
        _ => IrType::NamedGeneric(IR_UNION_TYPE_NAME.to_string(), flattened),
    }
}

impl AstLowering {
    /// Merge a typechecker-derived IR type with an already-lowered IR type without erasing in-scope generic
    /// placeholders that the typechecker may have normalized to nominal names.
    pub(super) fn merge_inferred_ir_type(existing: &IrType, inferred: IrType) -> IrType {
        match (existing, inferred) {
            (IrType::Generic(existing_name), IrType::Struct(inferred_name)) if existing_name == &inferred_name => {
                existing.clone()
            }
            (IrType::Ref(existing_inner), IrType::Ref(inferred_inner)) => {
                IrType::Ref(Box::new(Self::merge_inferred_ir_type(existing_inner, *inferred_inner)))
            }
            (IrType::Ref(existing_inner), inferred_inner) => {
                IrType::Ref(Box::new(Self::merge_inferred_ir_type(existing_inner, inferred_inner)))
            }
            (IrType::RefMut(existing_inner), IrType::RefMut(inferred_inner)) => {
                IrType::RefMut(Box::new(Self::merge_inferred_ir_type(existing_inner, *inferred_inner)))
            }
            (IrType::RefMut(existing_inner), inferred_inner) => {
                IrType::RefMut(Box::new(Self::merge_inferred_ir_type(existing_inner, inferred_inner)))
            }
            (IrType::List(existing_inner), IrType::List(inferred_inner)) => {
                IrType::List(Box::new(Self::merge_inferred_ir_type(existing_inner, *inferred_inner)))
            }
            (IrType::Set(existing_inner), IrType::Set(inferred_inner)) => {
                IrType::Set(Box::new(Self::merge_inferred_ir_type(existing_inner, *inferred_inner)))
            }
            (IrType::Option(existing_inner), IrType::Option(inferred_inner)) => {
                IrType::Option(Box::new(Self::merge_inferred_ir_type(existing_inner, *inferred_inner)))
            }
            (IrType::Dict(existing_key, existing_value), IrType::Dict(inferred_key, inferred_value)) => IrType::Dict(
                Box::new(Self::merge_inferred_ir_type(existing_key, *inferred_key)),
                Box::new(Self::merge_inferred_ir_type(existing_value, *inferred_value)),
            ),
            (IrType::Result(existing_ok, existing_err), IrType::Result(inferred_ok, inferred_err)) => IrType::Result(
                Box::new(Self::merge_inferred_ir_type(existing_ok, *inferred_ok)),
                Box::new(Self::merge_inferred_ir_type(existing_err, *inferred_err)),
            ),
            (IrType::Tuple(existing_items), IrType::Tuple(inferred_items))
                if existing_items.len() == inferred_items.len() =>
            {
                IrType::Tuple(
                    existing_items
                        .iter()
                        .cloned()
                        .zip(inferred_items)
                        .map(|(existing_item, inferred_item)| {
                            Self::merge_inferred_ir_type(&existing_item, inferred_item)
                        })
                        .collect(),
                )
            }
            (
                IrType::NamedGeneric(existing_name, existing_args),
                IrType::NamedGeneric(inferred_name, inferred_args),
            ) if existing_name == &inferred_name && existing_args.len() == inferred_args.len() => IrType::NamedGeneric(
                inferred_name,
                existing_args
                    .iter()
                    .cloned()
                    .zip(inferred_args)
                    .map(|(existing_arg, inferred_arg)| Self::merge_inferred_ir_type(&existing_arg, inferred_arg))
                    .collect(),
            ),
            (
                IrType::Function {
                    params: existing_params,
                    ret: existing_ret,
                },
                IrType::Function {
                    params: inferred_params,
                    ret: inferred_ret,
                },
            ) if existing_params.len() == inferred_params.len() => IrType::Function {
                params: existing_params
                    .iter()
                    .cloned()
                    .zip(inferred_params)
                    .map(|(existing_param, inferred_param)| {
                        Self::merge_inferred_ir_type(&existing_param, inferred_param)
                    })
                    .collect(),
                ret: Box::new(Self::merge_inferred_ir_type(existing_ret, *inferred_ret)),
            },
            (_, inferred) => inferred,
        }
    }

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
                    return match n {
                        "int" => IrType::Int,
                        "float" => IrType::Float,
                        "bool" => IrType::Bool,
                        _ => match id {
                            NumericTypeId::Bool => IrType::Bool,
                            _ => IrType::Numeric(id),
                        },
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
                if let Some(decimal) = ast_decimal_ir_type(base, params) {
                    return decimal;
                }
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
                    _ if base == IR_UNION_TYPE_NAME => union_ir_type(params_lowered),
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
            ResolvedType::Numeric(id) => IrType::Numeric(*id),
            ResolvedType::Bool => IrType::Bool,
            ResolvedType::Str => IrType::String,
            ResolvedType::Bytes => IrType::Bytes,
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
            ResolvedType::RefMut(inner) => IrType::RefMut(Box::new(self.lower_resolved_type(inner))),
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
                    CollectionTypeId::FrozenList
                    | CollectionTypeId::FrozenSet
                    | CollectionTypeId::FrozenDict
                    | CollectionTypeId::Generator,
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
                GenericBaseKind::Other => {
                    if let Some(decimal) = decimal_ir_type(name, args) {
                        return decimal;
                    }
                    let lowered_args: Vec<IrType> = args.iter().map(|t| self.lower_resolved_type(t)).collect();
                    if name == IR_UNION_TYPE_NAME {
                        return union_ir_type(lowered_args);
                    }
                    if lowered_args.is_empty() {
                        IrType::Struct(name.clone())
                    } else {
                        IrType::NamedGeneric(name.clone(), lowered_args)
                    }
                }
            },
            ResolvedType::Function(params, ret) => IrType::Function {
                params: params.iter().map(|p| self.lower_resolved_type(&p.ty)).collect(),
                ret: Box::new(self.lower_resolved_type(ret)),
            },
            ResolvedType::Tuple(items) => IrType::Tuple(items.iter().map(|t| self.lower_resolved_type(t)).collect()),
            ResolvedType::TypeVar(name) => IrType::Generic(name.clone()),
            ResolvedType::SelfType => IrType::SelfType,
            ResolvedType::RustPath(path) => IrType::Struct(path.clone()),
            ResolvedType::CallSiteInfer => IrType::Unknown,
            ResolvedType::Unknown => IrType::Unknown,
        }
    }

    /// Lower an AST type while preserving names that are in-scope type parameters.
    pub(super) fn lower_type_with_type_params(
        &self,
        ty: &ast::Type,
        type_param_names: Option<&std::collections::HashSet<&str>>,
    ) -> IrType {
        match ty {
            ast::Type::Qualified(segments) => IrType::Struct(segments.join("::")),
            ast::Type::Simple(name) => {
                let n = name.as_str();

                if type_param_names.is_some_and(|params| params.contains(n)) {
                    return IrType::Generic(name.clone());
                }

                if n == conventions::NONE_TYPE_NAME || n == conventions::UNIT_TYPE_NAME {
                    return IrType::Unit;
                }

                if let Some(id) = numerics::from_str(n) {
                    return match n {
                        "int" => IrType::Int,
                        "float" => IrType::Float,
                        "bool" => IrType::Bool,
                        _ => match id {
                            NumericTypeId::Bool => IrType::Bool,
                            _ => IrType::Numeric(id),
                        },
                    };
                }

                if let Some(id) = stringlike::from_str(n) {
                    return match id {
                        StringLikeId::Str | StringLikeId::FString => IrType::String,
                        StringLikeId::Bytes => IrType::Bytes,
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
                if let Some(decimal) = ast_decimal_ir_type(base, params) {
                    return decimal;
                }
                let lowered_params: Vec<_> = params
                    .iter()
                    .map(|p| self.lower_type_with_type_params(&p.node, type_param_names))
                    .collect();
                match classify_generic_base(base.as_str()) {
                    GenericBaseKind::Collection(CollectionTypeId::List) => {
                        IrType::List(Box::new(lowered_generic_arg_or_unknown(&lowered_params, 0)))
                    }
                    GenericBaseKind::Collection(CollectionTypeId::Dict) => IrType::Dict(
                        Box::new(lowered_generic_arg_or_unknown(&lowered_params, 0)),
                        Box::new(lowered_generic_arg_or_unknown(&lowered_params, 1)),
                    ),
                    GenericBaseKind::Collection(CollectionTypeId::Set) => {
                        IrType::Set(Box::new(lowered_generic_arg_or_unknown(&lowered_params, 0)))
                    }
                    GenericBaseKind::Collection(CollectionTypeId::Option) => {
                        IrType::Option(Box::new(lowered_generic_arg_or_unknown(&lowered_params, 0)))
                    }
                    GenericBaseKind::Collection(CollectionTypeId::Result) => IrType::Result(
                        Box::new(lowered_generic_arg_or_unknown(&lowered_params, 0)),
                        Box::new(lowered_generic_arg_or_unknown(&lowered_params, 1)),
                    ),
                    GenericBaseKind::Collection(CollectionTypeId::Tuple) => IrType::Tuple(lowered_params),
                    GenericBaseKind::Collection(
                        CollectionTypeId::FrozenList
                        | CollectionTypeId::FrozenSet
                        | CollectionTypeId::FrozenDict
                        | CollectionTypeId::Generator,
                    ) => {
                        let Some(id) = collections::from_str(base.as_str()) else {
                            return IrType::NamedGeneric(base.clone(), lowered_params);
                        };
                        IrType::NamedGeneric(collections::as_str(id).to_string(), lowered_params)
                    }
                    GenericBaseKind::Other if base == IR_UNION_TYPE_NAME => union_ir_type(lowered_params),
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
            ast::Type::Ref(inner) => IrType::Ref(Box::new(
                self.lower_type_with_type_params(&inner.node, type_param_names),
            )),
            ast::Type::RefMut(inner) => IrType::RefMut(Box::new(
                self.lower_type_with_type_params(&inner.node, type_param_names),
            )),
            ast::Type::Unit => IrType::Unit,
            ast::Type::Tuple(items) => IrType::Tuple(
                items
                    .iter()
                    .map(|t| self.lower_type_with_type_params(&t.node, type_param_names))
                    .collect(),
            ),
            ast::Type::SelfType => IrType::SelfType,
            ast::Type::IntLiteral(_) => IrType::Unknown,
            ast::Type::Infer => IrType::Unknown,
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

    /// Lower a binary operator from AST to IR.
    ///
    /// # Parameters
    ///
    /// * `op` - The AST binary operator
    ///
    /// # Returns
    ///
    /// The corresponding IR binary operator.
    pub(super) fn lower_binop(&self, op: &ast::BinaryOp, span: ast::Span) -> Result<BinOp, LoweringError> {
        let binop = match op {
            ast::BinaryOp::Add => BinOp::Add,
            ast::BinaryOp::Sub => BinOp::Sub,
            ast::BinaryOp::Mul => BinOp::Mul,
            ast::BinaryOp::Div => BinOp::Div,
            ast::BinaryOp::FloorDiv => BinOp::FloorDiv,
            ast::BinaryOp::Mod => BinOp::Mod,
            ast::BinaryOp::Pow => BinOp::Pow,
            ast::BinaryOp::BitAnd => BinOp::BitAnd,
            ast::BinaryOp::BitOr => BinOp::BitOr,
            ast::BinaryOp::BitXor => BinOp::BitXor,
            ast::BinaryOp::Shl => BinOp::Shl,
            ast::BinaryOp::Shr => BinOp::Shr,
            ast::BinaryOp::Eq => BinOp::Eq,
            ast::BinaryOp::NotEq => BinOp::Ne,
            ast::BinaryOp::Lt => BinOp::Lt,
            ast::BinaryOp::LtEq => BinOp::Le,
            ast::BinaryOp::Gt => BinOp::Gt,
            ast::BinaryOp::GtEq => BinOp::Ge,
            ast::BinaryOp::And => BinOp::And,
            ast::BinaryOp::Or => BinOp::Or,
            ast::BinaryOp::In | ast::BinaryOp::NotIn | ast::BinaryOp::Is => BinOp::Eq,
            ast::BinaryOp::IsNot => BinOp::Ne,
            ast::BinaryOp::MatMul | ast::BinaryOp::PipeForward | ast::BinaryOp::PipeBackward => {
                return Err(LoweringError {
                    message: format!("operator `{op}` must resolve to a user-defined operator hook before lowering"),
                    span: span.into(),
                });
            }
        };
        Ok(binop)
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
            | ast::BinaryOp::Is
            | ast::BinaryOp::IsNot => IrType::Bool,
            ast::BinaryOp::BitAnd
            | ast::BinaryOp::BitOr
            | ast::BinaryOp::BitXor
            | ast::BinaryOp::Shl
            | ast::BinaryOp::Shr => {
                if matches!((left, right), (IrType::Int, IrType::Int)) {
                    IrType::Int
                } else {
                    IrType::Unknown
                }
            }
            ast::BinaryOp::MatMul | ast::BinaryOp::PipeForward | ast::BinaryOp::PipeBackward => IrType::Unknown,
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

#[cfg(test)]
mod tests {
    use super::AstLowering;
    use crate::backend::ir::types::IrType;
    use crate::frontend::symbols::ResolvedType;

    #[test]
    fn lower_resolved_type_preserves_named_generic_args_for_nominal_types() {
        let lowering = AstLowering::new();
        let lowered = lowering.lower_resolved_type(&ResolvedType::Generic(
            "Box".to_string(),
            vec![ResolvedType::Named("Node".to_string())],
        ));

        assert_eq!(
            lowered,
            IrType::NamedGeneric("Box".to_string(), vec![IrType::Struct("Node".to_string())])
        );
    }

    #[test]
    fn merge_inferred_ir_type_preserves_existing_generic_placeholders() {
        let merged = AstLowering::merge_inferred_ir_type(
            &IrType::NamedGeneric(
                "Box".to_string(),
                vec![IrType::NamedGeneric(
                    "Node".to_string(),
                    vec![IrType::Generic("T".to_string())],
                )],
            ),
            IrType::NamedGeneric(
                "Box".to_string(),
                vec![IrType::NamedGeneric(
                    "Node".to_string(),
                    vec![IrType::Struct("T".to_string())],
                )],
            ),
        );

        assert_eq!(
            merged,
            IrType::NamedGeneric(
                "Box".to_string(),
                vec![IrType::NamedGeneric(
                    "Node".to_string(),
                    vec![IrType::Generic("T".to_string())]
                )]
            )
        );
    }
}
