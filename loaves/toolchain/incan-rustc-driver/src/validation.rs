//! Preflight validation of the Incan-owned scalar plan. No rustc type crosses this module.

use std::collections::BTreeSet;

use crate::error::PlanError;
use crate::plan::{
    BinaryOp, Callee, CalleeKind, Constant, Function, Operand, OperandKind, Place, Plan, PlanType, Projection,
    RvalueKind, SourceSpan, StatementKind, TerminatorKind, UnaryOp, Unwind,
};

/// A local comparison vocabulary; the public types remain the Incan Loaf's own types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scalar {
    Int,
    Float,
    Bool,
    Unit,
    CheckedInt,
    String,
    StringRef,
    StrRef,
    StringArray(i64),
    StrArray(i64),
    StringArrayRef(i64),
    StrArrayRef(i64),
    StringSlice,
    StrSlice,
}

/// Compare source-authored types without requiring a Rust derive on Incan types.
fn scalar(ty: &PlanType) -> Scalar {
    match ty {
        PlanType::Int => Scalar::Int,
        PlanType::Float => Scalar::Float,
        PlanType::Bool => Scalar::Bool,
        PlanType::Unit => Scalar::Unit,
        PlanType::CheckedInt => Scalar::CheckedInt,
        PlanType::String => Scalar::String,
        PlanType::StringRef => Scalar::StringRef,
        PlanType::StrRef => Scalar::StrRef,
        PlanType::StringArray(count) => Scalar::StringArray(*count),
        PlanType::StrArray(count) => Scalar::StrArray(*count),
        PlanType::StringArrayRef(count) => Scalar::StringArrayRef(*count),
        PlanType::StrArrayRef(count) => Scalar::StrArrayRef(*count),
        PlanType::StringSlice => Scalar::StringSlice,
        PlanType::StrSlice => Scalar::StrSlice,
    }
}

/// Describe a refusal in the containing source function.
fn invalid(function: &Function, reason: impl Into<String>) -> PlanError {
    PlanError::Invalid {
        function: function.name.clone(),
        reason: reason.into(),
    }
}

/// Validate the entire crate before loading source files or invoking rustc.
///
/// This pass checks references, signatures, scalar expression types, and cleanup edges. It is not an ownership
/// checker: stage 3c must only submit checked bodies, and rustc's borrow checker remains a backstop.
pub fn validate(plan: &Plan) -> Result<(), PlanError> {
    span(&plan.span)?;
    let mut names = BTreeSet::new();
    for function in &plan.functions {
        if !identifier(&function.name) || !names.insert(&function.name) {
            return Err(invalid(function, "function name is invalid or duplicated"));
        }
    }
    let mut paths = BTreeSet::new();
    for external in &plan.externals {
        span(&external.span)?;
        let segments: Vec<_> = external.path.split("::").collect();
        if segments.len() < 2 || !segments.iter().all(|segment| identifier(segment)) || !paths.insert(&external.path) {
            return Err(PlanError::Invalid {
                function: external.path.clone(),
                reason: "invalid or duplicate canonical external path".into(),
            });
        }
        if external.parameters.iter().any(|ty| !external_signature_type(scalar(ty)))
            || !external_signature_type(scalar(&external.return_type))
        {
            return Err(PlanError::Invalid {
                function: external.path.clone(),
                reason: "checked pairs and formatting arrays cannot cross a function signature".into(),
            });
        }
    }
    let main = plan
        .functions
        .iter()
        .find(|function| function.name == "main")
        .ok_or_else(|| PlanError::UnknownCallee("main".into()))?;
    if !main.parameters.is_empty() || scalar(&main.return_type) != Scalar::Unit {
        return Err(invalid(
            main,
            "native binary main must take no parameters and return unit",
        ));
    }
    for function in &plan.functions {
        validate_function(plan, function)?;
    }
    Ok(())
}

/// Only ordinary source identifiers are admitted; paths are built as AST nodes, never parsed as source.
fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !matches!(name, "self" | "Self" | "super" | "crate")
}

/// Source coordinates are one-based and must have a source file name.
fn span(source: &SourceSpan) -> Result<(), PlanError> {
    if source.file.is_empty() || source.line < 1 || source.column < 1 {
        return Err(PlanError::Invalid {
            function: source.file.clone(),
            reason: "source span requires a file and positive line and column".into(),
        });
    }
    Ok(())
}

/// Check the parameter-local convention and all nodes in one function.
fn validate_function(plan: &Plan, function: &Function) -> Result<(), PlanError> {
    span(&function.span)?;
    if function.blocks.is_empty() || function.blocks[0].cleanup {
        return Err(invalid(
            function,
            "entry block must exist and cannot be a cleanup block",
        ));
    }
    if function.locals.len() <= function.parameters.len() {
        return Err(invalid(function, "locals must include return place and all parameters"));
    }
    if scalar(&function.locals[0].ty) != scalar(&function.return_type)
        || !source_signature_type(scalar(&function.return_type))
    {
        return Err(invalid(function, "return local must match a scalar return type"));
    }
    let mut parameters = BTreeSet::new();
    for (index, parameter) in function.parameters.iter().enumerate() {
        span(&parameter.span)?;
        if !identifier(&parameter.name) || !parameters.insert(&parameter.name) {
            return Err(invalid(function, "parameter name is invalid or duplicated"));
        }
        if !source_signature_type(scalar(&parameter.ty))
            || scalar(&parameter.ty) != scalar(&function.locals[index + 1].ty)
        {
            return Err(invalid(function, "parameter local differs from its declaration"));
        }
    }
    for local in &function.locals {
        span(&local.span)?;
        validate_array_length(function, scalar(&local.ty))?;
    }
    for block in &function.blocks {
        span(&block.span)?;
        for statement in &block.statements {
            span(&statement.span)?;
            match &statement.kind {
                StatementKind::Assign(destination, value) => {
                    span(&value.span)?;
                    let expected = place(function, destination)?;
                    let result = rvalue(function, &value.kind, expected)?;
                    require(function, expected, result, "assignment")?;
                }
                StatementKind::StorageLive(index) | StatementKind::StorageDead(index) => {
                    local(function, *index)?;
                    if *index
                        <= i64::try_from(function.parameters.len())
                            .map_err(|_| invalid(function, "too many parameters"))?
                    {
                        return Err(invalid(
                            function,
                            "storage markers cannot target return place or parameters",
                        ));
                    }
                }
            }
        }
        span(&block.terminator.span)?;
        terminator(plan, function, &block.terminator.kind, block.cleanup)?;
    }
    Ok(())
}

/// Refuse signed, oversized, or dangling local indices before constructing rustc index types.
fn local(function: &Function, index: i64) -> Result<Scalar, PlanError> {
    usize::try_from(index)
        .ok()
        .and_then(|index| function.locals.get(index))
        .map(|local| scalar(&local.ty))
        .ok_or_else(|| PlanError::UnknownLocal {
            function: function.name.clone(),
            index,
        })
}

/// Resolve a local projection, restricted to the checked result pair.
fn place(function: &Function, value: &Place) -> Result<Scalar, PlanError> {
    span(&value.span)?;
    let ty = local(function, value.local)?;
    match &value.projection {
        Projection::Whole => Ok(ty),
        Projection::Value if ty == Scalar::CheckedInt => Ok(Scalar::Int),
        Projection::Overflow if ty == Scalar::CheckedInt => Ok(Scalar::Bool),
        _ => Err(invalid(function, "only checked integer results support projections")),
    }
}

/// Check an edge's index and whether it crosses the normal/cleanup boundary correctly.
fn edge(function: &Function, index: i64, cleanup: bool) -> Result<(), PlanError> {
    let block = usize::try_from(index)
        .ok()
        .and_then(|index| function.blocks.get(index))
        .ok_or_else(|| PlanError::UnknownBlock {
            function: function.name.clone(),
            index,
        })?;
    if block.cleanup != cleanup {
        return Err(invalid(function, "edge crosses normal and cleanup control flow"));
    }
    Ok(())
}

/// Cleanup edges must name cleanup blocks; cleanup code cannot install a nested cleanup edge.
fn unwind(function: &Function, action: &Unwind, cleanup: bool) -> Result<(), PlanError> {
    match action {
        Unwind::Continue if !cleanup => Ok(()),
        Unwind::Cleanup(index) if !cleanup => edge(function, *index, true),
        Unwind::Terminate if cleanup => Ok(()),
        _ => Err(invalid(function, "cleanup code cannot unwind again")),
    }
}

/// Infer a source operand's exact scalar representation.
fn operand(function: &Function, value: &Operand) -> Result<Scalar, PlanError> {
    span(&value.span)?;
    match &value.kind {
        OperandKind::Copy(value) => {
            let ty = place(function, value)?;
            if matches!(ty, Scalar::String | Scalar::StringArray(_)) {
                return Err(invalid(function, "owned formatting values cannot be copied"));
            }
            Ok(ty)
        }
        OperandKind::Move(value) => place(function, value),
        OperandKind::Literal(value) => Ok(match value {
            Constant::Int(_) => Scalar::Int,
            Constant::Float(_) => Scalar::Float,
            Constant::Bool(_) => Scalar::Bool,
            Constant::Unit => Scalar::Unit,
            Constant::Text(_) => Scalar::StrRef,
        }),
    }
}

/// Keep type mismatches explicit rather than allowing rustc MIR validation to ICE.
fn require(function: &Function, actual: Scalar, expected: Scalar, context: &str) -> Result<(), PlanError> {
    if actual != expected {
        return Err(invalid(
            function,
            format!("{context}: expected {expected:?}, found {actual:?}"),
        ));
    }
    Ok(())
}

/// Type scalar rvalues, including the checked integer pair rustc produces for arithmetic.
fn rvalue(function: &Function, value: &RvalueKind, expected: Scalar) -> Result<Scalar, PlanError> {
    match value {
        RvalueKind::Use(value) => operand(function, value),
        RvalueKind::IntToFloat(value) => {
            require(function, operand(function, value)?, Scalar::Int, "int-to-float")?;
            Ok(Scalar::Float)
        }
        RvalueKind::Unary(op, value) => {
            let ty = operand(function, value)?;
            match op {
                UnaryOp::Not if ty == Scalar::Bool => Ok(ty),
                UnaryOp::Negate if matches!(ty, Scalar::Int | Scalar::Float) => Ok(ty),
                _ => Err(invalid(function, "invalid unary operand type")),
            }
        }
        RvalueKind::Binary(op, left, right) => {
            let ty = operand(function, left)?;
            require(function, operand(function, right)?, ty, "binary operands")?;
            binary_result(function, op, ty)
        }
        RvalueKind::Array(elements) => validate_array(function, elements, expected),
        RvalueKind::Borrow(value) => match place(function, value)? {
            Scalar::String => Ok(Scalar::StringRef),
            Scalar::StringArray(count) => Ok(Scalar::StringArrayRef(count)),
            Scalar::StrArray(count) => Ok(Scalar::StrArrayRef(count)),
            _ => Err(invalid(function, "shared borrow requires an owned formatting value")),
        },
        RvalueKind::UnsizeSlice(value) => match operand(function, value)? {
            Scalar::StringArrayRef(_) => Ok(Scalar::StringSlice),
            Scalar::StrArrayRef(_) => Ok(Scalar::StrSlice),
            _ => Err(invalid(function, "slice coercion requires a shared formatting-array reference")),
        },
    }
}

/// Source functions expose only scalar or owned text values; formatting arrays and references stay body-internal.
fn source_signature_type(ty: Scalar) -> bool {
    matches!(ty, Scalar::Int | Scalar::Float | Scalar::Bool | Scalar::Unit | Scalar::String)
}

/// Runtime signatures additionally admit shared text and slice views, whose regions metadata checking erases.
fn external_signature_type(ty: Scalar) -> bool {
    source_signature_type(ty) || matches!(ty, Scalar::StringRef | Scalar::StrRef | Scalar::StringSlice | Scalar::StrSlice)
}

/// Refuse an invalid dimension before constructing native array constants, including unused locals.
fn validate_array_length(function: &Function, ty: Scalar) -> Result<(), PlanError> {
    match ty {
        Scalar::StringArray(count) | Scalar::StrArray(count) | Scalar::StringArrayRef(count) | Scalar::StrArrayRef(count) if count < 0 => {
            Err(invalid(function, "formatting array length must be nonnegative"))
        }
        _ => Ok(()),
    }
}

/// Check a formatting array's exact length and each element against its declared destination type.
fn validate_array(function: &Function, values: &[Operand], expected: Scalar) -> Result<Scalar, PlanError> {
    let (element, count) = match expected {
        Scalar::StringArray(count) => (Scalar::String, count),
        Scalar::StrArray(count) => (Scalar::StrRef, count),
        _ => return Err(invalid(function, "array expression requires a formatting-array destination")),
    };
    if usize::try_from(count).ok() != Some(values.len()) {
        return Err(invalid(function, "formatting array length differs from its elements"));
    }
    for value in values {
        require(function, operand(function, value)?, element, "formatting array element")?;
    }
    Ok(expected)
}

/// Arithmetic on int is always checked; floating arithmetic follows rustc's IEEE operations.
fn binary_result(function: &Function, op: &BinaryOp, ty: Scalar) -> Result<Scalar, PlanError> {
    match op {
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply if ty == Scalar::Int => Ok(Scalar::CheckedInt),
        BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide if ty == Scalar::Float => {
            Ok(Scalar::Float)
        }
        BinaryOp::Equal | BinaryOp::NotEqual if matches!(ty, Scalar::Int | Scalar::Float | Scalar::Bool) => {
            Ok(Scalar::Bool)
        }
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual
            if matches!(ty, Scalar::Int | Scalar::Float) =>
        {
            Ok(Scalar::Bool)
        }
        _ => Err(invalid(function, "invalid binary operand type")),
    }
}

/// Resolve a function reference against source-named declarations before invoking rustc.
fn signature(plan: &Plan, callee: &Callee) -> Result<(Vec<Scalar>, Scalar), PlanError> {
    span(&callee.span)?;
    match &callee.kind {
        CalleeKind::Planned(name) => {
            let function = plan
                .functions
                .iter()
                .find(|function| &function.name == name)
                .ok_or_else(|| PlanError::UnknownCallee(name.clone()))?;
            Ok((
                function
                    .parameters
                    .iter()
                    .map(|parameter| scalar(&parameter.ty))
                    .collect(),
                scalar(&function.return_type),
            ))
        }
        CalleeKind::External(path) => {
            let external = plan
                .externals
                .iter()
                .find(|external| &external.path == path)
                .ok_or_else(|| PlanError::UnknownCallee(path.clone()))?;
            Ok((
                external.parameters.iter().map(scalar).collect(),
                scalar(&external.return_type),
            ))
        }
    }
}

/// Validate terminator values and every normal and unwinding successor.
fn terminator(plan: &Plan, function: &Function, value: &TerminatorKind, cleanup: bool) -> Result<(), PlanError> {
    match value {
        TerminatorKind::Goto(target) => edge(function, *target, cleanup),
        TerminatorKind::SwitchBool(condition, false_target, true_target) => {
            require(
                function,
                operand(function, condition)?,
                Scalar::Bool,
                "switch condition",
            )?;
            edge(function, *false_target, cleanup)?;
            edge(function, *true_target, cleanup)
        }
        TerminatorKind::Call(callee, arguments, destination, target, action) => {
            let (parameters, result) = signature(plan, callee)?;
            if parameters.len() != arguments.len() {
                return Err(invalid(
                    function,
                    "call argument count differs from the callee signature",
                ));
            }
            for (argument, parameter) in arguments.iter().zip(parameters) {
                require(function, operand(function, argument)?, parameter, "call argument")?;
            }
            require(function, place(function, destination)?, result, "call destination")?;
            edge(function, *target, cleanup)?;
            unwind(function, action, cleanup)
        }
        TerminatorKind::Drop(value, target, action) => {
            place(function, value)?;
            edge(function, *target, cleanup)?;
            unwind(function, action, cleanup)
        }
        TerminatorKind::AssertOverflow(condition, op, left, right, target, action) => {
            require(function, operand(function, condition)?, Scalar::Bool, "overflow flag")?;
            require(function, operand(function, left)?, Scalar::Int, "overflow left operand")?;
            require(
                function,
                operand(function, right)?,
                Scalar::Int,
                "overflow right operand",
            )?;
            if !matches!(op, BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply) {
                return Err(invalid(
                    function,
                    "overflow assertion requires add, subtract, or multiply",
                ));
            }
            edge(function, *target, cleanup)?;
            unwind(function, action, cleanup)
        }
        TerminatorKind::Return if cleanup => Err(invalid(function, "cleanup block cannot return normally")),
        TerminatorKind::UnwindResume if !cleanup => Err(invalid(function, "unwind resume requires a cleanup block")),
        TerminatorKind::Return | TerminatorKind::Unreachable | TerminatorKind::UnwindResume => Ok(()),
    }
}
