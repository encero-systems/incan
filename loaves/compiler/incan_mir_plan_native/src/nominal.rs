//! Index-free nominal identities for keyed plan products.
//!
//! A plan type names a model or enum by its position in the current unit's tables. A keyed product outlives the unit
//! that produced it, so it never retains such a position: capture replaces each position by a product-local ordinal, in
//! first-seen order, and the product names each ordinal's declaration by its stable identity and meaning digest.
//! Binding replaces each ordinal by the position the current unit resolved for that identity.

use std::collections::HashMap;

use crate::types::{ListLeaf, PlanType, TupleElement};

/// The unit table a plan type's nominal position selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NominalKind {
    Model,
    Enum,
}

impl NominalKind {
    /// The kind's stored spelling: `model` or `enum`.
    pub fn value(self) -> &'static str {
        match self {
            NominalKind::Model => "model",
            NominalKind::Enum => "enum",
        }
    }
}

/// One position in the current unit's model or enum table and the native name it has there.
///
/// Request-local: a product never retains either, because both follow the declaration's place in the unit.
#[derive(Clone, PartialEq)]
pub struct NominalReference {
    pub kind: NominalKind,
    pub index: i64,
    pub name: String,
}

impl NominalReference {
    /// A position with no native name, as a construction or clone names its owner.
    pub fn unnamed(kind: NominalKind, index: i64) -> Self {
        Self {
            kind,
            index,
            name: String::new(),
        }
    }
}

/// A declaration a product names: its table kind, stable index-free identity and checked-meaning digest.
#[derive(Clone, PartialEq)]
pub struct ProductNominal {
    pub kind: NominalKind,
    pub identity: String,
    pub digest: String,
}

/// Translate nominal positions and native names in one direction.
///
/// Capture replaces each unit position by its product-local ordinal and its native name by an empty name, recording
/// both in first-seen order. Binding replaces each ordinal by the unit position and native name `references` resolved
/// for it. A whole-unit record, whose tables are its own, preserves both. Publication folds each carrier position into
/// the plan's enum table after its `fold` source enums.
#[derive(Clone)]
pub struct NominalRemap {
    pub capture: bool,
    pub references: Vec<NominalReference>,
    pub preserve: bool,
    pub fold: i64,
}

impl NominalRemap {
    /// Return the translated position and native name, refusing an ordinal the product does not name.
    pub fn slot(&mut self, kind: NominalKind, index: i64, name: &str) -> Result<NominalReference, String> {
        if self.fold >= 0 {
            if kind == NominalKind::Enum && is_carrier_index(index) {
                return Ok(named(kind, self.fold + carrier_position(index), name));
            }
            return Ok(named(kind, index, name));
        }
        if self.preserve {
            return Ok(named(kind, index, name));
        }
        if self.capture {
            return self.capture_slot(kind, index, name);
        }
        let selected = usize::try_from(index)
            .ok()
            .and_then(|position| self.references.get(position))
            .ok_or_else(|| "plan product names an unresolved nominal ordinal".to_owned())?;
        if selected.kind != kind {
            return Err("plan product nominal ordinal names another table".to_owned());
        }
        Ok(selected.clone())
    }

    /// Assign or reuse the product-local ordinal of one unit position, keeping the first native name it is given.
    fn capture_slot(&mut self, kind: NominalKind, index: i64, name: &str) -> Result<NominalReference, String> {
        for (ordinal, reference) in self.references.iter_mut().enumerate() {
            if reference.kind == kind && reference.index == index {
                if !name.is_empty() && reference.name.is_empty() {
                    // A construction or clone names no native type; a later type supplies the name.
                    reference.name = name.to_owned();
                } else if !name.is_empty() && reference.name != name {
                    return Err("plan product names one declaration by two native names".to_owned());
                }
                return Ok(NominalReference::unnamed(kind, ordinal_index(ordinal)));
            }
        }
        self.references.push(named(kind, index, name));
        Ok(NominalReference::unnamed(
            kind,
            ordinal_index(self.references.len() - 1),
        ))
    }
}

/// A reference with the given native name.
fn named(kind: NominalKind, index: i64, name: &str) -> NominalReference {
    NominalReference {
        kind,
        index,
        name: name.to_owned(),
    }
}

/// A product-local ordinal as a plan position; a product never holds more ordinals than `i64` counts.
fn ordinal_index(ordinal: usize) -> i64 {
    i64::try_from(ordinal).unwrap_or(i64::MAX)
}

/// Start a capture that assigns product-local ordinals in first-seen order.
pub fn capture_remap() -> NominalRemap {
    NominalRemap {
        capture: true,
        references: Vec::new(),
        preserve: false,
        fold: -1,
    }
}

/// Bind each product-local ordinal to the unit position resolved for its declaration.
pub fn binding_remap(references: Vec<NominalReference>) -> NominalRemap {
    NominalRemap {
        capture: false,
        references,
        preserve: false,
        fold: -1,
    }
}

/// Keep the positions of a record whose model and enum tables travel with it.
pub fn preserving_remap() -> NominalRemap {
    NominalRemap {
        capture: false,
        references: Vec::new(),
        preserve: true,
        fold: -1,
    }
}

/// Fold each carrier position into the plan's enum table, after its `sources` source enums.
pub fn folding_remap(sources: i64) -> NominalRemap {
    NominalRemap {
        capture: false,
        references: Vec::new(),
        preserve: false,
        fold: sources,
    }
}

/// Far above any source enum count, so lowering's two enum position ranges never meet.
pub const CARRIER_BASE: i64 = 1_099_511_627_776;

/// The lowering position of the carrier at `position` among the carriers lowering admitted.
pub fn carrier_index(position: i64) -> i64 {
    CARRIER_BASE + position
}

/// Whether a lowering enum position names an admitted carrier rather than a source enum.
pub fn is_carrier_index(index: i64) -> bool {
    index >= CARRIER_BASE
}

/// The position among the admitted carriers that a carrier's lowering position names.
pub fn carrier_position(index: i64) -> i64 {
    index - CARRIER_BASE
}

/// Translate every nominal position in an ordered type list.
pub fn remap_types(types: &[PlanType], remap: &mut NominalRemap) -> Result<Vec<PlanType>, String> {
    types.iter().map(|ty| remap_type(ty, remap)).collect()
}

/// Translate a plan type's nominal positions; every nested type crosses the same translation.
pub fn remap_type(ty: &PlanType, remap: &mut NominalRemap) -> Result<PlanType, String> {
    let model = |remap: &mut NominalRemap, index: i64, name: &str| remap.slot(NominalKind::Model, index, name);
    let enumeration = |remap: &mut NominalRemap, index: i64, name: &str| remap.slot(NominalKind::Enum, index, name);
    Ok(match ty {
        PlanType::Model(index, name) => {
            model(remap, *index, name).map(|slot| PlanType::Model(slot.index, slot.name))?
        }
        PlanType::ModelRef(index, name) => {
            model(remap, *index, name).map(|slot| PlanType::ModelRef(slot.index, slot.name))?
        }
        PlanType::ModelMutRef(index, name) => {
            model(remap, *index, name).map(|slot| PlanType::ModelMutRef(slot.index, slot.name))?
        }
        PlanType::Enum(index, name) => {
            enumeration(remap, *index, name).map(|slot| PlanType::Enum(slot.index, slot.name))?
        }
        PlanType::EnumRef(index, name) => {
            enumeration(remap, *index, name).map(|slot| PlanType::EnumRef(slot.index, slot.name))?
        }
        PlanType::EnumMutRef(index, name) => {
            enumeration(remap, *index, name).map(|slot| PlanType::EnumMutRef(slot.index, slot.name))?
        }
        PlanType::Tuple(elements) => PlanType::Tuple(remap_elements(elements, remap)?),
        PlanType::TupleRef(elements) => PlanType::TupleRef(remap_elements(elements, remap)?),
        PlanType::TupleMutRef(elements) => PlanType::TupleMutRef(remap_elements(elements, remap)?),
        PlanType::List(leaf, depth) => PlanType::List(remap_leaf(leaf, remap)?, *depth),
        PlanType::ListRef(leaf, depth) => PlanType::ListRef(remap_leaf(leaf, remap)?, *depth),
        PlanType::ListMutRef(leaf, depth) => PlanType::ListMutRef(remap_leaf(leaf, remap)?, *depth),
        PlanType::ListIntoIter(leaf, depth) => PlanType::ListIntoIter(remap_leaf(leaf, remap)?, *depth),
        PlanType::ListIntoIterMutRef(leaf, depth) => PlanType::ListIntoIterMutRef(remap_leaf(leaf, remap)?, *depth),
        PlanType::Set(leaf) => PlanType::Set(remap_leaf(leaf, remap)?),
        PlanType::SetRef(leaf) => PlanType::SetRef(remap_leaf(leaf, remap)?),
        PlanType::SetMutRef(leaf) => PlanType::SetMutRef(remap_leaf(leaf, remap)?),
        PlanType::Dict(key, value) => PlanType::Dict(remap_leaf(key, remap)?, remap_leaf(value, remap)?),
        PlanType::DictRef(key, value) => PlanType::DictRef(remap_leaf(key, remap)?, remap_leaf(value, remap)?),
        PlanType::DictMutRef(key, value) => PlanType::DictMutRef(remap_leaf(key, remap)?, remap_leaf(value, remap)?),
        PlanType::Generator(leaf, depth) => PlanType::Generator(remap_leaf(leaf, remap)?, *depth),
        PlanType::GeneratorMutRef(leaf, depth) => PlanType::GeneratorMutRef(remap_leaf(leaf, remap)?, *depth),
        PlanType::GeneratorYield(leaf, depth) => PlanType::GeneratorYield(remap_leaf(leaf, remap)?, *depth),
        PlanType::GeneratorYieldRef(leaf, depth) => PlanType::GeneratorYieldRef(remap_leaf(leaf, remap)?, *depth),
        _ => return remap_composite_type(ty, remap),
    })
}

/// Translate the callable, reference, iterator and native storage types that hold nested type lists.
fn remap_composite_type(ty: &PlanType, remap: &mut NominalRemap) -> Result<PlanType, String> {
    Ok(match ty {
        PlanType::ScalarRef(types) => PlanType::ScalarRef(remap_types(types, remap)?),
        PlanType::ScalarMutRef(types) => PlanType::ScalarMutRef(remap_types(types, remap)?),
        PlanType::FrozenCollection(name, types) => PlanType::FrozenCollection(name.clone(), remap_types(types, remap)?),
        PlanType::FrozenCollectionRef(name, types) => {
            PlanType::FrozenCollectionRef(name.clone(), remap_types(types, remap)?)
        }
        PlanType::TypeToken(types) => PlanType::TypeToken(remap_types(types, remap)?),
        PlanType::ZipIterator(left, right) => {
            PlanType::ZipIterator(remap_element(left, remap)?, remap_element(right, remap)?)
        }
        PlanType::ZipIteratorRef(left, right) => {
            PlanType::ZipIteratorRef(remap_element(left, remap)?, remap_element(right, remap)?)
        }
        PlanType::ZipIteratorMutRef(left, right) => {
            PlanType::ZipIteratorMutRef(remap_element(left, remap)?, remap_element(right, remap)?)
        }
        PlanType::FunctionPointer(types) => PlanType::FunctionPointer(remap_types(types, remap)?),
        PlanType::FunctionItem(name, types) => PlanType::FunctionItem(name.clone(), remap_types(types, remap)?),
        PlanType::Closure(types) => PlanType::Closure(remap_types(types, remap)?),
        PlanType::NativeClosure(types, send, sync) => PlanType::NativeClosure(remap_types(types, remap)?, *send, *sync),
        PlanType::SharedCallable(types) => PlanType::SharedCallable(remap_types(types, remap)?),
        PlanType::ClosureOnce(types) => PlanType::ClosureOnce(remap_types(types, remap)?),
        PlanType::ClosureRef(types) => PlanType::ClosureRef(remap_types(types, remap)?),
        PlanType::BorrowingClosure(types) => PlanType::BorrowingClosure(remap_types(types, remap)?),
        PlanType::BorrowingClosureRef(types) => PlanType::BorrowingClosureRef(remap_types(types, remap)?),
        PlanType::CallableArguments(types) => PlanType::CallableArguments(remap_types(types, remap)?),
        PlanType::Future(types) => PlanType::Future(remap_types(types, remap)?),
        PlanType::NativeSlice(types) => PlanType::NativeSlice(remap_types(types, remap)?),
        PlanType::SliceRef(types) => PlanType::SliceRef(remap_types(types, remap)?),
        PlanType::SliceMutRef(types) => PlanType::SliceMutRef(remap_types(types, remap)?),
        PlanType::NativeArray(types, length) => PlanType::NativeArray(remap_types(types, remap)?, *length),
        PlanType::NativeArrayRef(types, length) => PlanType::NativeArrayRef(remap_types(types, remap)?, *length),
        PlanType::NativeArrayMutRef(types, length) => PlanType::NativeArrayMutRef(remap_types(types, remap)?, *length),
        PlanType::RawPointer(mutable, types) => PlanType::RawPointer(*mutable, remap_types(types, remap)?),
        PlanType::Boxed(types) => PlanType::Boxed(remap_types(types, remap)?),
        PlanType::Trait(identity) => {
            // The publication fold crosses every type of the unit's own plan, and a trait names no position in it.
            if remap.fold >= 0 {
                return Ok(PlanType::Trait(identity.clone()));
            }
            return Err("plan products refuse a canonical source-trait type".to_owned());
        }
        // Every remaining alternative names no nested type and no nominal position.
        _ => ty.clone(),
    })
}

/// Translate an ordered list of collection leaves.
fn remap_leaves(leaves: &[ListLeaf], remap: &mut NominalRemap) -> Result<Vec<ListLeaf>, String> {
    leaves.iter().map(|leaf| remap_leaf(leaf, remap)).collect()
}

/// Translate one collection leaf's nominal positions.
pub fn remap_leaf(leaf: &ListLeaf, remap: &mut NominalRemap) -> Result<ListLeaf, String> {
    Ok(match leaf {
        ListLeaf::Model(index, name) => {
            let slot = remap.slot(NominalKind::Model, *index, name)?;
            ListLeaf::Model(slot.index, slot.name)
        }
        ListLeaf::Enum(index, name) => {
            let slot = remap.slot(NominalKind::Enum, *index, name)?;
            ListLeaf::Enum(slot.index, slot.name)
        }
        ListLeaf::Tuple(elements) => ListLeaf::Tuple(remap_elements(elements, remap)?),
        ListLeaf::FrozenCollection(name, types) => ListLeaf::FrozenCollection(name.clone(), remap_types(types, remap)?),
        ListLeaf::FunctionPointer(types) => ListLeaf::FunctionPointer(remap_types(types, remap)?),
        ListLeaf::TypeToken(types) => ListLeaf::TypeToken(remap_types(types, remap)?),
        ListLeaf::List(leaves, depth) => ListLeaf::List(remap_leaves(leaves, remap)?, *depth),
        ListLeaf::Dict(leaves) => ListLeaf::Dict(remap_leaves(leaves, remap)?),
        ListLeaf::Set(leaves) => ListLeaf::Set(remap_leaves(leaves, remap)?),
        ListLeaf::Reference(types) => ListLeaf::Reference(remap_types(types, remap)?),
        _ => leaf.clone(),
    })
}

/// Translate an ordered tuple layout.
fn remap_elements(elements: &[TupleElement], remap: &mut NominalRemap) -> Result<Vec<TupleElement>, String> {
    elements.iter().map(|element| remap_element(element, remap)).collect()
}

/// Translate one tuple element's nominal positions.
pub fn remap_element(element: &TupleElement, remap: &mut NominalRemap) -> Result<TupleElement, String> {
    Ok(match element {
        TupleElement::Model(index, name) => {
            let slot = remap.slot(NominalKind::Model, *index, name)?;
            TupleElement::Model(slot.index, slot.name)
        }
        TupleElement::Enum(index, name) => {
            let slot = remap.slot(NominalKind::Enum, *index, name)?;
            TupleElement::Enum(slot.index, slot.name)
        }
        TupleElement::ModelRef(index, name) => {
            let slot = remap.slot(NominalKind::Model, *index, name)?;
            TupleElement::ModelRef(slot.index, slot.name)
        }
        TupleElement::ModelMutRef(index, name) => {
            let slot = remap.slot(NominalKind::Model, *index, name)?;
            TupleElement::ModelMutRef(slot.index, slot.name)
        }
        TupleElement::List(leaf, depth) => TupleElement::List(remap_leaf(leaf, remap)?, *depth),
        TupleElement::Dict(key, value) => TupleElement::Dict(remap_leaf(key, remap)?, remap_leaf(value, remap)?),
        TupleElement::Set(leaf) => TupleElement::Set(remap_leaf(leaf, remap)?),
        TupleElement::Tuple(elements) => TupleElement::Tuple(remap_elements(elements, remap)?),
        TupleElement::FunctionPointer(types) => TupleElement::FunctionPointer(remap_types(types, remap)?),
        TupleElement::Reference(mutable, types) => TupleElement::Reference(*mutable, remap_types(types, remap)?),
        TupleElement::SliceRef(types) => TupleElement::SliceRef(remap_types(types, remap)?),
        TupleElement::SliceMutRef(types) => TupleElement::SliceMutRef(remap_types(types, remap)?),
        TupleElement::FrozenCollection(name, types) => {
            TupleElement::FrozenCollection(name.clone(), remap_types(types, remap)?)
        }
        _ => element.clone(),
    })
}

/// The current unit's model, source enum and carrier tables: each position's declaration, by stable identity and
/// meaning digest, and the native name it has at that position.
///
/// `model_positions` and `enum_positions` keep each identity's first position, as lowering's own first-match name
/// resolution does. A carrier's position is its lowering position, [`carrier_index`] of its place among the carriers,
/// so a source enum appended later moves none.
#[derive(Clone)]
pub struct UnitNominals {
    pub models: Vec<ProductNominal>,
    pub enums: Vec<ProductNominal>,
    pub model_names: Vec<String>,
    pub enum_names: Vec<String>,
    pub model_positions: HashMap<String, i64>,
    pub enum_positions: HashMap<String, i64>,
    pub carriers: Vec<ProductNominal>,
    pub carrier_names: Vec<String>,
}

impl UnitNominals {
    /// Describe the next standard carrier lowering admitted, at its lowering position.
    pub fn append_carrier(&mut self, nominal: ProductNominal, name: String) {
        let position = carrier_index(ordinal_index(self.carriers.len()));
        self.enum_positions.entry(nominal.identity.clone()).or_insert(position);
        self.carriers.push(nominal);
        self.carrier_names.push(name);
    }

    /// The declaration and native name at a lowering enum position, if the unit describes one.
    pub fn enum_at(&self, index: i64) -> Option<(&ProductNominal, &str)> {
        if is_carrier_index(index) {
            let position = usize::try_from(carrier_position(index)).ok()?;
            return Some((self.carriers.get(position)?, self.carrier_names.get(position)?.as_str()));
        }
        let position = usize::try_from(index).ok()?;
        Some((self.enums.get(position)?, self.enum_names.get(position)?.as_str()))
    }
}

/// Index described unit tables by identity, keeping each identity's first position.
pub fn unit_nominals(
    models: Vec<ProductNominal>,
    enums: Vec<ProductNominal>,
    model_names: Vec<String>,
    enum_names: Vec<String>,
) -> UnitNominals {
    let mut model_positions = HashMap::new();
    for (index, model) in models.iter().enumerate() {
        model_positions
            .entry(model.identity.clone())
            .or_insert(ordinal_index(index));
    }
    let mut enum_positions = HashMap::new();
    for (index, declaration) in enums.iter().enumerate() {
        enum_positions
            .entry(declaration.identity.clone())
            .or_insert(ordinal_index(index));
    }
    UnitNominals {
        models,
        enums,
        model_names,
        enum_names,
        model_positions,
        enum_positions,
        carriers: Vec::new(),
        carrier_names: Vec::new(),
    }
}

/// Find the current position and native name of each declaration a product names.
///
/// A declaration the unit no longer has, or whose meaning digest changed, makes the whole product a miss.
pub fn resolve_nominals(stored: &[ProductNominal], unit: &UnitNominals) -> Option<Vec<NominalReference>> {
    let mut references = Vec::with_capacity(stored.len());
    for nominal in stored {
        if nominal.kind == NominalKind::Model {
            let position = *unit.model_positions.get(&nominal.identity)?;
            let index = usize::try_from(position).ok()?;
            if unit.models.get(index)?.digest != nominal.digest {
                return None;
            }
            references.push(named(NominalKind::Model, position, unit.model_names.get(index)?));
        } else {
            let position = *unit.enum_positions.get(&nominal.identity)?;
            let (described, name) = unit.enum_at(position)?;
            if described.digest != nominal.digest {
                return None;
            }
            references.push(named(NominalKind::Enum, position, name));
        }
    }
    Some(references)
}

/// Name each referenced unit position by its declaration.
///
/// Refuses a position the unit does not describe, a declaration without a stable identity, and a native name that is
/// not the one the unit gives that position, which a reused product could not reproduce.
pub fn describe_nominals(references: &[NominalReference], unit: &UnitNominals) -> Result<Vec<ProductNominal>, String> {
    let mut nominals = Vec::with_capacity(references.len());
    for reference in references {
        if reference.kind == NominalKind::Model {
            let position = usize::try_from(reference.index)
                .ok()
                .filter(|position| *position < unit.models.len());
            let Some(position) = position else {
                return Err("plan product names a model the unit does not describe".to_owned());
            };
            let native = unit.model_names.get(position).map(String::as_str).unwrap_or_default();
            if !reference.name.is_empty() && native != reference.name {
                return Err("plan product names a model by another native name".to_owned());
            }
            nominals.push(unit.models[position].clone());
        } else {
            let Some((described, name)) = unit.enum_at(reference.index) else {
                return Err("plan product names an enum the unit does not describe".to_owned());
            };
            if !reference.name.is_empty() && name != reference.name {
                return Err("plan product names an enum by another native name".to_owned());
            }
            nominals.push(described.clone());
        }
    }
    if nominals.iter().any(|nominal| nominal.identity.is_empty()) {
        return Err("plan product names a declaration without a stable identity".to_owned());
    }
    Ok(nominals)
}

crate::structural_debug!(NominalReference, "NominalReference", kind, index, name);
crate::structural_debug!(ProductNominal, "ProductNominal", kind, identity, digest);
