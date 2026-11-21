//! Concrete function frames: their storage, parameters, body and entry wrappers.

use incan_mir_plan_native::{EnumDeclaration, ExternalFunction, Function};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{Body, BodyIrModule};

use crate::pending::untranslated;
use crate::program_context::ProgramContext;
use crate::type_parameters::Instance;

/// The role a frame plays for a decorated binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecoratorFrame {
    Normal,
}

/// A lowered frame with the runtime externals it calls, the instances it requests and the functions it lifted.
pub type LoweredFrame = (Function, Vec<ExternalFunction>, Vec<Instance>, Vec<Function>);

/// Build a concrete frame and retain its instance requests and lifted functions.
#[allow(clippy::too_many_arguments)]
pub fn lower_function(
    module: &BodyIrModule,
    program: &ProgramContext,
    body: &Body,
    unions: &[IncanType],
    source: &str,
    file: &str,
    intrinsics: &mut Vec<EnumDeclaration>,
    type_arguments: &[IncanType],
    role: DecoratorFrame,
) -> Result<LoweredFrame, String> {
    let _ = (
        module,
        program,
        body,
        unions,
        source,
        file,
        intrinsics,
        type_arguments,
        role,
    );
    Err(untranslated("function_frames::lower_function"))
}
