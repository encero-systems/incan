//! A read of one field of a guarded static copies only that field under the storage guard, and a read-only dictionary
//! call on a static's dictionary reads under the guard without copying the dictionary. No read loads the whole value.

use std::sync::Arc;

use incan_lang::lang::surface::dict_methods::DictMethodId;
use incan_mir_plan_native::{
    BasicBlock, CalleeKind, Function, ListLeaf, Local, OperandKind, PlanType, Projection, RvalueKind, SourceSpan,
    StatementKind, TerminatorKind, source_span,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, BodyIrModule, Constant as CheckedConstant, GlobalPlace, GlobalWritePolicy, LocalId,
    NominalDeclaration, Operand as CheckedOperand, OwnershipFact, Place as CheckedPlace, PlaceElem, PlaceOperand,
    Rvalue as CheckedRvalue, StaticDeclaration, StaticInitializer,
};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
};

use super::bodies::empty_body;
use crate::checked_values::place_read;
use crate::frame::{Builder, place_type};
use crate::hashed_collections::{collection_contains, dictionary_method};
use crate::list_iteration::list_root_place;
use crate::native_type_identity::constructor_index;
use crate::program_context::synthetic_program_context;
use crate::static_storage::write_static;
use crate::string_input::{SyntheticFrame, synthetic_builder};

const MODULE: &str = "app";

fn span() -> SourceSpan {
    source_span("static_field_reads.incn", 1, 1)
}

fn text() -> IncanType {
    IncanType::Primitive(IncanPrimitiveType::Str)
}

fn generic(base: &str, args: Vec<IncanType>) -> IncanType {
    IncanType::Generic {
        base: base.into(),
        args,
    }
}

fn identity(name: &str, kind: SemanticSourceTargetKind, start: usize) -> CanonicalSymbolId {
    CanonicalSymbolId::module_declaration(vec![MODULE.into()], name, kind, HirSourceSpan::new(start, start + 10))
}

/// A source model `name` with the given fields, declared at byte `start`.
fn model(name: &str, start: usize, fields: &[(&str, IncanType)]) -> NominalDeclaration {
    NominalDeclaration {
        record_package_identity: None,
        trait_adoptions: Vec::new(),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span(MODULE, start, start + 10),
        canonical: identity(name, SemanticSourceTargetKind::Model, start),
        name: name.into(),
        fields: fields.iter().map(|(field, _)| (*field).to_owned()).collect(),
        field_identities: fields
            .iter()
            .enumerate()
            .map(|(slot, (field, _))| identity(field, SemanticSourceTargetKind::Field, start + 1 + slot))
            .collect(),
        field_aliases: vec![None; fields.len()],
        field_types: fields.iter().map(|(_, ty)| ty.clone()).collect(),
        owned_callable_fields: Vec::new(),
        field_public: vec![true; fields.len()],
        reflection_fields: None,
        display_method: None,
        newtype_deserialize: Default::default(),
        newtype_string_conversion: Default::default(),
        reflection_metadata: None,
        comparison_equality: None,
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        validated_constructor: None,
        method_partial_defaults: Vec::new(),
        derives: vec!["Clone".into()],
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    }
}

/// `model Selection` with a dictionary field beside a large dictionary, a list, a set, a model and a scalar field;
/// `static SELECTED: Selection = Selection()` and `static VERIFIED: Dict[str, str] = {}`.
fn module() -> BodyIrModule {
    let mut module = BodyIrModule::declarations_only(MODULE.into());
    let dictionary = generic("Dict", vec![text(), text()]);
    module.nominal_declarations = vec![
        model(
            "Selection",
            0,
            &[
                ("registry", dictionary.clone()),
                ("documents", dictionary.clone()),
                ("names", generic("List", vec![text()])),
                ("tags", generic("Set", vec![text()])),
                ("entry", IncanType::Named("Entry".into())),
                ("count", IncanType::Primitive(IncanPrimitiveType::Int)),
            ],
        ),
        model("Entry", 100, &[("name", text())]),
    ];
    module.static_declarations = vec![
        StaticDeclaration {
            canonical: identity("SELECTED", SemanticSourceTargetKind::Static, 200),
            ty: IncanType::Named("Selection".into()),
            initial: StaticInitializer::Computed(Box::new(empty_body("SELECTED"))),
        },
        StaticDeclaration {
            canonical: identity("VERIFIED", SemanticSourceTargetKind::Static, 300),
            ty: dictionary,
            initial: StaticInitializer::Dict(Vec::new()),
        },
    ];
    module
}

/// A frame of module `app` whose Body IR local 0 has type `local`.
fn frame(module: &BodyIrModule, local: PlanType) -> Builder {
    let constructors = Arc::new(constructor_index(&module.nominal_declarations));
    let mut builder = synthetic_builder(
        synthetic_program_context(module, constructors),
        SyntheticFrame {
            locals: vec![
                Local {
                    ty: PlanType::Unit,
                    span: span(),
                },
                Local {
                    ty: local,
                    span: span(),
                },
            ],
            parameter_count: 0,
            file: "static_field_reads.incn".into(),
            module_identity: MODULE.into(),
            unions: Vec::new(),
            intrinsics: Vec::new(),
            name: "read_selection".into(),
            owned: Vec::new(),
        },
    );
    let _ = builder.new_block(&span());
    builder
}

fn global(module: &BodyIrModule, slot: usize) -> GlobalPlace {
    GlobalPlace {
        identity: module.static_declarations[slot].canonical.clone(),
        ty: module.static_declarations[slot].ty.clone(),
        write_policy: GlobalWritePolicy::Rebindable,
    }
}

/// The checked place `SELECTED.<field>`.
fn selected_field(module: &BodyIrModule, field: &str) -> CheckedPlace {
    let declaration = &module.nominal_declarations[0];
    let slot = declaration.fields.iter().position(|name| name == field);
    let canonical = slot.map(|slot| declaration.field_identities[slot].clone());
    let mut place = CheckedPlace::from_global(global(module, 0));
    place.projection.push(PlaceElem::field(field, canonical));
    place
}

fn read(place: CheckedPlace, fact: OwnershipFact) -> ArgumentElement {
    ArgumentElement::One(CheckedOperand::Place(PlaceOperand {
        place,
        fact,
        last_use: false,
    }))
}

fn constant(value: &str) -> ArgumentElement {
    ArgumentElement::One(CheckedOperand::Constant(CheckedConstant::Str(value.into())))
}

fn callees(blocks: &[BasicBlock]) -> Vec<&CalleeKind> {
    blocks
        .iter()
        .filter_map(|block| match &block.terminator.kind {
            TerminatorKind::Call(callee, ..) => Some(&callee.kind),
            _ => None,
        })
        .collect()
}

/// The guarded accessor of the one StaticMethod call the frame makes on `slot`, after checking it loads no snapshot
/// of the slot and keeps no local of the whole stored value.
fn only_accessor(builder: &Builder, slot: i64) -> Result<Function, String> {
    let calls = callees(&builder.blocks);
    if calls
        .iter()
        .any(|kind| matches!(kind, CalleeKind::StaticLoad(loaded) if *loaded == slot))
    {
        return Err(format!("the read loaded the whole static: {calls:?}"));
    }
    let methods: Vec<&String> = calls
        .iter()
        .filter_map(|kind| match kind {
            CalleeKind::StaticMethod(called, name) if *called == slot => Some(name),
            _ => None,
        })
        .collect();
    if methods.len() != 1 {
        return Err(format!("expected one guarded accessor, found {methods:?}"));
    }
    if builder
        .locals
        .iter()
        .any(|local| matches!(local.ty, PlanType::Model(0, _)))
    {
        return Err("the frame keeps a copy of the whole static".into());
    }
    builder
        .synthesized
        .iter()
        .find(|function| function.name == *methods[0])
        .cloned()
        .ok_or_else(|| "the guarded accessor was not synthesized".into())
}

/// The field slots an accessor borrows or copies from its receiver, in order.
fn accessed_slots(accessor: &Function) -> Vec<i64> {
    let mut slots = Vec::new();
    for block in &accessor.blocks {
        for statement in &block.statements {
            let StatementKind::Assign(_, value) = &statement.kind else {
                continue;
            };
            let place = match &value.kind {
                RvalueKind::Borrow(place) => place,
                RvalueKind::Use(operand) => match &operand.kind {
                    OperandKind::Copy(place) | OperandKind::Move(place) => place,
                    OperandKind::Literal(_) => continue,
                },
                _ => continue,
            };
            if let Projection::DerefFields(fields) = &place.projection {
                slots.extend(fields.iter().map(|field| field.slot));
            }
        }
    }
    slots
}

/// Reading a dictionary, list, set, model or scalar field copies that field under the guard, never the whole value.
#[test]
fn a_field_read_of_a_static_copies_only_the_field() -> Result<(), String> {
    let module = module();
    let cases: [(&str, i64, fn(&PlanType) -> bool, OwnershipFact); 5] = [
        (
            "registry",
            0,
            |ty| *ty == PlanType::Dict(ListLeaf::Str, ListLeaf::Str),
            OwnershipFact::Clone,
        ),
        (
            "names",
            2,
            |ty| *ty == PlanType::List(ListLeaf::Str, 1),
            OwnershipFact::Clone,
        ),
        (
            "tags",
            3,
            |ty| *ty == PlanType::Set(ListLeaf::Str),
            OwnershipFact::Clone,
        ),
        (
            "entry",
            4,
            |ty| matches!(ty, PlanType::Model(1, _)),
            OwnershipFact::Clone,
        ),
        ("count", 5, |ty| *ty == PlanType::Int, OwnershipFact::Copy),
    ];
    for (field, slot, expected, fact) in cases {
        let mut builder = frame(&module, PlanType::Unit);
        let value = place_read(&mut builder, &selected_field(&module, field), fact, &span())?;
        let accessor = only_accessor(&builder, 0)?;
        assert_eq!(accessed_slots(&accessor), [slot], "{field} accessor");
        assert!(
            expected(&accessor.return_type),
            "{field} accessor result {:?}",
            accessor.return_type
        );
        let (OperandKind::Move(place) | OperandKind::Copy(place)) = &value.kind else {
            return Err(format!("{field} read gave no place"));
        };
        assert!(expected(&place_type(&builder.locals, place)), "{field} read");
    }
    Ok(())
}

/// A static field used as a receiver is its field alone.
#[test]
fn a_receiver_of_a_static_field_is_the_field_alone() -> Result<(), String> {
    let module = module();
    let mut builder = frame(&module, PlanType::Unit);
    let place = list_root_place(&mut builder, &selected_field(&module, "names"), &span())?;
    assert_eq!(place_type(&builder.locals, &place), PlanType::List(ListLeaf::Str, 1));
    let accessor = only_accessor(&builder, 0)?;
    assert_eq!(accessed_slots(&accessor), [2]);
    Ok(())
}

/// `SELECTED.registry.get("path", "")` and `"path" in SELECTED.registry` read under the guard: only the selected
/// value or the answer leaves it, and the frame copies neither the static nor its dictionary.
#[test]
fn a_dictionary_field_call_reads_under_the_guard() -> Result<(), String> {
    let module = module();
    let dictionary = PlanType::Dict(ListLeaf::Str, ListLeaf::Str);
    let receiver = || read(selected_field(&module, "registry"), OwnershipFact::Borrow);
    let destination = Some(CheckedPlace::from_local(LocalId(0)));

    let mut builder = frame(&module, PlanType::String);
    let arguments = [receiver(), constant("path"), constant("")];
    dictionary_method(&mut builder, DictMethodId::Get, &arguments, &destination, &span())?;
    let accessor = only_accessor(&builder, 0)?;
    assert_eq!(accessed_slots(&accessor), [0]);
    assert!(
        builder.locals.iter().all(|local| local.ty != dictionary),
        "the frame copied the dictionary"
    );
    assert!(
        callees(&accessor.blocks).iter().any(
            |kind| matches!(kind, CalleeKind::Instantiated(path, _) if path == "incan_native_runtime::default_dict")
        ),
        "the accessor does not look the key up"
    );

    let mut builder = frame(&module, PlanType::Bool);
    collection_contains(
        &mut builder,
        false,
        &[receiver(), constant("path")],
        &destination,
        &span(),
    )?;
    let accessor = only_accessor(&builder, 0)?;
    assert_eq!(accessed_slots(&accessor), [0]);
    assert!(
        builder.locals.iter().all(|local| local.ty != dictionary),
        "membership copied the dictionary"
    );
    Ok(())
}

/// `VERIFIED.get(key, "")` on a dictionary static reads its one entry under the guard.
#[test]
fn a_dictionary_static_get_reads_under_the_guard() -> Result<(), String> {
    let module = module();
    let mut builder = frame(&module, PlanType::String);
    let receiver = read(CheckedPlace::from_global(global(&module, 1)), OwnershipFact::Borrow);
    let destination = Some(CheckedPlace::from_local(LocalId(0)));
    dictionary_method(
        &mut builder,
        DictMethodId::Get,
        &[receiver, constant("digest"), constant("")],
        &destination,
        &span(),
    )?;
    let calls = callees(&builder.blocks);
    assert!(
        !calls.iter().any(|kind| matches!(kind, CalleeKind::StaticLoad(1))),
        "the get loaded the dictionary"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|kind| matches!(kind, CalleeKind::StaticMethod(1, _)))
            .count(),
        1
    );
    let dictionary = PlanType::Dict(ListLeaf::Str, ListLeaf::Str);
    assert!(
        builder.locals.iter().all(|local| local.ty != dictionary),
        "the frame copied the dictionary"
    );
    Ok(())
}

/// A field read followed by a write of the same field keeps the value it read: the read's own local is filled under
/// the guard before the write, and nothing writes that local afterwards.
#[test]
fn a_read_before_a_write_keeps_the_value_it_read() -> Result<(), String> {
    let module = module();
    let mut builder = frame(&module, PlanType::Unit);
    let place = selected_field(&module, "registry");
    let value = place_read(&mut builder, &place, OwnershipFact::Clone, &span())?;
    let OperandKind::Move(read_local) = &value.kind else {
        return Err("the read did not produce an owned value".into());
    };
    let read_local = read_local.local;
    write_static(
        &module,
        &mut builder,
        &place,
        &global(&module, 0),
        &CheckedRvalue::Dict(Vec::new()),
        &span(),
    )?;
    let mut read_at = None;
    let mut write_at = None;
    for (position, block) in builder.blocks.iter().enumerate() {
        let TerminatorKind::Call(callee, _, destination, ..) = &block.terminator.kind else {
            continue;
        };
        match &callee.kind {
            CalleeKind::StaticLoad(0) => return Err("a static access loaded the whole value".into()),
            CalleeKind::StaticMethod(0, _) if destination.local == read_local => read_at = Some(position),
            CalleeKind::StaticMethod(0, _) => write_at = Some(position),
            _ => {}
        }
    }
    let (Some(read_at), Some(write_at)) = (read_at, write_at) else {
        return Err(format!(
            "expected a guarded read and a guarded write: {:?}",
            callees(&builder.blocks)
        ));
    };
    assert!(read_at < write_at, "the read happened after the write");
    for block in &builder.blocks[read_at + 1..] {
        for statement in &block.statements {
            if let StatementKind::Assign(target, _) = &statement.kind {
                assert!(target.local != read_local, "a later statement replaced the value read");
            }
        }
        if let TerminatorKind::Call(_, _, target, ..) = &block.terminator.kind {
            assert!(target.local != read_local, "a later call replaced the value read");
        }
    }
    Ok(())
}
