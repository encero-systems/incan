//! Concrete standard Option and Result layouts shared with native enum operations.

use std::collections::HashSet;

use incan_lang::lang::surface::constructors::{ConstructorId, from_str as builtin_constructor};
use incan_lang::lang::surface::option_methods::OptionMethodId;
use incan_lang::lang::surface::result_methods::ResultMethodId;
use incan_lang::lang::types::collections::{CollectionTypeId, from_str as collection_type};
use incan_mir_plan_native::{
    CalleeKind, Carrier, Constant, EnumDeclaration, EnumVariant, ExternalFunction, Operand as PlanOperand, OperandKind,
    Place, PlanType, Projection, RvalueKind, SourceSpan, TerminatorKind, Unwind, assign as plan_assign,
    carrier_position, place as plan_place, source_span,
};
use incan_semantics_core::body_ir::{
    Block, Body, BodyIrModule, CallableTarget, Callee, MatchArm, NamedCallableTarget, Operand, Rvalue, StatementKind,
    StaticInitializer,
};
use incan_semantics_core::body_ir::{Constant as SourceConstant, Pattern, ResultVariantKind};
use incan_semantics_core::{IncanType, SemanticSourceTargetKind, SymbolOrigin, generic};

use crate::carrier_operands::boundary_plan_operand;
use crate::compiler_record_adapters::{authored_plan_checked_type, semantic_key_matches_native};
use crate::enums::validate_payload_patterns;
use crate::formatting::display_plan_value;
use crate::frame::{
    Builder, cleanup_exit, drop_all_owned, drop_owned, forget_owned, place_type, read_place, temporary, terminate_call,
    track_rvalue_moves, variant_payload_place,
};
use crate::layout_types::native_source_type;
use crate::lists::taking_iterator_locals;
use crate::native_type_identity::{ConstructorIndex, native_owner};
use crate::nominal_frames::{closed_arguments, closed_type, declaration_storage};
use crate::numerics::numeric_carrier;
use crate::text_plan::clone_text;
use crate::tuples::{is_tuple_type, tuple_fields, tuple_has_owners};
use crate::type_parameters::substitute;
use crate::unions::lower_union_declaration;

/// Retain concrete carriers after any layouts whose indices an earlier lowering already published.
pub fn intrinsic_declarations(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    file: &str,
    retained: Vec<EnumDeclaration>,
) -> Result<Vec<EnumDeclaration>, String> {
    let mut declarations = retained;
    let mut pass = CarrierPass::new(constructors, module, &mut declarations, file);
    for ty in declaration_storage(module) {
        if closed_type(&ty) {
            pass.retain(&ty)?;
        }
    }
    for delegation in &module.loaf_callables {
        if !delegation.type_parameters.is_empty() || !closed_arguments(&delegation.native_type_arguments) {
            continue;
        }
        for parameter in &delegation.parameters {
            if closed_type(parameter) {
                pass.retain(parameter)?;
            }
        }
        // A checked native instantiation may still contain its enclosing source function's binders. Its closed
        // function frame schedules those carriers; the declaration template does not authorize an open layout.
        if closed_type(&delegation.return_type) {
            pass.retain(&delegation.return_type)?;
        }
    }
    for declaration in &module.nominal_declarations {
        for ty in &declaration.field_types {
            // A Rust layout keeps a field without a carrier opaque; that field grants no read, write or construction.
            if native_owner(declaration) {
                let _ = pass.retain(ty);
            } else {
                pass.retain(ty)?;
            }
        }
    }
    for declaration in &module.enum_declarations {
        for variant in &declaration.variants {
            for ty in &variant.fields {
                pass.retain(ty)?;
            }
        }
    }
    for body in &module.bodies {
        if !body.type_parameters.is_empty() {
            continue;
        }
        pass.retain_body(body)?;
    }
    for declaration in &module.static_declarations {
        pass.retain(&declaration.ty)?;
        if let StaticInitializer::Computed(body) = &declaration.initial {
            pass.retain_body(body)?;
        }
    }
    Ok(declarations)
}

/// Append closed frame and poll carriers without renumbering layouts; refusal names its declaring frame position.
pub fn retain_body_carriers(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut Vec<EnumDeclaration>,
    body: &Body,
    file: &str,
) -> Result<(), String> {
    CarrierPass::new(constructors, module, declarations, file).retain_body(body)
}

/// One carrier inventory pass over a module context: the carriers it appends and the closed types it has retained.
///
/// A pass only appends carriers. Once [`retain_carrier`] has accepted a type, every later call for an equal type finds
/// that type's carriers and changes nothing, so the pass skips the call instead of comparing the type with every
/// carrier again; a whole-program inventory names the same types in thousands of frames. A refused type is not
/// remembered and refuses again.
struct CarrierPass<'a> {
    constructors: &'a ConstructorIndex,
    module: &'a BodyIrModule,
    declarations: &'a mut Vec<EnumDeclaration>,
    file: &'a str,
    retained: HashSet<IncanType>,
}

impl<'a> CarrierPass<'a> {
    fn new(
        constructors: &'a ConstructorIndex,
        module: &'a BodyIrModule,
        declarations: &'a mut Vec<EnumDeclaration>,
        file: &'a str,
    ) -> Self {
        Self {
            constructors,
            module,
            declarations,
            file,
            retained: HashSet::new(),
        }
    }

    /// Retain one closed type's carriers, once per pass.
    fn retain(&mut self, ty: &IncanType) -> Result<(), String> {
        if self.retained.contains(ty) {
            return Ok(());
        }
        retain_carrier(self.constructors, self.module, self.declarations, ty, self.file)?;
        self.retained.insert(ty.clone());
        Ok(())
    }

    /// Retain a frame's return, local and consuming-iteration carriers; a refusal names the frame and its position.
    fn retain_body(&mut self, body: &Body) -> Result<(), String> {
        self.retain(&body.return_type)
            .map_err(|reason| format!("{reason} in {} return carrier", body.name))?;
        for (index, local) in body.locals.iter().enumerate() {
            self.retain(&local.ty)
                .map_err(|reason| format!("{reason} in {} local {index} carrier", body.name))?;
        }
        let taking = taking_iterator_locals(&body.block);
        self.retain_iteration_polls(body, &body.block, &taking)
            .map_err(|reason| format!("{reason} in {} iteration carriers", body.name))
    }

    /// Retain Iterator::next's Option output from a checked consuming loop's concrete item type before frame lowering.
    fn retain_iteration_polls(&mut self, body: &Body, block: &Block, taking: &[i64]) -> Result<(), String> {
        for statement in &block.stmts {
            self.retain_poll_statement(body, &statement.kind, taking)?;
        }
        Ok(())
    }

    /// Retain the carriers one statement's calls and consuming iterations need, descending into nested blocks.
    fn retain_poll_statement(&mut self, body: &Body, kind: &StatementKind, taking: &[i64]) -> Result<(), String> {
        match kind {
            StatementKind::Call {
                callee: Callee::Function(CallableTarget::Named(target)),
                ..
            } => {
                self.retain_call(target)?;
            }
            StatementKind::IterNext {
                destination, iterator, ..
            } => {
                if let Operand::Place(read) = iterator
                    && let Some(local) = read.place.local_id()
                {
                    let index = local.index() as i64 + 1;
                    if taking.contains(&index) {
                        let Some(item) = destination.local_id() else {
                            return Err("unsupported Body IR consuming iteration global item".into());
                        };
                        let ty = body.locals[item.index()].ty.clone();
                        let output = generic("Option".into(), vec![ty]);
                        self.retain(&output)?;
                    }
                }
            }
            StatementKind::Assign {
                rvalue: Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    self.retain_poll_arm(body, arm, taking)?;
                }
            }
            StatementKind::Race { arms, .. } => {
                for arm in arms {
                    self.retain_iteration_polls(body, &arm.body, taking)?;
                }
            }
            StatementKind::Loop { body: child } | StatementKind::UnsafeRegion { body: child, .. } => {
                self.retain_iteration_polls(body, child, taking)?;
            }
            StatementKind::If {
                then_block, else_block, ..
            } => {
                self.retain_iteration_polls(body, then_block, taking)?;
                if let Some(child) = else_block {
                    self.retain_iteration_polls(body, child, taking)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Retain the carriers of a match arm's guard and body statements.
    fn retain_poll_arm(&mut self, body: &Body, arm: &MatchArm, taking: &[i64]) -> Result<(), String> {
        for statement in arm.guard_stmts.iter().chain(&arm.body_stmts) {
            self.retain_poll_statement(body, &statement.kind, taking)?;
        }
        Ok(())
    }

    /// Retain closed source and published free-function signatures, including carriers absent from locals.
    fn retain_call(&mut self, target: &NamedCallableTarget) -> Result<(), String> {
        match &target.canonical {
            Some(identity) if identity.kind != SemanticSourceTargetKind::Function => return Ok(()),
            None => return Ok(()),
            _ => {}
        }
        if !closed_arguments(&target.type_args) {
            return Ok(());
        }
        let module = self.module;
        for candidate in &module.bodies {
            if target.canonical != candidate.canonical || candidate.type_parameters.len() != target.type_args.len() {
                continue;
            }
            let parameters: Vec<IncanType> = candidate.params.iter().map(|parameter| parameter.ty.clone()).collect();
            self.retain_closed_signature(
                &parameters,
                &candidate.return_type,
                &candidate.type_parameters,
                &target.type_args,
            )?;
        }
        for delegation in &module.loaf_callables {
            if target.canonical.as_ref() != Some(&delegation.canonical) {
                continue;
            }
            if target.type_args.len() != delegation.type_parameters.len() {
                return Err("the standard library carrier type argument count is unsupported".into());
            }
            self.retain_closed_signature(
                &delegation.parameters,
                &delegation.return_type,
                &delegation.type_parameters,
                &target.type_args,
            )?;
        }
        Ok(())
    }

    /// Close a retained callable's signature from checked type arguments before any operand needs its carrier.
    fn retain_closed_signature(
        &mut self,
        parameters: &[IncanType],
        result: &IncanType,
        names: &[String],
        arguments: &[IncanType],
    ) -> Result<(), String> {
        for parameter in parameters {
            let instantiated = substitute(parameter, names, arguments)?;
            self.retain(&instantiated)?;
        }
        let closed_result = substitute(result, names, arguments)?;
        self.retain(&closed_result)
    }
}

/// Retain one concrete standard layout once; nested carriers precede their containing carrier.
pub fn retain_carrier(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    declarations: &mut Vec<EnumDeclaration>,
    ty: &IncanType,
    file: &str,
) -> Result<(), String> {
    match ty {
        IncanType::Tuple(elements) => {
            // A carrier held only as a tuple element still needs its own layout.
            for element in elements {
                retain_carrier(constructors, module, declarations, element, file)?;
            }
        }
        IncanType::Generic { base, args } => {
            for argument in args {
                retain_carrier(constructors, module, declarations, argument, file)?;
            }
            let kind = collection_type(base);
            let is_option = kind == Some(CollectionTypeId::Option);
            let is_result = kind == Some(CollectionTypeId::Result);
            if !is_option && !is_result && !ty.is_union() {
                return Ok(());
            }
            for retained in declarations.iter() {
                if semantic_key_matches_native(&retained.semantic_key, ty)? {
                    return Ok(());
                }
            }
            if ty.is_union() {
                let declaration = lower_union_declaration(constructors, module, ty, file, declarations)?;
                declarations.push(declaration);
                return Ok(());
            }
            if (is_option && args.len() != 1) || (is_result && args.len() != 2) {
                return Err(format!("unsupported Body IR type {ty}"));
            }
            let mut fields = Vec::with_capacity(args.len());
            for (index, argument) in args.iter().enumerate() {
                let field = native_source_type(
                    constructors,
                    &module.nominal_declarations,
                    &module.enum_declarations,
                    &[],
                    argument,
                    declarations,
                    "",
                )
                .map_err(|reason| format!("{reason} in {base} payload {index} of {ty}"))?;
                fields.push(field);
            }
            let (variants, carrier) = if is_result {
                (
                    vec![
                        EnumVariant {
                            name: "Ok".into(),
                            fields: vec![fields[0].clone()],
                        },
                        EnumVariant {
                            name: "Err".into(),
                            fields: vec![fields[1].clone()],
                        },
                    ],
                    Carrier::Result,
                )
            } else {
                (
                    vec![
                        EnumVariant {
                            name: "None".into(),
                            fields: Vec::new(),
                        },
                        EnumVariant {
                            name: "Some".into(),
                            fields: vec![fields[0].clone()],
                        },
                    ],
                    Carrier::Option,
                )
            };
            let semantic_key =
                authored_plan_checked_type(ty, file, &constructors.source_files, &constructors.package_versions)?;
            let mut declaration = EnumDeclaration::new(
                format!("__IncanCarrier{}", declarations.len()),
                false,
                variants,
                vec!["Clone".into()],
                source_span(file, 1, 1),
            );
            declaration.carrier = carrier;
            declaration.semantic_key = Some(semantic_key);
            declaration.source_type = ty.to_string();
            declarations.push(declaration);
        }
        IncanType::Function { params, return_type } => {
            for parameter in params {
                retain_carrier(constructors, module, declarations, &parameter.ty, file)?;
            }
            retain_carrier(constructors, module, declarations, return_type, file)?;
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) => {
            retain_carrier(constructors, module, declarations, inner, file)?;
        }
        _ => {}
    }
    Ok(())
}

/// Evaluate an owned replacement before dropping its old field, keeping the enclosing owner initialized.
pub fn replace_owned_destination(builder: &mut Builder, target: Place, operation: RvalueKind, span: &SourceSpan) {
    if target.projection == Projection::Whole {
        track_rvalue_moves(builder, &operation);
        drop_owned(builder, target.local, span);
        let local = target.local;
        builder.emit(plan_assign(target, operation, span));
        builder.owned.push(local);
        return;
    }
    let replacement = temporary(builder, place_type(&builder.locals, &target), span);
    track_rvalue_moves(builder, &operation);
    builder.emit(plan_assign(replacement.clone(), operation, span));
    builder.owned.push(replacement.local);
    let continuation = builder.new_block(span);
    let cleanup = cleanup_exit(builder, span);
    builder.finish(
        TerminatorKind::Drop(target.clone(), continuation, Unwind::Cleanup(cleanup)),
        span,
    );
    builder.select(continuation);
    forget_owned(builder, replacement.local);
    builder.emit(plan_assign(
        target,
        RvalueKind::Use(read_place(replacement, true, span)),
        span,
    ));
}

/// Copy primitive, static frozen, function-pointer and immutable scalar-reference payloads, or clone an owned active
/// payload.
pub fn carrier_payload_value(builder: &mut Builder, place: Place, span: &SourceSpan) -> Result<PlanOperand, String> {
    let ty = place_type(&builder.locals, &place);
    if numeric_carrier(&ty) || matches!(ty, PlanType::Bool | PlanType::Unit) {
        return Ok(read_place(place, false, span));
    }
    match &ty {
        PlanType::FrozenStr
        | PlanType::FrozenBytes
        | PlanType::FunctionPointer(_)
        | PlanType::ScalarRef(_)
        | PlanType::StringRef
        | PlanType::ListRef(..)
        | PlanType::SetRef(_)
        | PlanType::DictRef(..)
        | PlanType::TupleRef(_)
        | PlanType::ModelRef(..)
        | PlanType::EnumRef(..)
        | PlanType::ValidationErrorRef => return Ok(read_place(place, false, span)),
        PlanType::Tuple(_) => return clone_tuple_operand(builder, place, span),
        _ => {}
    }
    let reference_type = match &ty {
        PlanType::String => PlanType::StringRef,
        PlanType::ValidationError => PlanType::ValidationErrorRef,
        PlanType::List(leaf, depth) => PlanType::ListRef(leaf.clone(), *depth),
        PlanType::Set(leaf) => PlanType::SetRef(leaf.clone()),
        PlanType::Dict(key, value) => PlanType::DictRef(key.clone(), value.clone()),
        PlanType::Enum(owner, name) => PlanType::EnumRef(*owner, name.clone()),
        PlanType::Model(owner, name) => PlanType::ModelRef(*owner, name.clone()),
        PlanType::FrozenCollection(name, elements) => PlanType::FrozenCollectionRef(name.clone(), elements.clone()),
        _ => {
            return Err(format!(
                "unsupported Body IR intrinsic payload clone type {ty:?} in {} at {}",
                builder.name, span.line
            ));
        }
    };
    let output = borrowed_native_value(
        builder,
        "core::clone::Clone::clone",
        place,
        ty.clone(),
        reference_type,
        ty,
        span,
    );
    Ok(read_place(output, true, span))
}

/// Snapshot tuple elements without partially moving an owned tuple, cloning each owned element in place.
pub fn clone_tuple_operand(builder: &mut Builder, value: Place, span: &SourceSpan) -> Result<PlanOperand, String> {
    let ty = place_type(&builder.locals, &value);
    let mut operands = Vec::new();
    for field in tuple_fields(&value, &ty, span)? {
        let field_type = place_type(&builder.locals, &field);
        if field_type == PlanType::String {
            operands.push(read_place(clone_text(builder, field, span)?, true, span));
        } else if is_tuple_type(&field_type) {
            operands.push(clone_tuple_operand(builder, field, span)?);
        } else {
            // Scalars are copied; an owned list or enum carrier is cloned through its real Clone implementation.
            operands.push(carrier_payload_value(builder, field, span)?);
        }
    }
    let result = temporary(builder, ty.clone(), span);
    let operation = RvalueKind::Tuple(operands);
    track_rvalue_moves(builder, &operation);
    builder.emit(plan_assign(result.clone(), operation, span));
    let owned = tuple_has_owners(&ty);
    if owned {
        builder.owned.push(result.local);
    }
    Ok(read_place(result, true, span))
}

/// Instantiate one native callee against a checked carrier or payload type and its shared reference.
pub fn borrowed_native_value(
    builder: &mut Builder,
    path: &str,
    source: Place,
    ty: PlanType,
    reference_type: PlanType,
    result_type: PlanType,
    span: &SourceSpan,
) -> Place {
    let reference = temporary(builder, reference_type.clone(), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(source), span));
    let output = temporary(builder, result_type.clone(), span);
    builder.externals.push(ExternalFunction {
        path: path.to_string(),
        type_arguments: vec![ty.clone()],
        parameters: vec![reference_type],
        return_type: result_type,
        span: span.clone(),
    });
    terminate_call(
        builder,
        CalleeKind::Instantiated(path.to_string(), vec![ty]),
        vec![read_place(reference, false, span)],
        output.clone(),
        span,
    );
    builder.owned.push(output.local);
    output
}

/// Match intrinsic Result facts; ordinary source constructor spellings never authorize a standard carrier.
pub fn intrinsic_pattern(declaration: &EnumDeclaration, pattern: &Pattern) -> Result<i64, String> {
    match pattern {
        Pattern::Wildcard => return Ok(-1),
        Pattern::Literal(value) => {
            if declaration.carrier == Carrier::Option && *value == SourceConstant::None {
                return Ok(0);
            }
        }
        Pattern::Enum {
            canonical: Some(canonical),
            fields,
            ..
        } => {
            // The checked identity names the builtin constructor; its written spelling decides nothing.
            if declaration.carrier == Carrier::Option && canonical.origin == SymbolOrigin::Builtin {
                let constructor = builtin_constructor(&canonical.declaration_name);
                if constructor == Some(ConstructorId::Some) && fields.len() == 1 {
                    validate_payload_patterns(fields)?;
                    return Ok(1);
                }
                // A bare `None` constructor, as an `assert value is None` pattern spells it.
                if constructor == Some(ConstructorId::None) && fields.is_empty() {
                    return Ok(0);
                }
            }
        }
        Pattern::Result { variant, fields } => {
            if declaration.carrier != Carrier::Result || fields.len() != 1 {
                return Err("invalid Body IR Result pattern".into());
            }
            validate_payload_patterns(fields)?;
            if *variant == ResultVariantKind::Ok {
                return Ok(0);
            }
            return Ok(1);
        }
        _ => {}
    }
    Err("unsupported Body IR intrinsic carrier pattern".into())
}

/// Test a checked variant through the adapter's native discriminant comparison.
pub fn variant_condition(builder: &mut Builder, source: &Place, slot: i64, span: &SourceSpan) -> PlanOperand {
    let condition = temporary(builder, PlanType::Bool, span);
    builder.emit(plan_assign(
        condition.clone(),
        RvalueKind::IsVariant(source.clone(), slot),
        span,
    ));
    read_place(condition, false, span)
}

/// Require a retained concrete Result carrier before reading its variant payload layout.
pub fn result_layout(builder: &Builder, ty: &PlanType) -> Result<(i64, EnumDeclaration), String> {
    if let PlanType::Enum(owner, _) = ty
        && *owner >= builder.enums().len() as i64
    {
        let declaration = &builder.intrinsics[carrier_position(*owner) as usize];
        if declaration.carrier == Carrier::Result {
            return Ok((*owner, declaration.clone()));
        }
    }
    Err("unsupported Body IR TryPropagate outside Result".into())
}

/// Move the selected payload once; error returns drop every other initialized owner before returning. A `const` str
/// error becomes the owned text the enclosing `Result[T, str]` carries, as at any other owned text boundary; any other
/// differing error type is refused. A kept payload whose storage differs from the destination's, such as a native
/// `Option[list[u8]]` kept as `Option[bytes]`, converts at the destination as a pattern binding's payload does.
pub fn propagate_result(
    builder: &mut Builder,
    source: Place,
    destination: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let (_, input) = result_layout(builder, &place_type(&builder.locals, &source))?;
    let output_type = builder.locals[0].ty.clone();
    let (output_owner, output) = result_layout(builder, &output_type)?;
    let owns_text = input.variants[1].fields.as_slice() == [PlanType::FrozenStr]
        && output.variants[1].fields.as_slice() == [PlanType::String];
    if input.variants[1].fields != output.variants[1].fields && !owns_text {
        return Err("unsupported Body IR TryPropagate error conversion without retained conversion target".into());
    }
    let failure = builder.new_block(span);
    let success = builder.new_block(span);
    let condition = variant_condition(builder, &source, 0, span);
    builder.finish(TerminatorKind::SwitchBool(condition, failure, success), span);
    // The carrier has one active payload. Both edges move it, so neither drops the emptied carrier.
    forget_owned(builder, source.local);
    let owners = builder.owned.clone();
    builder.select(failure);
    let error_type = input.variants[1].fields[0].clone();
    let error = variant_payload_place(&source, 1, 0, error_type, span)?;
    let mut propagated = read_place(error.clone(), true, span);
    if owns_text {
        let text = display_plan_value(builder, read_place(error, false, span), span, false)?;
        propagated = read_place(text, true, span);
    }
    builder.emit(plan_assign(
        plan_place(0, span),
        RvalueKind::Enum(output_owner, 1, vec![propagated]),
        span,
    ));
    drop_all_owned(builder, span);
    builder.finish(TerminatorKind::Return, span);
    builder.select(success);
    builder.owned = owners;
    let value_type = input.variants[0].fields[0].clone();
    let value = variant_payload_place(&source, 0, 0, value_type.clone(), span)?;
    let local = destination.local;
    let destination_type = place_type(&builder.locals, &destination);
    let mut kept = read_place(value, true, span);
    if value_type != destination_type {
        kept = boundary_plan_operand(builder, kept, &destination_type, span)?;
        track_rvalue_moves(builder, &RvalueKind::Use(kept.clone()));
    }
    builder.emit(plan_assign(destination, RvalueKind::Use(kept), span));
    if matches!(
        destination_type,
        PlanType::ValidationError | PlanType::String | PlanType::Enum(..) | PlanType::Model(..) | PlanType::List(..)
    ) {
        builder.owned.push(local);
    }
    Ok(())
}

/// Resolve only a retained standard carrier, keeping source enums on their existing canonical path.
pub fn carrier_layout(builder: &Builder, ty: &PlanType) -> Result<(i64, EnumDeclaration), String> {
    if let PlanType::Enum(owner, _) = ty
        && *owner >= builder.enums().len() as i64
    {
        return Ok((*owner, builder.intrinsics[carrier_position(*owner) as usize].clone()));
    }
    Err("unsupported Body IR intrinsic carrier receiver".into())
}

/// Materialize None using the checked unit or concrete Option destination contract.
pub fn none_operand(builder: &mut Builder, ty: &PlanType, span: &SourceSpan) -> Result<PlanOperand, String> {
    if *ty == PlanType::Unit {
        return Ok(PlanOperand {
            kind: OperandKind::Literal(Constant::Unit),
            span: span.clone(),
        });
    }
    let (owner, declaration) = carrier_layout(builder, ty)?;
    if declaration.carrier != Carrier::Option {
        return Err("unsupported Body IR None outside Option".into());
    }
    let output = temporary(builder, ty.clone(), span);
    replace_owned_destination(builder, output.clone(), RvalueKind::Enum(owner, 0, Vec::new()), span);
    Ok(read_place(output, true, span))
}

/// Render a retained standard carrier through legacy Debug formatting, without moving the source.
pub fn format_carrier(builder: &mut Builder, source: Place, span: &SourceSpan) -> Result<Place, String> {
    let (owner, declaration) = carrier_layout(builder, &place_type(&builder.locals, &source))?;
    let ty = PlanType::Enum(owner, declaration.name.clone());
    Ok(borrowed_native_value(
        builder,
        "incan_native_runtime::format_carrier",
        source,
        ty,
        PlanType::EnumRef(owner, declaration.name),
        PlanType::String,
        span,
    ))
}

/// Transfer active payload owners before replacing and tracking one concrete carrier destination.
pub fn assign_carrier(
    builder: &mut Builder,
    target: Place,
    owner: i64,
    slot: i64,
    operands: Vec<PlanOperand>,
    span: &SourceSpan,
) {
    replace_owned_destination(builder, target, RvalueKind::Enum(owner, slot, operands), span);
}

/// Lower retained carrier predicates and eager defaults using the shared discriminant and payload machinery.
pub fn lower_carrier_method_plan(
    builder: &mut Builder,
    option_method: Option<OptionMethodId>,
    result_method: Option<ResultMethodId>,
    source: Place,
    target: Place,
    fallback: Option<PlanOperand>,
    span: &SourceSpan,
) -> Result<(), String> {
    let (_, declaration) = carrier_layout(builder, &place_type(&builder.locals, &source))?;
    let mut present = if declaration.carrier == Carrier::Option { 1 } else { 0 };
    let predicate = matches!(option_method, Some(OptionMethodId::IsSome | OptionMethodId::IsNone))
        || matches!(result_method, Some(ResultMethodId::IsOk | ResultMethodId::IsErr));
    if predicate {
        if option_method == Some(OptionMethodId::IsNone) || result_method == Some(ResultMethodId::IsErr) {
            present = 1 - present;
        }
        let condition = variant_condition(builder, &source, present, span);
        builder.emit(plan_assign(target, RvalueKind::Use(condition), span));
        return Ok(());
    }
    if option_method == Some(OptionMethodId::Unwrap) || result_method == Some(ResultMethodId::Unwrap) {
        return lower_carrier_unwrap(builder, &declaration, present, source, target, span);
    }
    if option_method != Some(OptionMethodId::UnwrapOr) && result_method != Some(ResultMethodId::UnwrapOr) {
        return Err("unsupported Body IR intrinsic carrier method".into());
    }
    match fallback {
        Some(value) => lower_carrier_default(builder, &declaration, present, (source, target), value, span),
        None => Err("unsupported Body IR carrier missing default".into()),
    }
}

/// Project a carrier's payload where the carrier lives, keeping the field or reference path that reached it.
fn carrier_payload_place(source: &Place, variant: i64, ty: PlanType, span: &SourceSpan) -> Result<Place, String> {
    variant_payload_place(source, variant, 0, ty, span)
}

/// Whether a carrier payload owns resources its destination must drop.
fn payload_owns(ty: &PlanType) -> bool {
    !numeric_carrier(ty) && *ty != PlanType::Bool && *ty != PlanType::Unit
}

/// Select the active borrowed payload, or fail through the runtime exactly as legacy's `unwrap` does.
fn lower_carrier_unwrap(
    builder: &mut Builder,
    declaration: &EnumDeclaration,
    present: i64,
    source: Place,
    target: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let payload_type = declaration.variants[present as usize].fields[0].clone();
    let selected = builder.new_block(span);
    let missing = builder.new_block(span);
    let incoming = builder.owned.clone();
    let condition = variant_condition(builder, &source, present, span);
    builder.finish(TerminatorKind::SwitchBool(condition, missing, selected), span);
    // `None` fails with the standard message; `Err` passes a clone of its payload so the message renders it as Debug.
    builder.select(missing);
    let unit = temporary(builder, PlanType::Unit, span);
    if declaration.carrier == Carrier::Option {
        unwrap_failure(builder, "unwrap_none", Vec::new(), Vec::new(), unit, span);
    } else {
        let error_type = declaration.variants[(1 - present) as usize].fields[0].clone();
        let error_place = carrier_payload_place(&source, 1 - present, error_type.clone(), span)?;
        let error = carrier_payload_value(builder, error_place, span)?;
        unwrap_failure(builder, "unwrap_err", vec![error_type], vec![error], unit, span);
    }
    builder.finish(TerminatorKind::Unreachable, span);
    builder.select(selected);
    builder.owned = incoming;
    let projected = carrier_payload_place(&source, present, payload_type.clone(), span)?;
    let payload = carrier_payload_value(builder, projected, span)?;
    let value = RvalueKind::Use(payload);
    track_rvalue_moves(builder, &value);
    let local = target.local;
    builder.emit(plan_assign(target, value, span));
    if payload_owns(&payload_type) {
        builder.owned.push(local);
    }
    Ok(())
}

/// Call the runtime failure for an absent carrier payload; its type argument is the `Err` payload's carrier.
fn unwrap_failure(
    builder: &mut Builder,
    name: &str,
    type_arguments: Vec<PlanType>,
    arguments: Vec<PlanOperand>,
    destination: Place,
    span: &SourceSpan,
) {
    let path = format!("incan_native_runtime::{name}");
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: type_arguments.clone(),
        parameters: type_arguments.clone(),
        return_type: PlanType::Unit,
        span: span.clone(),
    });
    // Keep the full instantiation next to the signature that declares it, as `list_call` does.
    let callee = if type_arguments.is_empty() {
        CalleeKind::External(path)
    } else {
        CalleeKind::Instantiated(path, type_arguments)
    };
    terminate_call(builder, callee, arguments, destination, span);
}

/// Select the active borrowed payload or the eager default, releasing the unused default on the payload edge.
fn lower_carrier_default(
    builder: &mut Builder,
    declaration: &EnumDeclaration,
    present: i64,
    (source, target): (Place, Place),
    fallback: PlanOperand,
    span: &SourceSpan,
) -> Result<(), String> {
    let payload_type = declaration.variants[present as usize].fields[0].clone();
    let selected = builder.new_block(span);
    let missing = builder.new_block(span);
    let join = builder.new_block(span);
    let owners = builder.owned.clone();
    let condition = variant_condition(builder, &source, present, span);
    builder.finish(TerminatorKind::SwitchBool(condition, missing, selected), span);
    builder.select(selected);
    let projected = carrier_payload_place(&source, present, payload_type.clone(), span)?;
    let payload = carrier_payload_value(builder, projected, span)?;
    let value = RvalueKind::Use(payload);
    track_rvalue_moves(builder, &value);
    builder.emit(plan_assign(target.clone(), value, span));
    if let OperandKind::Move(place) = &fallback.kind {
        drop_owned(builder, place.local, span);
    }
    builder.finish(TerminatorKind::Goto(join), span);
    builder.select(missing);
    builder.owned = owners;
    let value = RvalueKind::Use(fallback);
    track_rvalue_moves(builder, &value);
    let local = target.local;
    builder.emit(plan_assign(target, value, span));
    let retained = builder.owned.clone();
    builder.finish(TerminatorKind::Goto(join), span);
    builder.select(join);
    builder.owned = retained;
    if payload_owns(&payload_type) {
        builder.owned.push(local);
    }
    Ok(())
}
