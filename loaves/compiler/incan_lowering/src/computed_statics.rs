//! Computed module statics (RFC 052): their initializer names, module initialization keys and frame entry.
//!
//! Legacy declares each computed static as lazily initialized storage and, on entry to every function of the module,
//! forces the module's statics once in declaration order; a computed storage read forces its own static first. A plan
//! keeps the same order: each module with computed statics gets one planned initializer, and every function of the
//! module calls it on entry.

use incan_mir_plan_native::{CalleeKind, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{Body, StaticDeclaration, StaticInitializer};
use incan_semantics_core::{SemanticSourceTargetKind, canonical_module_identity};

use crate::frame::{Builder, temporary, terminate_call};

/// Report whether a static's initializer is a computed body rather than an effect-free literal.
pub fn is_computed_static(declaration: &StaticDeclaration) -> bool {
    matches!(declaration.initial, StaticInitializer::Computed(_))
}

/// The declaring module of a static, read from its canonical identity.
pub fn static_owner(declaration: &StaticDeclaration) -> String {
    canonical_module_identity(&declaration.canonical).unwrap_or_default()
}

/// The first computed static slot module `owner` declares, which keys its initialization, or -1 for none.
pub fn module_static_key(statics: &[StaticDeclaration], owner: &str) -> i64 {
    for (slot, declaration) in statics.iter().enumerate() {
        if is_computed_static(declaration) && static_owner(declaration) == owner {
            return slot as i64;
        }
    }
    -1
}

/// Name the planned function initializing the statics of the module keyed `key`.
pub fn module_initializer_name(key: i64) -> String {
    format!("__incan_init_module_statics_{key}")
}

/// Call module `owner`'s static initializer, as legacy calls `__incan_init_module_statics` at a function entry.
pub fn initialize_module_statics(builder: &mut Builder, owner: &str, span: &SourceSpan) {
    let key = module_static_key(&builder.program.statics, owner);
    if key < 0 {
        return;
    }
    let output = temporary(builder, PlanType::Unit, span);
    terminate_call(
        builder,
        CalleeKind::Planned(module_initializer_name(key)),
        Vec::new(),
        output,
        span,
    );
}

/// Report whether a body initializes its module's statics on entry: every function and method does, as in legacy,
/// but a computed static's or constant's own initializer does not, because the module's initialization is what runs
/// it.
pub fn enters_module_initialization(body: &Body) -> bool {
    match &body.canonical {
        Some(identity) => {
            identity.kind != SemanticSourceTargetKind::Static && identity.kind != SemanticSourceTargetKind::Const
        }
        None => true,
    }
}
