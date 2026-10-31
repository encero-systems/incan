//! One checked ownership contract shared by declarations, provider metadata and diagnostic tooling.
//!
//! A contract describes requested access and retained source dependencies. It is not proof that an executable body
//! obeys those requests; admitting execution remains a separate checker capability.

use serde::{Deserialize, Serialize};

/// A callable source slot; source parameter spelling is deliberately not part of the relationship.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "index", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceSlot {
    /// Index in the declaration's source parameter order, excluding its receiver.
    Parameter(usize),
    /// The declared receiver, independently of its local spelling.
    Receiver,
}

/// A type component in a parameter or result. Child indices follow generic/tuple argument order; callable results
/// follow their parameter children. The empty path selects the complete parameter or result type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeComponent {
    /// None denotes the result; Some denotes the source parameter slot.
    pub parameter: Option<usize>,
    /// Structural child path under the selected source type.
    pub children: Vec<usize>,
}

/// An explicit mode attached to a checked component, with its underlying semantic type preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessMode<T> {
    /// Canonical prelude mode, never inferred from a user declaration's display name.
    pub mode: incan_lang::lang::types::ownership::OwnershipMode,
    /// Component whose written type explicitly requests the mode.
    pub component: TypeComponent,
    /// Resolved inner type, before any native representation is chosen.
    pub underlying: T,
}

/// One retained source relationship, resolved to a slot and canonical stored-field identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultDependency<I> {
    /// Source parameter or receiver selected by checking.
    pub slot: SourceSlot,
    /// Ordered stored-field declarations. Computed properties and opaque projections cannot stand in for fields.
    pub fields: Vec<I>,
}

/// Versioned explicit contract. Absence of this structure means no explicit request; it does not assert ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipContract<T, I> {
    /// Schema discriminator; readers must refuse unsupported or missing versions.
    pub schema_version: u32,
    /// Explicit modes in parameter order followed by result traversal order.
    pub modes: Vec<AccessMode<T>>,
    /// An owned aggregate may retain a dependency without a directly borrowed result component.
    pub result_source: Option<ResultDependency<I>>,
}

impl<T, I> OwnershipContract<T, I> {
    /// Project types and identities through their owning boundaries without changing slots or access modes.
    pub fn try_map<U, J, E>(
        &self,
        mut ty: impl FnMut(&T) -> Result<U, E>,
        mut identity: impl FnMut(&I) -> Result<J, E>,
    ) -> Result<OwnershipContract<U, J>, E> {
        Ok(OwnershipContract {
            schema_version: self.schema_version,
            modes: self
                .modes
                .iter()
                .map(|mode| {
                    Ok(AccessMode {
                        mode: mode.mode,
                        component: mode.component.clone(),
                        underlying: ty(&mode.underlying)?,
                    })
                })
                .collect::<Result<_, E>>()?,
            result_source: self
                .result_source
                .as_ref()
                .map(|source| {
                    Ok(ResultDependency {
                        slot: source.slot.clone(),
                        fields: source.fields.iter().map(&mut identity).collect::<Result<_, E>>()?,
                    })
                })
                .transpose()?,
        })
    }

    /// Validate relationships independently of parameter names and reject incomplete metadata.
    pub fn validate(&self, parameter_count: usize, has_receiver: bool) -> Result<(), &'static str> {
        if self.schema_version != 1 {
            return Err("unsupported ownership contract schema");
        }
        if self.modes.is_empty() && self.result_source.is_none() {
            return Err("empty explicit ownership contract");
        }
        let mut components = std::collections::BTreeSet::new();
        for mode in &self.modes {
            if mode.component.parameter.is_some_and(|slot| slot >= parameter_count) {
                return Err("ownership parameter slot is outside the signature");
            }
            if !components.insert((mode.component.parameter, &mode.component.children)) {
                return Err("duplicate ownership type component");
            }
        }
        if self.modes.iter().any(|mode| {
            mode.component.parameter.is_none() && mode.mode != incan_lang::lang::types::ownership::OwnershipMode::Owned
        }) && self.result_source.is_none()
        {
            return Err("borrowed result has no source dependency");
        }
        if let Some(source) = &self.result_source {
            match source.slot {
                SourceSlot::Parameter(slot) if slot >= parameter_count => {
                    return Err("result source slot is outside the signature");
                }
                SourceSlot::Receiver if !has_receiver => return Err("result source receiver is absent"),
                _ => {}
            }
        }
        Ok(())
    }
}

/// Native checked projection; source positions are not stored in its relationship payload.
pub type CheckedOwnershipContract =
    OwnershipContract<crate::symbols::ResolvedType, incan_semantics_core::CanonicalSymbolId>;
