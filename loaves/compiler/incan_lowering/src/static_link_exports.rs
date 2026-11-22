//! Native exports for the statics selected by the declaring Loaf's executable publication.

use std::collections::{BTreeMap, BTreeSet};

use incan_mir_plan_native::plan::static_accessor_name;
use incan_mir_plan_native::{
    Constant, Function, Local, Parameter, PlanExport, PlanType, RvalueKind, SourceSpan, TerminatorKind, assign, place,
    source_span,
};
use incan_semantics_core::CanonicalSymbolId;
use incan_semantics_core::body_ir::{GlobalPlace, GlobalWritePolicy, LinkedStaticAccessors, OwnershipFact, Place};

use crate::collections::is_owned_collection;
use crate::computed_statics::{module_initializer_name, static_owner};
use crate::frame::literal;
use crate::frame_instances::merge_externals;
use crate::lowering::Inventories;
use crate::models::model_type;
use crate::program_context::ProgramContext;
use crate::static_storage::{persist_static_value, read_static};
use crate::string_input::{SyntheticFrame, synthetic_builder};

/// Export the selected native cells, or wrap the producer's scalar runtime operations in ordinary planned
/// functions. Both use the same slot that the declaring unit's own checked frames use.
pub(crate) fn publish_static_accessors(
    program: &ProgramContext,
    selected: &BTreeMap<CanonicalSymbolId, LinkedStaticAccessors>,
    file: &str,
    inventories: &mut Inventories,
    exports: &mut Vec<PlanExport>,
) -> Result<(), String> {
    if selected.is_empty() {
        return Ok(());
    }
    let mut emitted = 0;
    let mut initializers = BTreeSet::new();
    let span = source_span(file, 1, 1);
    for (slot, declaration) in program.statics.iter().enumerate() {
        let Some(accessors) = selected.get(&declaration.canonical) else {
            continue;
        };
        let slot = i64::try_from(slot).map_err(|_| "native static slot exceeds its plan representation")?;
        emitted += 1;
        if let Some(guarded) = &accessors.guarded {
            export(exports, &accessors.load, &static_accessor_name(slot, "load"));
            if let Some(store) = &accessors.store {
                export(exports, store, &static_accessor_name(slot, "store"));
            }
            export(exports, &guarded.force, &static_accessor_name(slot, "force"));
            export(exports, &guarded.with_ref, &static_accessor_name(slot, "with_ref"));
            if let Some(with_mut) = &guarded.with_mut {
                export(exports, with_mut, &static_accessor_name(slot, "with_mut"));
            }
            if let Some(initialize) = &accessors.initialize_module
                && initializers.insert(initialize)
            {
                export(
                    exports,
                    initialize,
                    &module_initializer_name(&static_owner(declaration)),
                );
            }
            continue;
        }
        let global = GlobalPlace {
            identity: declaration.canonical.clone(),
            ty: declaration.ty.clone(),
            write_policy: if accessors.store.is_some() {
                GlobalWritePolicy::Rebindable
            } else {
                GlobalWritePolicy::ReadOnly
            },
        };
        let owner = static_owner(declaration);
        let name = static_accessor_name(slot, "load");
        lower_scalar_accessor(program, &global, &owner, &name, false, &span, inventories)?;
        export(exports, &accessors.load, &name);
        if let Some(store) = &accessors.store {
            let name = static_accessor_name(slot, "store");
            lower_scalar_accessor(program, &global, &owner, &name, true, &span, inventories)?;
            export(exports, store, &name);
        }
    }
    if emitted != selected.len() {
        return Err("native plan omitted a static selected by Loaf publication".into());
    }
    Ok(())
}

/// Publish the exact recorded native name as an alias of an item declared by this unit.
fn export(exports: &mut Vec<PlanExport>, public: &str, local: &str) {
    exports.push(PlanExport {
        name: public.into(),
        path: format!("crate::{local}"),
    });
}

/// Build one closed runtime-cell accessor with the same read and assignment lowering as an ordinary body.
fn lower_scalar_accessor(
    program: &ProgramContext,
    global: &GlobalPlace,
    owner: &str,
    name: &str,
    store: bool,
    span: &SourceSpan,
    inventories: &mut Inventories,
) -> Result<(), String> {
    let ty = model_type(&program.constructors, &program.nominals, &global.ty, "")?;
    let result = if store { PlanType::Unit } else { ty.clone() };
    let mut locals = vec![Local {
        ty: result.clone(),
        span: span.clone(),
    }];
    let mut parameters = Vec::new();
    if store {
        parameters.push(Parameter {
            name: "value".into(),
            ty: ty.clone(),
            span: span.clone(),
        });
        locals.push(Local {
            ty: ty.clone(),
            span: span.clone(),
        });
    }
    let mut builder = synthetic_builder(
        program.clone(),
        SyntheticFrame {
            locals,
            parameter_count: i64::from(store),
            file: span.file.to_string(),
            module_identity: owner.into(),
            unions: Vec::new(),
            intrinsics: Vec::new(),
            name: name.into(),
            owned: Vec::new(),
        },
    );
    builder.new_block(span);
    let output = if store {
        persist_static_value(&mut builder, global, place(1, span), span)?;
        literal(Constant::Unit, span)
    } else {
        let source = Place::from_global(global.clone());
        let fact = if ty == PlanType::String || is_owned_collection(&ty) {
            OwnershipFact::Clone
        } else {
            OwnershipFact::Copy
        };
        read_static(&mut builder, &source, global, fact, span)?
    };
    builder.emit(assign(place(0, span), RvalueKind::Use(output), span));
    builder.finish(TerminatorKind::Return, span);
    inventories.functions.push(Function::new(
        builder.name,
        parameters,
        result,
        builder.locals,
        builder.blocks,
        span.clone(),
    ));
    merge_externals(&mut inventories.externals, builder.externals)
}
