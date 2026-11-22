//! Source-owned raw newtype string conversion roots, with Incan parsing, composition and error propagation.

use std::sync::Arc;

use incan_mir_plan_native::{
    CalleeKind, EnumDeclaration, ExternalFunction, Function, Parameter, PlanType, RvalueKind, SourceSpan,
    TerminatorKind, assign as plan_assign, place as plan_place, source_span as plan_span,
};
use incan_semantics_core::body_ir::{BodyIrModule, NamedCallableTarget, NominalDeclaration, StringConversionContract};
use incan_semantics_core::{IncanPrimitiveType, IncanType, generic};

use crate::frame::{Builder, drop_all_owned, forget_owned, read_place, temporary, terminate_call, track_rvalue_moves};
use crate::layout_types::native_source_type;
use crate::native_type_identity::ConstructorIndex;
use crate::options::{propagate_result, result_layout, retain_carrier};
use crate::string_conversion_contracts::{conversion_name, conversion_trait};
use crate::string_input::owned_string_frame;
use crate::type_parameters::substitute;
use crate::unions::collect_unions;

/// Plan selected raw chain roots from `start`, leaving a continuation's unchanged roots intact.
pub fn lower_string_conversions(
    constructors: &Arc<ConstructorIndex>,
    module: &BodyIrModule,
    file: &str,
    intrinsics: &mut Vec<EnumDeclaration>,
    start: usize,
) -> Result<(Vec<Function>, Vec<ExternalFunction>), String> {
    let mut functions = Vec::new();
    let mut externals = Vec::new();
    if conversion_trait(module)?.is_empty() {
        return Ok((functions, externals));
    }
    for index in start..module.nominal_declarations.len() {
        let declaration = &module.nominal_declarations[index];
        let name = conversion_name(module, declaration)?;
        if name.is_empty() {
            continue;
        }
        let field = underlying_field(declaration)?;
        let text = IncanType::Primitive(IncanPrimitiveType::Str);
        let output = generic(
            "Result".into(),
            vec![IncanType::Named(declaration.name.clone()), text.clone()],
        );
        retain_carrier(constructors, module, intrinsics, &output, file)?;
        let input_result = generic("Result".into(), vec![field.clone(), text]);
        retain_carrier(constructors, module, intrinsics, &input_result, file)?;
        let nominals = &module.nominal_declarations;
        let enums = &module.enum_declarations;
        let result = native_source_type(
            constructors,
            nominals,
            enums,
            &collect_unions(module),
            &output,
            intrinsics,
            "",
        )?;
        let underlying_result = native_source_type(
            constructors,
            nominals,
            enums,
            &collect_unions(module),
            &input_result,
            intrinsics,
            "",
        )?;
        let span = plan_span(file, 1, 1);
        let (function, runtime) = conversion_function(
            constructors.clone(),
            module,
            index,
            name,
            (result, underlying_result),
            intrinsics,
            &span,
        )?;
        functions.push(function);
        externals.extend(runtime);
    }
    Ok((functions, externals))
}

/// The newtype's single underlying field type.
fn underlying_field(declaration: &NominalDeclaration) -> Result<&IncanType, String> {
    declaration
        .field_types
        .first()
        .ok_or_else(|| "invalid Body IR automatic string conversion newtype without a field".to_string())
}

/// Convert the owned string once, propagate Err unchanged, and wrap only the successful underlying value.
fn conversion_function(
    constructors: Arc<ConstructorIndex>,
    module: &BodyIrModule,
    index: usize,
    name: String,
    (result, underlying_result): (PlanType, PlanType),
    intrinsics: &[EnumDeclaration],
    span: &SourceSpan,
) -> Result<(Function, Vec<ExternalFunction>), String> {
    let declaration = &module.nominal_declarations[index];
    let mut builder = owned_string_frame(constructors.clone(), module, &name, result.clone(), intrinsics, span);
    let underlying = native_source_type(
        &constructors,
        &module.nominal_declarations,
        &module.enum_declarations,
        &collect_unions(module),
        underlying_field(declaration)?,
        intrinsics,
        "",
    )?;
    let payload = temporary(&mut builder, underlying.clone(), span);
    if underlying == PlanType::String {
        builder.emit(plan_assign(
            payload.clone(),
            RvalueKind::Use(read_place(plan_place(1, span), true, span)),
            span,
        ));
        forget_owned(&mut builder, 1);
        builder.owned.push(payload.local);
    } else {
        let parsed = temporary(&mut builder, underlying_result.clone(), span);
        let callee = underlying_conversion(module, &mut builder, declaration, &underlying_result, span)?;
        terminate_call(
            &mut builder,
            callee,
            vec![read_place(plan_place(1, span), true, span)],
            parsed.clone(),
            span,
        );
        builder.owned.push(parsed.local);
        propagate_result(&mut builder, parsed, payload.clone(), span)?;
    }
    let owner = PlanType::Model(index as i64, declaration.name.clone());
    let wrapped = temporary(&mut builder, owner, span);
    let construction = RvalueKind::Model(index as i64, vec![read_place(payload, true, span)]);
    track_rvalue_moves(&mut builder, &construction);
    builder.emit(plan_assign(wrapped.clone(), construction, span));
    builder.owned.push(wrapped.local);
    let (result_owner, _) = result_layout(&builder, &result)?;
    let success = RvalueKind::Enum(result_owner, 0, vec![read_place(wrapped, true, span)]);
    track_rvalue_moves(&mut builder, &success);
    builder.emit(plan_assign(plan_place(0, span), success, span));
    drop_all_owned(&mut builder, span);
    builder.finish(TerminatorKind::Return, span);
    let function = Function::new(
        name,
        vec![Parameter {
            name: "value".into(),
            ty: PlanType::String,
            span: span.clone(),
        }],
        result,
        builder.locals,
        builder.blocks,
        span.clone(),
    );
    Ok((function, builder.externals))
}

/// A primitive leaf calls the authored Incan parser; a named edge calls only its previously validated local root.
fn underlying_conversion(
    module: &BodyIrModule,
    builder: &mut Builder,
    owner: &NominalDeclaration,
    result: &PlanType,
    span: &SourceSpan,
) -> Result<CalleeKind, String> {
    match underlying_field(owner)? {
        IncanType::Primitive(IncanPrimitiveType::Int) => {
            let path = "incan_native_runtime::parse_newtype_int";
            builder.externals.push(ExternalFunction {
                path: path.to_owned(),
                type_arguments: Vec::new(),
                parameters: vec![PlanType::String],
                return_type: result.clone(),
                span: span.clone(),
            });
            Ok(CalleeKind::External(path.to_owned()))
        }
        IncanType::Named(name) => {
            if let Some(identity) = owner.named_type_identities.get(name) {
                for declaration in &module.nominal_declarations {
                    // The foreign map exposes a borrowed identity; compare owned identity values.
                    if declaration.canonical == *identity {
                        let root = conversion_name(module, declaration)?;
                        if !root.is_empty() {
                            return Ok(CalleeKind::Planned(root));
                        }
                    }
                }
            }
            Err("unsupported Body IR automatic string conversion underlying selection".into())
        }
        _ => Err("unsupported Body IR automatic string conversion primitive".into()),
    }
}

/// Dispatch only the actual selected Package trait slot at its checked closed receiver, never a method spelling.
pub fn string_conversion_callee(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: &NamedCallableTarget,
    span: &SourceSpan,
) -> Result<Option<(CalleeKind, PlanType)>, String> {
    if conversion_trait(module)?.is_empty() {
        return Ok(None);
    }
    let StringConversionContract::Selected(selection) = &module.string_conversion_contract else {
        return Ok(None);
    };
    if target.canonical.as_ref() != Some(&selection.method_identity) {
        return Ok(None);
    }
    if !target.type_args.is_empty() {
        return Err("unsupported Body IR string conversion method type arguments".into());
    }
    if target.direct_call_id.is_some() {
        return Err("unsupported Body IR string conversion local trait slot".into());
    }
    let Some(receiver) = &target.receiver_type else {
        return Err("unsupported Body IR string conversion without checked receiver".into());
    };
    let closed = substitute(receiver, &builder.type_parameters, &builder.type_arguments)?;
    let output = generic(
        "Result".into(),
        vec![closed.clone(), IncanType::Primitive(IncanPrimitiveType::Str)],
    );
    let result = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &output,
        &builder.intrinsics,
        "",
    )?;
    match &closed {
        IncanType::Primitive(IncanPrimitiveType::Int) => {
            let path = "incan_native_runtime::parse_newtype_int";
            builder.externals.push(ExternalFunction {
                path: path.to_owned(),
                type_arguments: Vec::new(),
                parameters: vec![PlanType::String],
                return_type: result.clone(),
                span: span.clone(),
            });
            Ok(Some((CalleeKind::External(path.to_owned()), result)))
        }
        IncanType::Named(name) => {
            for declaration in &module.nominal_declarations {
                if declaration.name == *name || declaration.carrier_name() == *name {
                    let root = conversion_name(module, declaration)?;
                    if !root.is_empty() {
                        return Ok(Some((CalleeKind::Planned(root), result)));
                    }
                }
            }
            Err("unsupported Body IR automatic string conversion underlying selection".into())
        }
        _ => Err("unsupported Body IR string conversion receiver profile".into()),
    }
}
