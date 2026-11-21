//! A module's static initializer is named by its module's stable identity: a static added to another module never
//! renames it, so a saved product whose frame enters it still links against the current plan.

use std::sync::Arc;

use incan_mir_plan_native::{CalleeKind, Function, Local, PlanType, SourceSpan, TerminatorKind, source_span};
use incan_semantics_core::body_ir::{BodyIrModule, StaticDeclaration, StaticInitializer};
use incan_semantics_core::{
    CanonicalSymbolId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind, SymbolNamespace,
    SymbolOrigin,
};

use super::bodies::empty_body;
use crate::computed_statics::{initialize_module_statics, module_initializer, module_static_key, static_owner};
use crate::native_type_identity::ConstructorIndex;
use crate::program_context::synthetic_program_context;
use crate::string_input::{SyntheticFrame, synthetic_builder};

/// A computed static `name` of module `module`, declared at byte `start`.
fn computed(module: &str, name: &str, start: usize) -> StaticDeclaration {
    StaticDeclaration {
        canonical: CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(vec![module.into()]),
            declaration_name: name.into(),
            kind: SemanticSourceTargetKind::Static,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(start, start + 10),
        },
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        initial: StaticInitializer::Computed(Box::new(empty_body(name))),
    }
}

fn span() -> SourceSpan {
    source_span("beta.incn", 1, 1)
}

/// The frame of a function of module `owner` that only initializes its module on entry, lowered over `statics`.
fn entering_frame(statics: &[StaticDeclaration], owner: &str) -> Function {
    let mut module = BodyIrModule::declarations_only("main".into());
    module.static_declarations = statics.to_vec();
    let program = synthetic_program_context(&module, Arc::new(ConstructorIndex::default()));
    let frame = SyntheticFrame {
        locals: vec![Local {
            ty: PlanType::Unit,
            span: span(),
        }],
        parameter_count: 0,
        file: "beta.incn".into(),
        module_identity: owner.into(),
        unions: Vec::new(),
        intrinsics: Vec::new(),
        name: "read_beta".into(),
        owned: Vec::new(),
    };
    let mut builder = synthetic_builder(program, frame);
    builder.new_block(&span());
    initialize_module_statics(&mut builder, owner, &span());
    builder.finish(TerminatorKind::Return, &span());
    Function::new(
        builder.name,
        Vec::new(),
        PlanType::Unit,
        builder.locals,
        builder.blocks,
        span(),
    )
}

/// The planned functions a frame calls.
fn planned_calls(function: &Function) -> Vec<String> {
    function
        .blocks
        .iter()
        .filter_map(|block| match &block.terminator.kind {
            TerminatorKind::Call(callee, ..) => match &callee.kind {
                CalleeKind::Planned(name) => Some(name.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[test]
fn a_static_added_to_another_module_keeps_a_module_s_initializer_and_its_saved_frames() {
    let before = vec![computed("alpha", "first", 0), computed("beta", "only", 100)];
    let after = vec![
        computed("alpha", "first", 0),
        computed("alpha", "second", 20),
        computed("beta", "only", 100),
    ];
    let alpha = static_owner(&after[0]);
    let beta = static_owner(&after[2]);
    // The frame a saved product holds was lowered before the static was added; it is the frame lowered now.
    let saved = entering_frame(&before, &beta);
    assert_eq!(
        saved,
        entering_frame(&after, &beta),
        "a static added to another module changed this frame"
    );
    // Its entry call names the initializer the current plan defines for its module.
    let initializer = module_initializer(&after, module_static_key(&after, &beta), &span());
    assert_eq!(planned_calls(&saved), [initializer.name.clone()]);
    assert_eq!(
        initializer.name,
        module_initializer(&before, module_static_key(&before, &beta), &span()).name
    );
    // Each module keeps its own initializer.
    let first = module_initializer(&after, module_static_key(&after, &alpha), &span());
    assert_ne!(first.name, initializer.name);
}
