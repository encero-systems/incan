//! Carrier positions during lowering, and their fold into the plan's one enum table.
//!
//! While a unit lowers, a standard carrier lowering admits is named by its own position among the carriers, offset by
//! [`CARRIER_BASE`](crate::nominal::CARRIER_BASE), and a source enum by its position among the source enums. Each table
//! only appends, so a module whose enums arrive in a later lowering attempt moves no carrier a frame already names. The
//! plan keeps one enum table, the source enums then the carriers; publication folds every carrier position into it
//! once, after the last append.

use crate::body::{
    BasicBlock, Callee, CalleeKind, Constant, FieldProjection, FrozenValue, Local, Operand, OperandKind, Parameter,
    Place, Projection, Rvalue, RvalueKind, Statement, StatementKind, Terminator, TerminatorKind,
};
use crate::nominal::{NominalKind, NominalRemap, folding_remap, remap_element, remap_leaf, remap_type, remap_types};
use crate::plan::{
    EnumDeclaration, EnumVariant, ExternalFunction, FallibleIteratorMethods, Function, GenericEnumDeclaration,
    GenericEnumVariant, GenericLayoutField, GenericLayoutType, GenericModelDeclaration, IteratorMethods,
    ModelDeclaration, Plan, StaticCell, ZipCarrier,
};

/// Name every carrier by its place in the plan's enum table, after the `sources` source enums.
///
/// Every other position, and every source enum position, is kept.
pub fn fold_carrier_positions(plan: &Plan, sources: i64) -> Result<Plan, String> {
    let mut remap = folding_remap(sources);
    let mut statics = Vec::with_capacity(plan.statics.len());
    for cell in &plan.statics {
        statics.push(StaticCell {
            ty: remap_type(&cell.ty, &mut remap)?,
            ..cell.clone()
        });
    }
    let mut zips = Vec::with_capacity(plan.zip_carriers.len());
    for carrier in &plan.zip_carriers {
        zips.push(ZipCarrier {
            left: remap_element(&carrier.left, &mut remap)?,
            right: remap_element(&carrier.right, &mut remap)?,
            target: remap_type(&carrier.target, &mut remap)?,
        });
    }
    Ok(Plan {
        enums: collect(&plan.enums, |declaration| fold_enum(declaration, &mut remap))?,
        models: collect(&plan.models, |declaration| fold_model(declaration, &mut remap))?,
        functions: collect(&plan.functions, |function| fold_function(function, &mut remap))?,
        externals: collect(&plan.externals, |external| fold_external(external, &mut remap))?,
        span: plan.span.clone(),
        statics,
        exports: plan.exports.clone(),
        tests: plan.tests.clone(),
        zip_carriers: zips,
        programs: plan.programs.clone(),
        generic_models: collect(&plan.generic_models, |declaration| {
            fold_generic_model(declaration, &mut remap)
        })?,
        generic_enums: collect(&plan.generic_enums, |declaration| {
            fold_generic_enum(declaration, &mut remap)
        })?,
    })
}

/// Fold a unit's carrier positions in place once its last carrier is appended after its `sources` source enums.
pub fn fold_inventories(
    functions: &mut [Function],
    models: &mut [ModelDeclaration],
    enums: &mut [EnumDeclaration],
    externals: &mut [ExternalFunction],
    sources: i64,
) -> Result<(), String> {
    let mut remap = folding_remap(sources);
    for function in functions.iter_mut() {
        *function = fold_function(function, &mut remap)?;
    }
    for model in models.iter_mut() {
        *model = fold_model(model, &mut remap)?;
    }
    for declaration in enums.iter_mut() {
        *declaration = fold_enum(declaration, &mut remap)?;
    }
    for external in externals.iter_mut() {
        *external = fold_external(external, &mut remap)?;
    }
    Ok(())
}

/// Fold each value of an ordered table, keeping the first refusal.
fn collect<T, R>(values: &[T], mut fold: impl FnMut(&T) -> Result<R, String>) -> Result<Vec<R>, String> {
    values.iter().map(&mut fold).collect()
}

/// Fold the carrier positions in an enum's payloads and closed SDK arguments.
fn fold_enum(declaration: &EnumDeclaration, remap: &mut NominalRemap) -> Result<EnumDeclaration, String> {
    let mut variants = Vec::with_capacity(declaration.variants.len());
    for variant in &declaration.variants {
        variants.push(EnumVariant {
            name: variant.name.clone(),
            fields: remap_types(&variant.fields, remap)?,
        });
    }
    let library_type_arguments = remap_types(&declaration.library_type_arguments, remap)?;
    Ok(EnumDeclaration {
        variants,
        library_type_arguments,
        ..declaration.clone()
    })
}

/// Fold the carrier positions in a model's fields, closed SDK arguments and checked bridge carriers.
fn fold_model(declaration: &ModelDeclaration, remap: &mut NominalRemap) -> Result<ModelDeclaration, String> {
    let mut kept = declaration.clone();
    kept.fields = collect(&declaration.fields, |field| fold_parameter(field, remap))?;
    kept.library_type_arguments = remap_types(&declaration.library_type_arguments, remap)?;
    kept.callable_signature = remap_types(&declaration.callable_signature, remap)?;
    if let Some(methods) = &declaration.iterator {
        kept.iterator = Some(IteratorMethods {
            item: remap_type(&methods.item, remap)?,
            next: methods.next.clone(),
        });
    }
    if let Some(methods) = &declaration.fallible_iterator {
        kept.fallible_iterator = Some(FallibleIteratorMethods {
            item: remap_type(&methods.item, remap)?,
            error: remap_type(&methods.error, remap)?,
            next: methods.next.clone(),
        });
    }
    Ok(kept)
}

/// Fold the carrier positions in a dependency signature.
fn fold_external(external: &ExternalFunction, remap: &mut NominalRemap) -> Result<ExternalFunction, String> {
    Ok(ExternalFunction {
        path: external.path.clone(),
        type_arguments: remap_types(&external.type_arguments, remap)?,
        parameters: remap_types(&external.parameters, remap)?,
        return_type: remap_type(&external.return_type, remap)?,
        span: external.span.clone(),
    })
}

/// Fold the concrete carrier positions in a published model template.
fn fold_generic_model(
    declaration: &GenericModelDeclaration,
    remap: &mut NominalRemap,
) -> Result<GenericModelDeclaration, String> {
    let mut fields = Vec::with_capacity(declaration.fields.len());
    for field in &declaration.fields {
        fields.push(GenericLayoutField {
            name: field.name.clone(),
            ty: fold_layout(&field.ty, remap)?,
            public: field.public,
        });
    }
    Ok(GenericModelDeclaration {
        fields,
        ..declaration.clone()
    })
}

/// Fold the concrete carrier positions in a published enum template.
fn fold_generic_enum(
    declaration: &GenericEnumDeclaration,
    remap: &mut NominalRemap,
) -> Result<GenericEnumDeclaration, String> {
    let mut variants = Vec::with_capacity(declaration.variants.len());
    for variant in &declaration.variants {
        variants.push(GenericEnumVariant {
            name: variant.name.clone(),
            fields: collect(&variant.fields, |ty| fold_layout(ty, remap))?,
        });
    }
    Ok(GenericEnumDeclaration {
        variants,
        ..declaration.clone()
    })
}

/// Fold the concrete types inside a template layout type.
fn fold_layout(ty: &GenericLayoutType, remap: &mut NominalRemap) -> Result<GenericLayoutType, String> {
    Ok(match ty {
        GenericLayoutType::Concrete(concrete) => GenericLayoutType::Concrete(remap_type(concrete, remap)?),
        GenericLayoutType::Applied(name, arguments) => GenericLayoutType::Applied(
            name.clone(),
            collect(arguments, |argument| fold_layout(argument, remap))?,
        ),
        GenericLayoutType::Tuple(elements) => {
            GenericLayoutType::Tuple(collect(elements, |element| fold_layout(element, remap))?)
        }
        GenericLayoutType::FunctionPointer(types) => {
            GenericLayoutType::FunctionPointer(collect(types, |element| fold_layout(element, remap))?)
        }
        GenericLayoutType::Parameter(_) => ty.clone(),
    })
}

/// Fold one typed parameter or field.
fn fold_parameter(parameter: &Parameter, remap: &mut NominalRemap) -> Result<Parameter, String> {
    Ok(Parameter {
        name: parameter.name.clone(),
        ty: remap_type(&parameter.ty, remap)?,
        span: parameter.span.clone(),
    })
}

/// Fold every carrier position in a function's signature, locals, blocks, loans and future callee.
fn fold_function(function: &Function, remap: &mut NominalRemap) -> Result<Function, String> {
    let mut kept = function.clone();
    kept.parameters = collect(&function.parameters, |parameter| fold_parameter(parameter, remap))?;
    kept.return_type = remap_type(&function.return_type, remap)?;
    kept.locals = collect(&function.locals, |local| {
        Ok(Local {
            ty: remap_type(&local.ty, remap)?,
            span: local.span.clone(),
        })
    })?;
    kept.blocks = collect(&function.blocks, |block| fold_block(block, remap))?;
    for loan in &mut kept.generic_loans {
        loan.ty = remap_type(&loan.ty, remap)?;
    }
    if let Some(callee) = &function.future_callee {
        kept.future_callee = Some(fold_callee(callee, remap)?);
    }
    Ok(kept)
}

/// Fold one block's statements and terminator.
fn fold_block(block: &BasicBlock, remap: &mut NominalRemap) -> Result<BasicBlock, String> {
    let mut statements = Vec::with_capacity(block.statements.len());
    for statement in &block.statements {
        match &statement.kind {
            StatementKind::Assign(target, value) => statements.push(Statement {
                kind: StatementKind::Assign(
                    fold_place(target, remap)?,
                    Rvalue {
                        kind: fold_rvalue(&value.kind, remap)?,
                        span: value.span.clone(),
                    },
                ),
                span: statement.span.clone(),
            }),
            _ => statements.push(statement.clone()),
        }
    }
    let terminator = Terminator {
        kind: fold_terminator(&block.terminator.kind, remap)?,
        span: block.terminator.span.clone(),
    };
    Ok(BasicBlock {
        statements,
        terminator,
        cleanup: block.cleanup,
        span: block.span.clone(),
    })
}

/// Fold the places, operands and callee a terminator holds.
fn fold_terminator(kind: &TerminatorKind, remap: &mut NominalRemap) -> Result<TerminatorKind, String> {
    Ok(match kind {
        TerminatorKind::SwitchBool(condition, otherwise, then) => {
            TerminatorKind::SwitchBool(fold_operand(condition, remap)?, *otherwise, *then)
        }
        TerminatorKind::Call(callee, arguments, destination, target, unwind) => TerminatorKind::Call(
            fold_callee(callee, remap)?,
            fold_operands(arguments, remap)?,
            fold_place(destination, remap)?,
            *target,
            unwind.clone(),
        ),
        TerminatorKind::Drop(dropped, target, unwind) => {
            TerminatorKind::Drop(fold_place(dropped, remap)?, *target, unwind.clone())
        }
        TerminatorKind::AssertOverflow(flag, operation, left, right, target, unwind) => TerminatorKind::AssertOverflow(
            fold_operand(flag, remap)?,
            operation.clone(),
            fold_operand(left, remap)?,
            fold_operand(right, remap)?,
            *target,
            unwind.clone(),
        ),
        TerminatorKind::Await(future, output, resume, drop, unwind, cancel) => TerminatorKind::Await(
            fold_place(future, remap)?,
            fold_place(output, remap)?,
            *resume,
            *drop,
            unwind.clone(),
            cancel.clone(),
        ),
        _ => kind.clone(),
    })
}

/// Fold a callee's enum owner, type arguments, generator leaves, operand and foreign signature.
fn fold_callee(callee: &Callee, remap: &mut NominalRemap) -> Result<Callee, String> {
    let kind = match &callee.kind {
        CalleeKind::CloneEnum(owner, name) => CalleeKind::CloneEnum(fold_enum_owner(*owner, remap)?, name.clone()),
        CalleeKind::Instantiated(path, types) => CalleeKind::Instantiated(path.clone(), remap_types(types, remap)?),
        CalleeKind::FutureMethod(path, types) => CalleeKind::FutureMethod(path.clone(), remap_types(types, remap)?),
        CalleeKind::SpawnGenerator(name, leaf, depth) => {
            CalleeKind::SpawnGenerator(name.clone(), remap_leaf(leaf, remap)?, *depth)
        }
        CalleeKind::YieldGenerator(leaf, depth) => CalleeKind::YieldGenerator(remap_leaf(leaf, remap)?, *depth),
        CalleeKind::CollectGenerator(leaf, depth) => CalleeKind::CollectGenerator(remap_leaf(leaf, remap)?, *depth),
        CalleeKind::Value(operand) => CalleeKind::Value(fold_operand(operand, remap)?),
        CalleeKind::SharedCallableBody(types) => CalleeKind::SharedCallableBody(remap_types(types, remap)?),
        CalleeKind::CallClosure(types) => CalleeKind::CallClosure(remap_types(types, remap)?),
        CalleeKind::CallClosureOnce(types) => CalleeKind::CallClosureOnce(remap_types(types, remap)?),
        CalleeKind::CallBorrowingClosure(types) => CalleeKind::CallBorrowingClosure(remap_types(types, remap)?),
        CalleeKind::CloneClosure(types) => CalleeKind::CloneClosure(remap_types(types, remap)?),
        CalleeKind::FormatValue(ty, debug) => CalleeKind::FormatValue(remap_type(ty, remap)?, *debug),
        CalleeKind::InstallPanicHook(types) => CalleeKind::InstallPanicHook(remap_types(types, remap)?),
        CalleeKind::Foreign(foreign) => {
            let mut kept = foreign.clone();
            kept.parameters = remap_types(&foreign.parameters, remap)?;
            kept.return_type = remap_type(&foreign.return_type, remap)?;
            CalleeKind::Foreign(kept)
        }
        _ => return Ok(callee.clone()),
    };
    Ok(Callee {
        kind,
        span: callee.span.clone(),
    })
}

/// Fold an rvalue's enum owner, types, operands and places.
fn fold_rvalue(kind: &RvalueKind, remap: &mut NominalRemap) -> Result<RvalueKind, String> {
    Ok(match kind {
        RvalueKind::Use(operand) => RvalueKind::Use(fold_operand(operand, remap)?),
        RvalueKind::NativeAlias(operand, source, target) => RvalueKind::NativeAlias(
            fold_operand(operand, remap)?,
            remap_type(source, remap)?,
            remap_type(target, remap)?,
        ),
        RvalueKind::FrozenText(operand) => RvalueKind::FrozenText(fold_operand(operand, remap)?),
        RvalueKind::FrozenCollection(ty, values) => RvalueKind::FrozenCollection(
            remap_type(ty, remap)?,
            collect(values, |value| fold_frozen(value, remap))?,
        ),
        RvalueKind::Binary(operation, left, right) => RvalueKind::Binary(
            operation.clone(),
            fold_operand(left, remap)?,
            fold_operand(right, remap)?,
        ),
        RvalueKind::Unary(operation, operand) => RvalueKind::Unary(operation.clone(), fold_operand(operand, remap)?),
        RvalueKind::NumericCast(operand, source, target) => RvalueKind::NumericCast(
            fold_operand(operand, remap)?,
            remap_type(source, remap)?,
            remap_type(target, remap)?,
        ),
        RvalueKind::IntToFloat(operand) => RvalueKind::IntToFloat(fold_operand(operand, remap)?),
        RvalueKind::FloatToInt(operand) => RvalueKind::FloatToInt(fold_operand(operand, remap)?),
        RvalueKind::BoolToInt(operand) => RvalueKind::BoolToInt(fold_operand(operand, remap)?),
        RvalueKind::Array(operands) => RvalueKind::Array(fold_operands(operands, remap)?),
        RvalueKind::Enum(owner, variant, operands) => RvalueKind::Enum(
            fold_enum_owner(*owner, remap)?,
            *variant,
            fold_operands(operands, remap)?,
        ),
        RvalueKind::Discriminant(target) => RvalueKind::Discriminant(fold_place(target, remap)?),
        RvalueKind::TagToInt(operand) => RvalueKind::TagToInt(fold_operand(operand, remap)?),
        RvalueKind::Model(owner, operands) => RvalueKind::Model(*owner, fold_operands(operands, remap)?),
        RvalueKind::Tuple(operands) => RvalueKind::Tuple(fold_operands(operands, remap)?),
        RvalueKind::Range(operands) => RvalueKind::Range(fold_operands(operands, remap)?),
        _ => return fold_reference_rvalue(kind, remap),
    })
}

/// Fold the borrowing, callable and pointer rvalues.
fn fold_reference_rvalue(kind: &RvalueKind, remap: &mut NominalRemap) -> Result<RvalueKind, String> {
    Ok(match kind {
        RvalueKind::Borrow(target) => RvalueKind::Borrow(fold_place(target, remap)?),
        RvalueKind::MutBorrow(target) => RvalueKind::MutBorrow(fold_place(target, remap)?),
        RvalueKind::UnsizeSlice(operand) => RvalueKind::UnsizeSlice(fold_operand(operand, remap)?),
        RvalueKind::UnsizeTraitObject(operand, source, target) => RvalueKind::UnsizeTraitObject(
            fold_operand(operand, remap)?,
            remap_type(source, remap)?,
            remap_type(target, remap)?,
        ),
        RvalueKind::FunctionItem(callee) => RvalueKind::FunctionItem(fold_callee(callee, remap)?),
        RvalueKind::ReifyFunction(operand) => RvalueKind::ReifyFunction(fold_operand(operand, remap)?),
        RvalueKind::FunctionAddress(operand) => RvalueKind::FunctionAddress(fold_operand(operand, remap)?),
        RvalueKind::ClosureObject(target) => RvalueKind::ClosureObject(fold_place(target, remap)?),
        RvalueKind::IsVariant(target, variant) => RvalueKind::IsVariant(fold_place(target, remap)?, *variant),
        RvalueKind::RawAddressOf(target, mutable) => RvalueKind::RawAddressOf(fold_place(target, remap)?, *mutable),
        RvalueKind::PointerCast(operand, ty) => {
            RvalueKind::PointerCast(fold_operand(operand, remap)?, remap_type(ty, remap)?)
        }
        RvalueKind::PointerOffset(pointer, offset) => {
            RvalueKind::PointerOffset(fold_operand(pointer, remap)?, fold_operand(offset, remap)?)
        }
        RvalueKind::PointerAddress(operand) => RvalueKind::PointerAddress(fold_operand(operand, remap)?),
        // The unit function name and frozen bytes name no type.
        _ => kind.clone(),
    })
}

/// Fold a static value's constants and nested collection types.
fn fold_frozen(value: &FrozenValue, remap: &mut NominalRemap) -> Result<FrozenValue, String> {
    Ok(match value {
        FrozenValue::Scalar(constant) => FrozenValue::Scalar(fold_constant(constant, remap)?),
        FrozenValue::Collection(ty, values) => FrozenValue::Collection(
            remap_type(ty, remap)?,
            collect(values, |nested| fold_frozen(nested, remap))?,
        ),
        FrozenValue::Bytes(_) => value.clone(),
    })
}

/// Fold an ordered operand list.
fn fold_operands(operands: &[Operand], remap: &mut NominalRemap) -> Result<Vec<Operand>, String> {
    collect(operands, |operand| fold_operand(operand, remap))
}

/// Fold an operand's place or constant type.
fn fold_operand(operand: &Operand, remap: &mut NominalRemap) -> Result<Operand, String> {
    let kind = match &operand.kind {
        OperandKind::Copy(target) => OperandKind::Copy(fold_place(target, remap)?),
        OperandKind::Move(target) => OperandKind::Move(fold_place(target, remap)?),
        OperandKind::Literal(constant) => OperandKind::Literal(fold_constant(constant, remap)?),
    };
    Ok(Operand {
        kind,
        span: operand.span.clone(),
    })
}

/// Fold a typed constant's carrier.
fn fold_constant(constant: &Constant, remap: &mut NominalRemap) -> Result<Constant, String> {
    Ok(match constant {
        Constant::Numeric(digits, ty) => Constant::Numeric(digits.clone(), remap_type(ty, remap)?),
        Constant::RustItem(path, ty) => Constant::RustItem(path.clone(), remap_type(ty, remap)?),
        Constant::TypeToken(types) => Constant::TypeToken(remap_types(types, remap)?),
        _ => constant.clone(),
    })
}

/// Fold the types a place's projection names.
fn fold_place(target: &Place, remap: &mut NominalRemap) -> Result<Place, String> {
    Ok(Place {
        local: target.local,
        projection: fold_projection(&target.projection, remap)?,
        span: target.span.clone(),
    })
}

/// Fold each typed projection step.
fn fold_projection(projection: &Projection, remap: &mut NominalRemap) -> Result<Projection, String> {
    Ok(match projection {
        Projection::NumericValue(ty) => Projection::NumericValue(remap_type(ty, remap)?),
        Projection::VariantField(variant, slot, ty) => {
            Projection::VariantField(*variant, *slot, remap_type(ty, remap)?)
        }
        Projection::Field(slot, ty) => Projection::Field(*slot, remap_type(ty, remap)?),
        Projection::Deref(ty) => Projection::Deref(remap_type(ty, remap)?),
        Projection::DerefField(slot, ty) => Projection::DerefField(*slot, remap_type(ty, remap)?),
        Projection::Fields(steps) => Projection::Fields(fold_steps(steps, remap)?),
        Projection::DerefFields(steps) => Projection::DerefFields(fold_steps(steps, remap)?),
        _ => projection.clone(),
    })
}

/// Fold each step of a canonical field path.
fn fold_steps(steps: &[FieldProjection], remap: &mut NominalRemap) -> Result<Vec<FieldProjection>, String> {
    collect(steps, |step| {
        Ok(FieldProjection {
            ty: remap_type(&step.ty, remap)?,
            ..step.clone()
        })
    })
}

/// Fold one enum owner position.
fn fold_enum_owner(owner: i64, remap: &mut NominalRemap) -> Result<i64, String> {
    Ok(remap.slot(NominalKind::Enum, owner, "")?.index)
}
