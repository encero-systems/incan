//! Runtime layouts of checked source-local declarations; erased aliases have no storage.

use std::collections::{BTreeMap, BTreeSet};

use incan_lang::lang::types::ownership::{OwnershipMode, from_str as ownership_mode};
use incan_mir_plan_native::{
    EnumDeclaration as PlanEnumDeclaration, FieldLifecycle, FieldMetadata, ModelDeclaration, Parameter, PlanType,
    source_span,
};
use incan_semantics_core::body_ir::{BodyIrModule, EnumDeclaration, LifecycleMetadata, NominalDeclaration};
use incan_semantics_core::{IncanType, SemanticSourceTargetKind};

use crate::callable_methods::callable_methods;
use crate::checked_json_contracts::checked_deserialize_name;
use crate::comparison_methods::comparison_functions;
use crate::fallible_methods::fallible_methods;
use crate::iterator_methods::iterator_methods;
use crate::layout_types::stored_field_type;
use crate::library_model_arguments::{library_model_arguments, local_generic_item};
use crate::marker_traits::validate_marker_traits;
use crate::native_arguments::native_argument_names;
use crate::native_traits::native_trait_adoptions;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::NominalInstance;
use crate::reflection_carriers::is_field_metadata_carrier;
use crate::stored_types::stored_type;
use crate::string_conversion_contracts::{conversion_name, conversion_trait};
use crate::symbols::model_symbol;
use crate::unions::collect_unions;

const FROZEN: [&str; 6] = [
    "FrozenList",
    "FrozenSet",
    "FrozenDict",
    "$FrozenList",
    "$FrozenSet",
    "$FrozenDict",
];

/// Expose canonical native ABI items for a compiled Loaf; its checked manifest retains source visibility.
pub fn publish_local_layouts(models: &mut [ModelDeclaration], enums: &mut [PlanEnumDeclaration]) {
    for model in models.iter_mut() {
        if model.library_item.is_empty() {
            model.public = true;
        }
    }
    for declaration in enums.iter_mut() {
        if declaration.library_item.is_empty() {
            declaration.public = true;
        }
    }
}

/// Retain checked field layout and derives for admitted plain model, class, and newtype declarations.
///
/// `start` lets a resumed unit append layouts for declarations absent from its preserved plan. Semantic
/// construction derives stay in checked Body IR rather than becoming Rust derives.
pub fn lower_model_declarations(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    file: &str,
    intrinsics: &mut Vec<PlanEnumDeclaration>,
    nominal_instances: &[NominalInstance],
    publish_generics: bool,
    start: usize,
) -> Result<Vec<ModelDeclaration>, String> {
    let unions = collect_unions(module);
    let reached = serde_propagation(module)?;
    let comparisons = comparison_functions(constructors, module, intrinsics)?;
    let native_names = native_argument_names(module)?;
    let mut view_lifetimes = ViewLifetimeLayout::start(&module.nominal_declarations, &module.enum_declarations);
    let mut models = Vec::new();
    for owner in start..module.nominal_declarations.len() {
        let context = ModelContext {
            constructors,
            module,
            file,
            nominal_instances,
            publish_generics,
            unions: &unions,
            reached: &reached,
            native_names: &native_names,
        };
        models.push(lower_model_declaration(
            &context,
            owner,
            &comparisons[owner],
            intrinsics,
            &mut view_lifetimes,
        )?);
    }
    Ok(models)
}

/// The per-unit facts every model layout reads.
struct ModelContext<'a> {
    constructors: &'a ConstructorIndex,
    module: &'a BodyIrModule,
    file: &'a str,
    nominal_instances: &'a [NominalInstance],
    publish_generics: bool,
    unions: &'a [IncanType],
    reached: &'a (Vec<String>, Vec<String>),
    native_names: &'a [String],
}

/// One model declaration's native layout.
fn lower_model_declaration(
    context: &ModelContext<'_>,
    owner: usize,
    comparisons: &[String],
    intrinsics: &mut [PlanEnumDeclaration],
    view_lifetimes: &mut ViewLifetimeLayout,
) -> Result<ModelDeclaration, String> {
    let ModelContext {
        constructors,
        module,
        file,
        nominal_instances,
        publish_generics,
        unions,
        reached,
        native_names,
    } = *context;
    let declaration = &module.nominal_declarations[owner];
    let mut item = library_item(declaration).to_owned();
    if publish_generics && item.is_empty() && !declaration.type_parameters.is_empty() {
        item = local_generic_item(declaration, nominal_instances)?;
    }
    let mut library_arguments = Vec::new();
    let mut checked_deserialize = String::new();
    let mut string_conversion_root = String::new();
    let mut string_conversion_trait = String::new();
    if item.is_empty() && declaration.canonical.kind == SemanticSourceTargetKind::Newtype {
        checked_deserialize = checked_deserialize_name(declaration)?;
        string_conversion_root = conversion_name(module, declaration)?;
        if !string_conversion_root.is_empty() {
            string_conversion_trait = conversion_trait(module)?;
        }
    }
    let mut derives: Vec<String> = declaration
        .derives
        .iter()
        .filter(|derive| {
            !matches!(derive.as_str(), "Default" | "Validate")
                && !(!comparisons.is_empty() && matches!(derive.as_str(), "PartialEq" | "Eq" | "PartialOrd" | "Ord"))
        })
        .cloned()
        .collect();
    let mut fields = Vec::new();
    let mut unavailable_fields = Vec::new();
    let mut serde_names = Vec::new();
    if !item.is_empty() {
        library_arguments =
            library_model_arguments(constructors, module, declaration, nominal_instances, unions, intrinsics)?;
        (fields, unavailable_fields) = library_fields(constructors, module, declaration, file, unions, intrinsics);
        // Native Copy is a checked capability, verified again against rustc rather than derived on this alias.
        derives = Vec::new();
        if declaration.derives.iter().any(|derive| derive == "Copy") {
            derives.push("Copy".into());
        }
    } else {
        // Only a plain newtype takes a propagated serde derive; another reached nominal keeps its own derives.
        if declaration.fields.len() == 1 && declaration.fields[0] == "0" {
            derives = with_serde_derives(derives, &declaration.name, reached);
        }
        serde_names = serde_field_names(declaration);
        for slot in 0..declaration.fields.len() {
            let ty = stored_field_type(
                constructors,
                &module.nominal_declarations,
                &module.enum_declarations,
                unions,
                declaration,
                slot,
                intrinsics,
            )
            .map_err(|reason| {
                format!(
                    "{reason} in model {} field {}",
                    declaration.name, declaration.fields[slot]
                )
            })?;
            fields.push(Parameter {
                name: declaration.fields[slot].clone(),
                ty,
                span: source_span(file, 1, 1),
            });
        }
    }
    let (callable_name, callable_signature) = callable_methods(constructors, module, owner, native_names, intrinsics)?;
    let mut view_lifetime_count = 0;
    if item.is_empty() {
        validate_marker_traits(declaration)?;
        view_lifetime_count =
            view_lifetimes.declaration(declaration, &module.nominal_declarations, &module.enum_declarations);
    }
    let mut model = ModelDeclaration::new(
        model_symbol(&declaration.name),
        declaration.public || !item.is_empty(),
        fields,
        declaration.field_public.clone(),
        derives,
        source_span(file, 1, 1),
    );
    model.source_name = declaration.canonical.declaration_name.clone();
    model.view_lifetime_count = view_lifetime_count;
    model.library_item = item;
    model.library_type_arguments = library_arguments;
    model.field_serde_names = serde_names;
    model.checked_deserialize = checked_deserialize;
    model.reflection_metadata = planned_field_metadata(module, constructors, declaration);
    model.field_metadata_carrier = is_field_metadata_carrier(declaration);
    model.comparison_methods = comparisons.to_vec();
    model.fallible_iterator = fallible_methods(constructors, module, owner, native_names, intrinsics)?;
    model.iterator = iterator_methods(constructors, module, owner, native_names, intrinsics)?;
    model.callable_method = callable_name;
    model.callable_signature = callable_signature;
    model.string_conversion_root = string_conversion_root;
    model.string_conversion_trait = string_conversion_trait;
    model.native_trait_adoptions = native_trait_adoptions(module, owner)?;
    model.unavailable_native_fields = unavailable_fields.iter().map(|slot| *slot as i64).collect();
    Ok(model)
}

/// One measured layout and whether no declaration cycle truncated it.
#[derive(Clone, Copy)]
struct ViewLifetimeCount {
    count: i64,
    complete: bool,
}

impl ViewLifetimeCount {
    fn is_complete_zero(self) -> bool {
        self.complete && self.count == 0
    }
}

fn combined_view_lifetimes(left: ViewLifetimeCount, right: ViewLifetimeCount) -> ViewLifetimeCount {
    ViewLifetimeCount {
        count: left.count + right.count,
        complete: left.complete && right.complete,
    }
}

const COMPLETE_ZERO: ViewLifetimeCount = ViewLifetimeCount {
    count: 0,
    complete: true,
};

/// Index view-bearing nominal layouts, then memoize their ordered native lifetime counts.
#[derive(Default)]
pub struct ViewLifetimeLayout {
    cached: BTreeMap<String, i64>,
    active: Vec<String>,
    dependents: BTreeMap<String, Vec<String>>,
    view_types: BTreeSet<String>,
    pending: Vec<String>,
}

impl ViewLifetimeLayout {
    /// Index direct borrow storage and propagate it once through the reverse nominal dependency graph.
    pub fn start(declarations: &[NominalDeclaration], enums: &[EnumDeclaration]) -> Self {
        let mut layout = Self::default();
        for declaration in declarations {
            for field in &declaration.field_types {
                layout.index_type(&declaration.name, field);
            }
        }
        for declaration in enums {
            let owner = declaration.carrier_name();
            for variant in &declaration.variants {
                for field in &variant.fields {
                    layout.index_type(&owner, field);
                }
            }
        }
        layout.propagate_view_types();
        layout
    }

    /// Index direct references and transitive nominal dependencies using the storage rules of `type_lifetimes`.
    fn index_type(&mut self, owner: &str, ty: &IncanType) {
        match ty {
            IncanType::Ref(inner) | IncanType::RefMut(inner) => {
                self.seed(owner);
                self.index_type(owner, inner);
            }
            IncanType::Named(name) => self.record_dependency(name, owner),
            IncanType::Generic { base, args } => {
                if FROZEN.contains(&base.as_str()) {
                    return;
                }
                match ownership_mode(base) {
                    Some(OwnershipMode::Borrowed | OwnershipMode::BorrowedMut | OwnershipMode::Reference) => {
                        self.seed(owner)
                    }
                    Some(OwnershipMode::Owned) => {}
                    None => self.record_dependency(base, owner),
                }
                for argument in args {
                    self.index_type(owner, argument);
                }
            }
            IncanType::Tuple(elements) => {
                for element in elements {
                    self.index_type(owner, element);
                }
            }
            IncanType::NativeArray { element, .. } => self.index_type(owner, element),
            _ => {}
        }
    }

    fn seed(&mut self, owner: &str) {
        if !self.pending.iter().any(|pending| pending == owner) {
            self.pending.push(owner.to_owned());
        }
    }

    fn record_dependency(&mut self, dependency: &str, owner: &str) {
        let owners = self.dependents.entry(dependency.to_owned()).or_default();
        if !owners.iter().any(|known| known == owner) {
            owners.push(owner.to_owned());
        }
    }

    /// Compute the least fixed point of nominal layouts that directly or transitively retain a borrow.
    fn propagate_view_types(&mut self) {
        let mut cursor = 0;
        while cursor < self.pending.len() {
            let selected = self.pending[cursor].clone();
            cursor += 1;
            if self.view_types.contains(&selected) {
                continue;
            }
            self.view_types.insert(selected.clone());
            let owners = self.dependents.get(&selected).cloned().unwrap_or_default();
            for owner in owners {
                if !self.view_types.contains(&owner) {
                    self.pending.push(owner);
                }
            }
        }
    }

    /// Count physical references retained by one source layout; callable fields bind their own signatures.
    pub fn declaration(
        &mut self,
        declaration: &NominalDeclaration,
        declarations: &[NominalDeclaration],
        enums: &[EnumDeclaration],
    ) -> i64 {
        self.named(&declaration.name, declarations, enums).count
    }

    /// Count physical references retained by an enum's payloads in source variant order.
    pub fn enum_declaration(
        &mut self,
        declaration: &EnumDeclaration,
        declarations: &[NominalDeclaration],
        enums: &[EnumDeclaration],
    ) -> i64 {
        self.named(&declaration.carrier_name(), declarations, enums).count
    }

    /// Prove one already canonical-selected nominal has a complete layout with no view lifetime.
    pub fn selected_nominal_is_complete_zero(
        &mut self,
        declaration: &NominalDeclaration,
        declarations: &[NominalDeclaration],
        enums: &[EnumDeclaration],
    ) -> bool {
        self.nominal_fields(declaration, declarations, enums).is_complete_zero()
    }

    /// Prove one already canonical-selected enum has a complete layout with no view lifetime.
    pub fn selected_enum_is_complete_zero(
        &mut self,
        declaration: &EnumDeclaration,
        declarations: &[NominalDeclaration],
        enums: &[EnumDeclaration],
    ) -> bool {
        self.enum_fields(declaration, declarations, enums).is_complete_zero()
    }

    /// Resolve one nominal once when acyclic; a cycle is bounded without caching its path-dependent partial count.
    fn named(
        &mut self,
        name: &str,
        declarations: &[NominalDeclaration],
        enums: &[EnumDeclaration],
    ) -> ViewLifetimeCount {
        if let Some(count) = self.cached.get(name) {
            return ViewLifetimeCount {
                count: *count,
                complete: true,
            };
        }
        if !self.view_types.contains(name) {
            self.cached.insert(name.to_owned(), 0);
            return COMPLETE_ZERO;
        }
        if self.active.iter().any(|active| active == name) {
            return ViewLifetimeCount {
                count: 0,
                complete: false,
            };
        }
        self.active.push(name.to_owned());
        let mut measured = COMPLETE_ZERO;
        let mut found = false;
        if let Some(declaration) = declarations.iter().find(|declaration| declaration.name == name) {
            measured = self.nominal_fields(declaration, declarations, enums);
            found = true;
        } else if let Some(declaration) = enums.iter().find(|declaration| declaration.carrier_name() == name) {
            measured = self.enum_fields(declaration, declarations, enums);
            found = true;
        }
        self.active.pop();
        if found && measured.complete {
            self.cached.insert(name.to_owned(), measured.count);
        }
        measured
    }

    fn nominal_fields(
        &mut self,
        declaration: &NominalDeclaration,
        declarations: &[NominalDeclaration],
        enums: &[EnumDeclaration],
    ) -> ViewLifetimeCount {
        let mut measured = COMPLETE_ZERO;
        for field in &declaration.field_types {
            measured = combined_view_lifetimes(measured, self.type_lifetimes(field, declarations, enums));
        }
        measured
    }

    fn enum_fields(
        &mut self,
        declaration: &EnumDeclaration,
        declarations: &[NominalDeclaration],
        enums: &[EnumDeclaration],
    ) -> ViewLifetimeCount {
        let mut measured = COMPLETE_ZERO;
        for variant in &declaration.variants {
            for field in &variant.fields {
                measured = combined_view_lifetimes(measured, self.type_lifetimes(field, declarations, enums));
            }
        }
        measured
    }

    /// Count reference slots through structural fields and source nominal view layouts.
    fn type_lifetimes(
        &mut self,
        ty: &IncanType,
        declarations: &[NominalDeclaration],
        enums: &[EnumDeclaration],
    ) -> ViewLifetimeCount {
        match ty {
            IncanType::Ref(inner) | IncanType::RefMut(inner) => {
                let nested = self.type_lifetimes(inner, declarations, enums);
                ViewLifetimeCount {
                    count: 1 + nested.count,
                    complete: nested.complete,
                }
            }
            IncanType::Named(name) => self.named(name, declarations, enums),
            IncanType::Generic { base, args } => {
                let arguments_count = self.element_lifetimes(args, declarations, enums);
                match ownership_mode(base) {
                    Some(OwnershipMode::Borrowed | OwnershipMode::BorrowedMut | OwnershipMode::Reference) => {
                        return ViewLifetimeCount {
                            count: 1 + arguments_count.count,
                            complete: arguments_count.complete,
                        };
                    }
                    Some(OwnershipMode::Owned) => return arguments_count,
                    None => {}
                }
                if FROZEN.contains(&base.as_str()) {
                    return COMPLETE_ZERO;
                }
                combined_view_lifetimes(self.named(base, declarations, enums), arguments_count)
            }
            IncanType::Tuple(elements) => self.element_lifetimes(elements, declarations, enums),
            IncanType::NativeArray { element, .. } => self.type_lifetimes(element, declarations, enums),
            _ => COMPLETE_ZERO,
        }
    }

    fn element_lifetimes(
        &mut self,
        elements: &[IncanType],
        declarations: &[NominalDeclaration],
        enums: &[EnumDeclaration],
    ) -> ViewLifetimeCount {
        let mut measured = COMPLETE_ZERO;
        for element in elements {
            measured = combined_view_lifetimes(measured, self.type_lifetimes(element, declarations, enums));
        }
        measured
    }
}

/// Keep the five source-checked labels as ordinary plan data.
fn planned_lifecycle(value: &Option<LifecycleMetadata>) -> Option<FieldLifecycle> {
    value.as_ref().map(|labels| FieldLifecycle {
        since: labels.since.clone(),
        retire_in: labels.retire_in.clone(),
        message: labels.message.clone(),
        by: labels.by.clone(),
        issue: labels.issue.clone(),
    })
}

/// Retain checked visibility and defaults; derive stored selectors from each original typed field slot.
fn planned_field_metadata(
    module: &BodyIrModule,
    constructors: &ConstructorIndex,
    declaration: &NominalDeclaration,
) -> Option<Vec<FieldMetadata>> {
    let fields = declaration.reflection_metadata.as_ref()?;
    Some(
        fields
            .iter()
            .map(|field| FieldMetadata {
                name: field.name.clone(),
                record_identity: field.record_identity.iter().map(|byte| i64::from(*byte)).collect(),
                record_canonical: field.record_canonical,
                renamed_from: field.renamed_from.clone(),
                renamed_from_lifecycle: planned_lifecycle(&field.renamed_from_lifecycle),
                position: field.position as i64,
                default_is_empty: field.default_is_empty,
                stored_type: declaration
                    .field_types
                    .get(field.position)
                    .and_then(|ty| stored_type(module, constructors, ty)),
                alias: field.alias.clone(),
                description: field.description.clone(),
                type_name: field.type_name.clone(),
                has_default: field.has_default,
            })
            .collect(),
    )
}

/// The nominals a serde-deriving declaration's fields hold: those reached under Serialize, then Deserialize.
///
/// Legacy adds a model's serde derives to each source enum and plain newtype its field types name, one level deep,
/// within one module's declarations. A stdlib layout's derives belong to its crate.
pub fn serde_propagation(module: &BodyIrModule) -> Result<(Vec<String>, Vec<String>), String> {
    let mut serialize: Vec<String> = Vec::new();
    let mut deserialize: Vec<String> = Vec::new();
    for declaration in &module.nominal_declarations {
        if !library_item(declaration).is_empty() {
            continue;
        }
        let derives = |name: &str| declaration.derives.iter().any(|derive| derive == name);
        if !derives("serde::Serialize") && !derives("serde::Deserialize") {
            continue;
        }
        let mut reached: Vec<String> = Vec::new();
        for ty in &declaration.field_types {
            reached_names(ty, &mut reached)?;
        }
        for name in reached {
            if !declared_beside(module, &name, declaration) {
                continue;
            }
            if derives("serde::Serialize") && !serialize.contains(&name) {
                serialize.push(name.clone());
            }
            if derives("serde::Deserialize") && !deserialize.contains(&name) {
                deserialize.push(name);
            }
        }
    }
    Ok((serialize, deserialize))
}

/// Whether `name` is a source enum or nominal declared in the same module as `source`.
fn declared_beside(module: &BodyIrModule, name: &str, source: &NominalDeclaration) -> bool {
    if let Some(declaration) = module
        .enum_declarations
        .iter()
        .find(|declaration| declaration.name == name)
    {
        return declaration.canonical.origin == source.canonical.origin;
    }
    if let Some(declaration) = module
        .nominal_declarations
        .iter()
        .find(|declaration| declaration.name == name)
    {
        return declaration.canonical.origin == source.canonical.origin;
    }
    false
}

/// Add the nominal names one field type holds; a union field is refused.
fn reached_names(ty: &IncanType, names: &mut Vec<String>) -> Result<(), String> {
    match ty {
        IncanType::NativeBuiltin(_) => return Err("unsupported Body IR serde of a builtin runtime carrier".into()),
        IncanType::Named(name) => {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        IncanType::Generic { args, .. } => {
            if ty.is_union() {
                return Err("unsupported Body IR serde derive propagation through a union field".into());
            }
            for argument in args {
                reached_names(argument, names)?;
            }
        }
        IncanType::Tuple(elements) => {
            for element in elements {
                reached_names(element, names)?;
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => reached_names(inner, names)?,
        _ => {}
    }
    Ok(())
}

/// Append each serde derive a serde-deriving declaration's fields require of `name`, unless it is already derived.
pub fn with_serde_derives(mut derives: Vec<String>, name: &str, reached: &(Vec<String>, Vec<String>)) -> Vec<String> {
    let (serialize, deserialize) = reached;
    if serialize.iter().any(|candidate| candidate == name) && !derives.iter().any(|derive| derive == "serde::Serialize")
    {
        derives.push("serde::Serialize".into());
    }
    if deserialize.iter().any(|candidate| candidate == name)
        && !derives.iter().any(|derive| derive == "serde::Deserialize")
    {
        derives.push("serde::Deserialize".into());
    }
    derives
}

/// A native layout's fields in their native slots, and the slots whose type the plan cannot carry.
fn library_fields(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declaration: &NominalDeclaration,
    file: &str,
    unions: &[IncanType],
    intrinsics: &[PlanEnumDeclaration],
) -> (Vec<Parameter>, Vec<usize>) {
    let mut fields = Vec::with_capacity(declaration.fields.len());
    let mut unavailable = Vec::new();
    for slot in 0..declaration.fields.len() {
        let ty = match stored_field_type(
            constructors,
            &module.nominal_declarations,
            &module.enum_declarations,
            unions,
            declaration,
            slot,
            intrinsics,
        ) {
            Ok(ty) => ty,
            Err(_) => {
                unavailable.push(slot);
                PlanType::Unit
            }
        };
        fields.push(Parameter {
            name: declaration.fields[slot].clone(),
            ty,
            span: source_span(file, 1, 1),
        });
    }
    (fields, unavailable)
}

/// The compiled item that owns a layout a linked Loaf declares, or nothing for a layout declared here.
pub fn library_item(declaration: &NominalDeclaration) -> &str {
    &declaration.native_item
}

/// The compiled item that owns an enum a linked Loaf declares, or nothing for an enum declared here.
pub fn library_enum_item(declaration: &EnumDeclaration) -> &str {
    &declaration.native_item
}

/// Each aliased field by its alias in the declaration's serde form, as legacy's `#[serde(rename)]` does.
fn serde_field_names(declaration: &NominalDeclaration) -> Vec<String> {
    if !declaration
        .derives
        .iter()
        .any(|derive| derive == "serde::Serialize" || derive == "serde::Deserialize")
    {
        return Vec::new();
    }
    if declaration.field_aliases.iter().all(Option::is_none) {
        return Vec::new();
    }
    declaration
        .field_aliases
        .iter()
        .map(|alias| alias.clone().unwrap_or_default())
        .collect()
}

/// Resolve global storage only by retained canonical identity and exact checked carrier.
pub fn static_slot(
    declarations: &[incan_semantics_core::body_ir::StaticDeclaration],
    value: &incan_semantics_core::body_ir::GlobalPlace,
) -> Result<usize, String> {
    declarations
        .iter()
        .position(|declaration| declaration.canonical == value.identity && declaration.ty == value.ty)
        .ok_or_else(|| {
            format!(
                "unsupported Body IR unretained global storage `{}`",
                value.identity.declaration_name
            )
        })
}

/// Name persistent primitive and set cells after checked carrier admission.
///
/// Literal lists and dictionaries use native guarded cells; the remaining carriers use typed runtime cells.
pub fn static_accessors(ty: &PlanType) -> Result<(&'static str, &'static str), String> {
    match ty {
        PlanType::Set(leaf) => {
            if *leaf == incan_mir_plan_native::ListLeaf::Int {
                return Ok((
                    "incan_native_runtime::read_static_int_set",
                    "incan_native_runtime::write_static_int_set",
                ));
            }
            Err("unsupported Body IR static set carrier".into())
        }
        PlanType::String => Ok((
            "incan_native_runtime::read_static_text",
            "incan_native_runtime::write_static_text",
        )),
        PlanType::Int => Ok((
            "incan_native_runtime::read_static_int",
            "incan_native_runtime::write_static_int",
        )),
        PlanType::Float => Ok((
            "incan_native_runtime::read_static_float",
            "incan_native_runtime::write_static_float",
        )),
        PlanType::Bool => Ok((
            "incan_native_runtime::read_static_bool",
            "incan_native_runtime::write_static_bool",
        )),
        _ => Err("unsupported Body IR static carrier".into()),
    }
}
