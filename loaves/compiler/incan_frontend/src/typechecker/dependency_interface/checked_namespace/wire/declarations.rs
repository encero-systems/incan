//! Native projection of the compiler declarations in the checked editable namespace.
//! Exhaustive patterns make an added field or alternative a compiler error until its Crumb contract is assigned.

use super::super::{CheckedNamespaceInterface, NamespaceCompiledType, NamespaceMemberIdentities};
use super::*;
use crate::api_metadata::*;
use crate::library_manifest::*;
use crate::loaf_index::LoafIdentity;
use crate::ownership_metadata::{AccessMode, OwnershipContract, ResultDependency, SourceSlot, TypeComponent};
use crate::typechecker::ownership_contracts::defaults::DefaultValue;
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::body_ir::LifecycleMetadata;
use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, IncanCallableParam, IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType,
    IncanType, NumericTypeId, RuntimeBuiltinType, ScopeDiscriminant, SemanticSourceTargetKind, SymbolNamespace,
    SymbolOrigin,
};

macro_rules! tuple_model {
    ($model:ident, $id:literal) => {
        impl Project for $model {
            fn project(&self) -> Cell {
                Cell::Record(vec![($id, self.0.project())])
            }

            fn restore(value: Cell) -> Result<Self, String> {
                let Cell::Record(fields) = value else {
                    return Err(concat!("expected namespace ", stringify!($model)).into());
                };
                let [value] = read_fields(fields, [$id])?;
                Ok(Self(Project::restore(value)?))
            }
        }
    };
}

include!("projection.rs");
