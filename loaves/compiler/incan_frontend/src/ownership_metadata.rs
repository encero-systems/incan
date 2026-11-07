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
    /// Whether the returned access sits inside a user-defined or nested aggregate rather than one builtin carrier
    /// payload. Aggregate values retain their own storage while carrying the source dependency.
    #[serde(default)]
    pub aggregate: bool,
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
                        aggregate: source.aggregate,
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

/// Translate only direct input modes. A retained Reference is a result dependency, never an input passing mode.
pub(crate) fn source_input_access(
    mode: incan_lang::lang::types::ownership::OwnershipMode,
) -> Option<incan_semantics_core::IncanInputAccess> {
    use incan_lang::lang::types::ownership::OwnershipMode;
    use incan_semantics_core::IncanInputAccess;
    match mode {
        OwnershipMode::Borrowed => Some(IncanInputAccess::Shared),
        OwnershipMode::BorrowedMut => Some(IncanInputAccess::Exclusive),
        OwnershipMode::Owned => Some(IncanInputAccess::Owned),
        OwnershipMode::Reference => None,
    }
}

/// Resolve only a direct canonical input request. Nested dependencies and returned Reference modes require a
/// later representation; absence of metadata grants ordinary inference, never an explicit access mode.
pub(crate) fn direct_input_access(
    contract: Option<&CheckedOwnershipContract>,
    slot: usize,
) -> Result<incan_semantics_core::IncanInputAccess, &'static str> {
    Ok(direct_input_mode(contract, slot)?
        .map_or(incan_semantics_core::IncanInputAccess::Inferred, |(_, access)| access))
}

/// A result that is a view of exactly one source place: the declared parameter or receiver with its stored field
/// path. The access is shared for `Borrowed[T]` and exclusive for `Reference[T]`. A carried view is one payload of a
/// `Result` or `Option` result, or one element of a tuple result, rather than the whole result; the declaration's
/// header check confirms the carrier.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReturnedView<'a> {
    pub mode: &'a AccessMode<crate::symbols::ResolvedType>,
    pub access: incan_semantics_core::IncanInputAccess,
    pub source: &'a ResultDependency<incan_semantics_core::CanonicalSymbolId>,
    /// Complete structural path from the result root to the explicit access mode.
    pub component: &'a [usize],
    /// The result component holding the view: `0` for `Ok` or `Some`, `1` for `Err`, or a tuple element's index;
    /// `None` for a view returned whole.
    pub carried: Option<usize>,
}

/// Select one top-level owned result independently of whether it retains a declared source. `Owned[T]` is a
/// boundary contract rather than a runtime wrapper, so callers and bodies use the underlying `T` value in both
/// forms. Nested owned components need aggregate dependency facts and remain outside this projection.
pub(crate) fn owned_result(
    contract: Option<&CheckedOwnershipContract>,
) -> Result<Option<&AccessMode<crate::symbols::ResolvedType>>, &'static str> {
    use incan_lang::lang::types::ownership::OwnershipMode;
    let Some(contract) = contract else { return Ok(None) };
    if contract.schema_version != 1 {
        return Err("unsupported checked ownership contract schema");
    }
    let mut results = contract.modes.iter().filter(|mode| mode.component.parameter.is_none());
    let Some(mode) = results.next() else { return Ok(None) };
    if mode.mode != OwnershipMode::Owned || !mode.component.children.is_empty() {
        return Ok(None);
    }
    if results.next().is_some() {
        return Err("an owned result with nested access needs aggregate dependency facts");
    }
    Ok(Some(mode))
}

/// A top-level owned result that retains the declaration's one result-source dependency.
pub(crate) fn owned_result_source(
    contract: Option<&CheckedOwnershipContract>,
) -> Result<
    Option<(
        &AccessMode<crate::symbols::ResolvedType>,
        &ResultDependency<incan_semantics_core::CanonicalSymbolId>,
    )>,
    &'static str,
> {
    let Some(contract) = contract else { return Ok(None) };
    let Some(mode) = owned_result(Some(contract))? else {
        return Ok(None);
    };
    Ok(contract.result_source.as_ref().map(|source| (mode, source)))
}

/// Resolve a direct or carried returned view. Owned aggregates retaining a dependency and views nested elsewhere in a
/// result type need aggregate dependency facts and stay refused; absence of both result facts is an ordinary result.
pub(crate) fn returned_view(
    contract: Option<&CheckedOwnershipContract>,
) -> Result<Option<ReturnedView<'_>>, &'static str> {
    use incan_lang::lang::types::ownership::OwnershipMode;
    use incan_semantics_core::IncanInputAccess;
    let Some(contract) = contract else { return Ok(None) };
    if contract.schema_version != 1 {
        return Err("unsupported checked ownership contract schema");
    }
    let mut results = contract.modes.iter().filter(|mode| mode.component.parameter.is_none());
    match (results.next(), &contract.result_source) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err("a result source has no returned access mode"),
        (Some(mode), None) if mode.mode == OwnershipMode::Owned && mode.component.children.is_empty() => Ok(None),
        (Some(_), None) => Err("borrowed result has no source dependency"),
        (Some(mode), Some(source)) => {
            let carried = match (source.aggregate, mode.component.children.as_slice()) {
                (true, _) | (false, []) => None,
                (false, [component]) => Some(*component),
                (false, _) => return Err("returned access inside another type requires aggregate dependency facts"),
            };
            // The one `from` clause names one source, so one component of the result carries the view.
            if results.next().is_some() {
                return Err("a result with more than one view needs a separate source for each view");
            }
            let access = match mode.mode {
                OwnershipMode::Reference => IncanInputAccess::Exclusive,
                OwnershipMode::Borrowed => IncanInputAccess::Shared,
                OwnershipMode::BorrowedMut => return Err("returned exclusive access is written Reference[T]"),
                OwnershipMode::Owned if carried.is_none() => return Ok(None),
                OwnershipMode::Owned => {
                    return Err("an owned result with nested access needs aggregate dependency facts");
                }
            };
            Ok(Some(ReturnedView {
                mode,
                access,
                source,
                component: &mode.component.children,
                carried,
            }))
        }
    }
}

/// Expose a returned view's value type only when its canonical declaration authorizes the result mode, as for inputs:
/// `T` for a direct view, `Result[T, E]` or `Option[T]` for a carried one, and the tuple of value types for a view
/// carried in a tuple element. An ordinary result is returned unchanged.
pub(crate) fn result_value_type(
    contract: Option<&CheckedOwnershipContract>,
    result: &crate::symbols::ResolvedType,
) -> Result<crate::symbols::ResolvedType, &'static str> {
    use crate::symbols::ResolvedType;
    if let Some(mode) = owned_result(contract)? {
        return Ok(mode.underlying.clone());
    }
    let Some(view) = returned_view(contract)? else {
        return Ok(result.clone());
    };
    let underlying = |view_type: &ResolvedType| match view_type {
        ResolvedType::Generic(_, arguments) => match arguments.as_slice() {
            [underlying] => Ok(underlying.clone()),
            _ => Err("selected returned access requires exactly one value type"),
        },
        _ => Err("selected ownership contract does not match its result metadata"),
    };
    replace_result_component(result, view.component, &underlying)
}

/// Expose the checker-visible result of a returned view. Generic aggregates preserve their instantiated ownership
/// argument so nominal assignment compares the authored type; direct and builtin carried views remain source-place
/// aliases and expose their value type.
pub(crate) fn result_expression_type(
    contract: Option<&CheckedOwnershipContract>,
    result: &crate::symbols::ResolvedType,
) -> Result<crate::symbols::ResolvedType, &'static str> {
    use crate::symbols::ResolvedType;
    if let Some(mode) = owned_result(contract)? {
        return Ok(mode.underlying.clone());
    }
    let Some(view) = returned_view(contract)? else {
        return Ok(result.clone());
    };
    if view.source.aggregate {
        return Ok(result.clone());
    }
    replace_result_component(result, view.component, &|view_type| match view_type {
        ResolvedType::Generic(_, arguments) => match arguments.as_slice() {
            [underlying] => Ok(underlying.clone()),
            _ => Err("selected returned access requires exactly one value type"),
        },
        _ => Err("selected ownership contract does not match its result metadata"),
    })
}

/// Replace one checked ownership wrapper at its structural result path while preserving every enclosing carrier.
fn replace_result_component(
    result: &crate::symbols::ResolvedType,
    component: &[usize],
    leaf: &impl Fn(&crate::symbols::ResolvedType) -> Result<crate::symbols::ResolvedType, &'static str>,
) -> Result<crate::symbols::ResolvedType, &'static str> {
    use crate::symbols::ResolvedType;
    let Some((&slot, rest)) = component.split_first() else {
        return leaf(result);
    };
    match result {
        ResolvedType::Generic(carrier, arguments) => {
            let mut values = arguments.clone();
            let selected = values
                .get(slot)
                .ok_or("returned access component is outside its generic carrier")?;
            values[slot] = replace_result_component(selected, rest, leaf)?;
            Ok(ResolvedType::Generic(carrier.clone(), values))
        }
        ResolvedType::Tuple(elements) => {
            let mut values = elements.clone();
            let selected = values
                .get(slot)
                .ok_or("returned access component is outside its tuple carrier")?;
            values[slot] = replace_result_component(selected, rest, leaf)?;
            Ok(ResolvedType::Tuple(values))
        }
        _ => Err("returned access component requires a generic or tuple carrier"),
    }
}

/// Keep the source mode and its payload together after validating the selected structural slot.
fn direct_input_mode(
    contract: Option<&CheckedOwnershipContract>,
    slot: usize,
) -> Result<
    Option<(
        &AccessMode<crate::symbols::ResolvedType>,
        incan_semantics_core::IncanInputAccess,
    )>,
    &'static str,
> {
    let Some(contract) = contract else { return Ok(None) };
    if contract.schema_version != 1 {
        return Err("unsupported checked input ownership contract schema");
    }
    let mut modes = contract
        .modes
        .iter()
        .filter(|mode| mode.component.parameter == Some(slot));
    let Some(mode) = modes.next() else { return Ok(None) };
    if modes.next().is_some() || !mode.component.children.is_empty() {
        return Err("nested input ownership requires a dependency representation");
    }
    let access =
        source_input_access(mode.mode).ok_or("returned reference ownership is not an executable input contract")?;
    Ok(Some((mode, access)))
}

/// Expose an instantiated input's value type only after its canonical declaration authorizes a direct mode.
/// The closed argument is retained instead of substituting the declaration's potentially open underlying type.
/// Same-spelled user types without an owning contract remain untouched. Fixed payload structure must agree;
/// a declaration binder may close consistently but cannot erase a concrete component mismatch.
pub(crate) fn input_value_type<'a>(
    contract: Option<&CheckedOwnershipContract>,
    slot: usize,
    parameter: &'a crate::symbols::ResolvedType,
) -> Result<&'a crate::symbols::ResolvedType, &'static str> {
    input_value_type_matching(contract, slot, parameter, &|_, _| false)
}

/// Expose an instantiated input value after either exact payload matching or a caller-supplied canonical relation.
pub(crate) fn input_value_type_matching<'a>(
    contract: Option<&CheckedOwnershipContract>,
    slot: usize,
    parameter: &'a crate::symbols::ResolvedType,
    equivalent: &impl Fn(&crate::symbols::ResolvedType, &crate::symbols::ResolvedType) -> bool,
) -> Result<&'a crate::symbols::ResolvedType, &'static str> {
    let Some((mode, _)) = direct_input_mode(contract, slot)? else {
        return Ok(parameter);
    };
    let crate::symbols::ResolvedType::Generic(_, arguments) = parameter else {
        return Err("selected ownership contract does not match its parameter metadata");
    };
    let [underlying] = arguments.as_slice() else {
        return Err("selected ownership input requires exactly one value type");
    };
    if !input_payload::matches(&mode.underlying, underlying) && !equivalent(&mode.underlying, underlying) {
        return Err("selected ownership input payload differs from its checked declaration");
    }
    Ok(underlying)
}

mod input_payload;

#[cfg(test)]
mod tests;
