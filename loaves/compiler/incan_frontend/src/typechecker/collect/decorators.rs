//! Decorator resolution and validation helpers for the first pass.
//!
//! This keeps decorator path resolution and validation logic out of the main collection flow while preserving RFC 022
//! semantics.

use std::collections::{HashMap, HashSet};

use crate::api_metadata::{ApiDeclaration, ApiFunction};
use crate::ast::*;
use crate::decorator_resolution;
use crate::diagnostics::{CompileError, errors};
use crate::symbols::{ResolvedType, SymbolKind, SymbolTable, TypeInfo};
use crate::typechecker::derive_requirements::LocalDeriveFacts;
use crate::typechecker::type_info::{
    CBindingBuffer, CBindingDescriptor, CBindingEnum, CBindingEnumVariant, CBindingOutcome, CBindingParameter,
    CBindingResource, CBindingStruct, CBindingStructField, CBindingSymbol, CBindingType, COutputMode, CResourceAccess,
};
use crate::typechecker::{ConstValue, TypeChecker};
use incan_lang::lang::c_abi::{
    self, BindingArgumentId, BindingMemberId, LinkCapabilityId, ResourceArgumentId, ResourceTypeConstructorId,
    SymbolArgumentId, SymbolOutcomeArgumentId,
};
use incan_lang::lang::decorators::{self, DecoratorId};
use incan_lang::lang::derives;
use incan_semantics_core::body_ir::{NativeAttribute, NativeAttributeArgument, NativeAttributeValue};
use incan_semantics_core::{
    CanonicalSymbolId, DecoratorFeature, HirSourceSpan, SemanticSourceTargetKind, SurfaceFeatureKey, SymbolNamespace,
    SymbolOrigin,
};

mod c_binding_classes;
mod c_binding_symbols;
mod c_binding_types;
mod decorator_validation;
mod derive_validation;

#[derive(Clone, Copy)]
enum DecoratorValidationTarget {
    AllowsUserDefined,
    RejectsUserDefined(&'static str),
}

/// Parsed outcome data before it is checked against one C symbol's result and output contracts.
struct RawCBindingOutcome {
    result: String,
    initializes: Vec<String>,
    updates: Vec<String>,
    invalidates: Vec<String>,
}

/// Raw data parsed from one C symbol before it becomes a checked binding descriptor.
type RawCBindingSymbolData = (String, Vec<(String, String)>, Vec<RawCBindingOutcome>);

/// Parsed C symbol data that must wait for the binding's complete enum namespace before outcome validation.
struct RawCBindingSymbol {
    span: Span,
    name: String,
    native: String,
    parameters: Vec<CBindingParameter>,
    return_type: CBindingType,
    bounds: Vec<(String, String)>,
    outcomes: Vec<RawCBindingOutcome>,
}

/// Fully checked members retained in one C binding descriptor.
struct CBindingMembers {
    resources: Vec<CBindingResource>,
    symbols: Vec<CBindingSymbol>,
    enums: Vec<CBindingEnum>,
    structs: Vec<CBindingStruct>,
}

/// Why a written decorator argument cannot become the literal value an attribute macro receives.
enum NativeAttributeRefusal {
    /// An expression form an attribute macro does not receive.
    Expression,
    /// A name bound to a value without a scalar constant form, such as a list constant.
    NonScalarBinding(String),
}

impl NativeAttributeRefusal {
    /// The diagnostic for this refusal at one attribute macro's decorator.
    fn error(&self, macro_path: &str, span: Span) -> CompileError {
        match self {
            Self::Expression => errors::native_attribute_argument_unsupported(macro_path, span),
            Self::NonScalarBinding(name) => errors::native_attribute_argument_not_scalar(macro_path, name, span),
        }
    }
}

/// Whether a value of this type has a scalar literal form an attribute macro can receive.
fn is_scalar_attribute_type(ty: &ResolvedType) -> bool {
    matches!(
        ty,
        ResolvedType::Str
            | ResolvedType::FrozenStr
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Bool
            | ResolvedType::Numeric(_)
    )
}

/// Resolve a decorator path to a module path.
pub(in crate::typechecker) fn resolve_decorator_path(dec: &Decorator, symbols: &SymbolTable) -> Vec<String> {
    decorator_resolution::resolve_decorator_path(dec, symbols)
}

/// Resolve a decorator path to a decorator id.
pub(in crate::typechecker) fn resolve_decorator_id(dec: &Decorator, symbols: &SymbolTable) -> Option<DecoratorId> {
    let resolved = resolve_decorator_path(dec, symbols);
    decorators::from_segments(&resolved)
}

/// Find decorators by name.
pub(in crate::typechecker) fn decorators_named<'a>(
    decorators: &'a [Spanned<Decorator>],
    symbols: &SymbolTable,
    id: DecoratorId,
) -> impl Iterator<Item = &'a Spanned<Decorator>> {
    decorators
        .iter()
        .filter(move |d| resolve_decorator_id(&d.node, symbols) == Some(id))
}

/// Extract the derive names positional decorator arguments spell, bare or module-qualified
/// ([`decorator_resolution::derive_argument_name`]).
pub(in crate::typechecker) fn positional_derive_names(
    args: &[DecoratorArg],
) -> impl Iterator<Item = (String, Span)> + '_ {
    args.iter().filter_map(|arg| match arg {
        DecoratorArg::Positional(expr) => {
            decorator_resolution::derive_argument_name(&expr.node).map(|name| (name, expr.span))
        }
        _ => None,
    })
}
