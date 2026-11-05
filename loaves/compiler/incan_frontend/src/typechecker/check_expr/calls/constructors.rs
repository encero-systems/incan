//! Constructor-style call validation for models, classes, enums, and surface constructors.

use super::{TypeChecker, generic_bounds};
use crate::ast::{CallArg, Expr, Literal, ParamKind, Span, Spanned, Type};
use crate::diagnostics::errors::{self, TypeArgumentOrigin};
use crate::resolved_type_subst::{substitute_resolved_type, type_param_subst_map_call_site};
use crate::symbols::{CallableParam, FieldInfo, ResolvedType, SymbolKind, TypeInfo, ValueEnumInfo};
use crate::typechecker::helpers::option_ty;
use crate::typechecker::type_info::ConstructorFieldBinding;
use incan_lang::lang::conventions::TYPE_CONSTRUCTOR_HOOK;
use incan_lang::lang::surface::types::{self as surface_types, SurfaceTypeId};

mod class_constructors;
mod value_constructors;

#[cfg(test)]
mod selected_module_binding_tests;
