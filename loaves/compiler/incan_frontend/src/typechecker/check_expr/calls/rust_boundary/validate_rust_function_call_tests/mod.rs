//! Tests for Rust boundary matching and Rust function and method call validation.

use super::{RustArgBoundaryMatch, TypeChecker};
use crate::ast::{CallArg, Expr, IntLiteral, Literal, Span, Spanned};
use crate::symbols::{
    CallableParam, NewtypeInfo, ResolvedType, RustImportBindingKind, RustItemInfo, ScopeKind, Symbol, SymbolKind,
    TypeInfo, VariableInfo,
};
use crate::typechecker::{RustArgCoercionInfo, RustArgCoercionKind};
use incan_lang::interop::{
    CoercionPolicy, RustAssociatedTypeBinding, RustFunctionSig, RustItemKind, RustItemMetadata, RustParam,
    RustTraitBound, RustTypeInfo, RustVisibility,
};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use incan_lang::lang::types::numerics::{self, NumericTypeId};
use std::collections::HashMap;

mod argument_boundaries;
mod arity_and_callbacks;
mod borrow_shapes;
