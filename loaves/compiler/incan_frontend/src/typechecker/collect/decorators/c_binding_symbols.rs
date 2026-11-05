//! C binding resources, symbol data, bounds, buffer contracts and outcomes.

use super::*;

impl TypeChecker {
    /// Extract the native opaque spelling and release symbol from one resource declaration.
    pub(super) fn c_resource_fields(
        member: &incan_vocab::VocabDeclaration,
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<(String, String)> {
        let mut native = None;
        let mut release = None;
        for item in &member.body {
            let incan_vocab::VocabBodyItem::Statement(statement) = item else {
                errors.push(CompileError::type_error(
                    "C resource declarations contain data fields only".to_string(),
                    span,
                ));
                return None;
            };
            let (field, value) = match statement {
                incan_vocab::IncanStatement::Let { name, value, .. } => (name, value),
                incan_vocab::IncanStatement::Assign { target, value } => (target, value),
                _ => {
                    errors.push(CompileError::type_error(
                        "C resource declarations contain data fields only".to_string(),
                        span,
                    ));
                    return None;
                }
            };
            let Some(argument) = c_abi::resource_argument_from_str(field) else {
                errors.push(CompileError::type_error(
                    format!("C resource does not accept `{field}`"),
                    span,
                ));
                return None;
            };
            match (argument, value) {
                (ResourceArgumentId::Native, incan_vocab::IncanExpr::Str(value))
                    if !value.is_empty() && native.replace(value.clone()).is_none() => {}
                (ResourceArgumentId::Release, incan_vocab::IncanExpr::Name(value))
                    if !value.is_empty() && release.replace(value.clone()).is_none() => {}
                _ => {
                    errors.push(CompileError::type_error(
                        format!("C resource `{field}` has the wrong value form or is repeated"),
                        span,
                    ));
                    return None;
                }
            }
        }
        match (native, release) {
            (Some(native), Some(release)) => Some((native, release)),
            _ => {
                errors.push(CompileError::type_error(
                    "C resource requires one non-empty `native` field and one `release` symbol".to_string(),
                    span,
                ));
                None
            }
        }
    }

    /// Extract the physical symbol spelling and declarative result outcomes from one raw C symbol.
    pub(super) fn c_symbol_data(
        member: &incan_vocab::VocabDeclaration,
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<RawCBindingSymbolData> {
        let mut native = None;
        let mut bounds = None;
        let mut outcomes = Vec::new();
        for item in &member.body {
            match item {
                incan_vocab::VocabBodyItem::Statement(statement) => {
                    let (field, value) = match statement {
                        incan_vocab::IncanStatement::Let { name, value, .. }
                        | incan_vocab::IncanStatement::Assign { target: name, value } => (name, value),
                        _ => {
                            errors.push(CompileError::type_error(
                                "C symbols contain only a `native` field and declarative outcomes".to_string(),
                                span,
                            ));
                            return None;
                        }
                    };
                    let Some(argument) = c_abi::symbol_argument_from_str(field) else {
                        errors.push(CompileError::type_error(
                            "C symbols accept only `native` and `bounds` data fields".to_string(),
                            span,
                        ));
                        return None;
                    };
                    match (argument, value) {
                        (SymbolArgumentId::Native, incan_vocab::IncanExpr::Str(value))
                            if !value.is_empty() && native.replace(value.clone()).is_none() => {}
                        (SymbolArgumentId::Native, _) => {
                            errors.push(CompileError::type_error(
                                "C symbol `native` must be one non-empty string literal".to_string(),
                                span,
                            ));
                            return None;
                        }
                        (SymbolArgumentId::Bounds, value) => {
                            let Some(parsed) = Self::c_symbol_bounds(value) else {
                                errors.push(CompileError::type_error(
                                    "C symbol `bounds` must be a non-empty dictionary of pointer parameter names to c.Size parameter names"
                                        .to_string(),
                                    span,
                                ));
                                return None;
                            };
                            if bounds.replace(parsed).is_some() {
                                errors.push(CompileError::type_error(
                                    "C symbols may state `bounds` once".to_string(),
                                    span,
                                ));
                                return None;
                            }
                        }
                    }
                }
                incan_vocab::VocabBodyItem::Declaration(outcome) => {
                    outcomes.push(Self::c_symbol_outcome(outcome, span, errors)?);
                }
                _ => {
                    errors.push(CompileError::type_error(
                        "C symbols contain only a `native` field and declarative outcomes".to_string(),
                        span,
                    ));
                    return None;
                }
            }
        }
        native
            .map(|native| (native, bounds.unwrap_or_default(), outcomes))
            .or_else(|| {
                errors.push(CompileError::type_error(
                    "C symbols require exactly one non-empty `native` field".to_string(),
                    span,
                ));
                None
            })
    }

    /// Parse the deliberately declarative pointer-to-length relation before checking it against one symbol signature.
    pub(super) fn c_symbol_bounds(value: &incan_vocab::IncanExpr) -> Option<Vec<(String, String)>> {
        let incan_vocab::IncanExpr::Dict(entries) = value else {
            return None;
        };
        if entries.is_empty() {
            return None;
        }
        let mut pointer_names = HashSet::new();
        let mut result = Vec::with_capacity(entries.len());
        for (pointer, length) in entries {
            let (incan_vocab::IncanExpr::Name(pointer), incan_vocab::IncanExpr::Name(length)) = (pointer, length)
            else {
                return None;
            };
            if pointer.is_empty() || length.is_empty() || !pointer_names.insert(pointer.clone()) {
                return None;
            }
            result.push((pointer.clone(), length.clone()));
        }
        Some(result)
    }

    /// Check every typed-span declaration against the symbol it bounds before it can become a compiler fact.
    pub(super) fn c_symbol_buffer_contracts(
        bounds: Vec<(String, String)>,
        symbol: &str,
        parameters: &[CBindingParameter],
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<Vec<CBindingBuffer>> {
        let pointer_parameters = parameters
            .iter()
            .filter_map(|parameter| match &parameter.ty {
                CBindingType::Pointer { pointee, .. }
                    if matches!(
                        pointee.as_ref(),
                        CBindingType::Scalar(c_abi::ScalarTypeId::U8 | c_abi::ScalarTypeId::F32)
                    ) =>
                {
                    let CBindingType::Scalar(element) = pointee.as_ref() else {
                        return None;
                    };
                    Some((parameter.name.as_str(), *element))
                }
                _ => None,
            })
            .collect::<HashMap<_, _>>();
        if pointer_parameters.is_empty() {
            if bounds.is_empty() {
                return Some(Vec::new());
            }
            errors.push(CompileError::type_error(
                format!("C symbol `{symbol}` declares checked-span bounds but has no c.ConstPtr[c.u8|c.f32] or c.MutPtr[c.u8|c.f32] parameter"),
                span,
            ));
            return None;
        }
        let declared_pointers = bounds
            .iter()
            .map(|(pointer, _)| pointer.as_str())
            .collect::<HashSet<_>>();
        if declared_pointers != pointer_parameters.keys().copied().collect::<HashSet<_>>() {
            errors.push(CompileError::type_error(
                format!(
                    "C symbol `{symbol}` must declare `bounds` for every checked span-pointer parameter exactly once"
                ),
                span,
            ));
            return None;
        }
        let mut contracts = Vec::with_capacity(bounds.len());
        for (pointer_parameter, length_parameter) in bounds {
            let Some(length) = parameters.iter().find(|parameter| parameter.name == length_parameter) else {
                errors.push(CompileError::type_error(
                    format!(
                        "C symbol `{symbol}` bounds `{pointer_parameter}` to unknown parameter `{length_parameter}`"
                    ),
                    span,
                ));
                return None;
            };
            if !matches!(length.ty, CBindingType::Scalar(c_abi::ScalarTypeId::Size)) {
                errors.push(CompileError::type_error(
                    format!("C symbol `{symbol}` bounds `{pointer_parameter}` with `{length_parameter}`, which must be c.Size"),
                    span,
                ));
                return None;
            }
            let Some(element) = pointer_parameters.get(pointer_parameter.as_str()).copied() else {
                errors.push(CompileError::type_error(
                    format!("C symbol `{symbol}` bounds unknown checked span-pointer `{pointer_parameter}`"),
                    span,
                ));
                return None;
            };
            contracts.push(CBindingBuffer {
                pointer_parameter,
                length_parameter,
                element,
            });
        }
        Some(contracts)
    }

    /// Extract one raw result declaration before checking it against a symbol's typed contract.
    pub(super) fn c_symbol_outcome(
        outcome: &incan_vocab::VocabDeclaration,
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<RawCBindingOutcome> {
        if outcome.keyword != c_abi::SYMBOL_OUTCOME_KEYWORD
            || outcome.head.name.is_some()
            || outcome.head.header_args.len() != 1
            || !outcome.head.parameters.is_empty()
            || outcome.head.return_type.is_some()
            || !outcome.decorators.is_empty()
        {
            errors.push(CompileError::type_error(
                "C symbol outcomes require one qualified result value and no signature".to_string(),
                span,
            ));
            return None;
        }
        let Some(result) = Self::c_native_reference(&outcome.head.header_args[0]) else {
            errors.push(CompileError::type_error(
                "C symbol outcomes require one qualified enum result value".to_string(),
                span,
            ));
            return None;
        };
        let mut initializes = None;
        let mut updates = None;
        let mut invalidates = None;
        for item in &outcome.body {
            let incan_vocab::VocabBodyItem::Statement(statement) = item else {
                errors.push(CompileError::type_error(
                    "C symbol outcomes contain data fields only".to_string(),
                    span,
                ));
                return None;
            };
            let (field, value) = match statement {
                incan_vocab::IncanStatement::Let { name, value, .. }
                | incan_vocab::IncanStatement::Assign { target: name, value } => (name, value),
                _ => {
                    errors.push(CompileError::type_error(
                        "C symbol outcomes contain data fields only".to_string(),
                        span,
                    ));
                    return None;
                }
            };
            let Some(field) = c_abi::symbol_outcome_argument_from_str(field) else {
                errors.push(CompileError::type_error(
                    "C symbol outcomes accept only initializes, updates, or invalidates".to_string(),
                    span,
                ));
                return None;
            };
            let Some(names) = Self::c_outcome_parameter_names(value) else {
                errors.push(CompileError::type_error(
                    "C symbol outcome fields require a list of unique parameter names".to_string(),
                    span,
                ));
                return None;
            };
            let slot = match field {
                SymbolOutcomeArgumentId::Initializes => &mut initializes,
                SymbolOutcomeArgumentId::Updates => &mut updates,
                SymbolOutcomeArgumentId::Invalidates => &mut invalidates,
            };
            if slot.replace(names).is_some() {
                errors.push(CompileError::type_error(
                    "C symbol outcomes may state each field once".to_string(),
                    span,
                ));
                return None;
            }
        }
        Some(RawCBindingOutcome {
            result,
            initializes: initializes.unwrap_or_default(),
            updates: updates.unwrap_or_default(),
            invalidates: invalidates.unwrap_or_default(),
        })
    }

    /// Extract an outcome's ordinary distinct parameter-name list.
    pub(super) fn c_outcome_parameter_names(value: &incan_vocab::IncanExpr) -> Option<Vec<String>> {
        let incan_vocab::IncanExpr::List(values) = value else {
            return None;
        };
        let mut names = Vec::with_capacity(values.len());
        let mut seen = HashSet::new();
        for value in values {
            let incan_vocab::IncanExpr::Name(name) = value else {
                return None;
            };
            if name.is_empty() || !seen.insert(name.clone()) {
                return None;
            }
            names.push(name.clone());
        }
        Some(names)
    }

    /// Check raw outcome data against one C symbol's scalar result and output-slot contracts.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn c_symbol_outcomes(
        raw_outcomes: Vec<RawCBindingOutcome>,
        symbol_name: &str,
        return_type: &CBindingType,
        parameters: &[CBindingParameter],
        enums: &[CBindingEnum],
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> Option<Vec<CBindingOutcome>> {
        let mut outcomes = Vec::with_capacity(raw_outcomes.len());
        let mut seen_results = HashSet::new();
        let mut initialized_outputs = HashSet::new();
        let mut valid = true;

        for raw in raw_outcomes {
            if !Self::c_outcome_carrier_valid(&raw, symbol_name, return_type, enums, span, errors) {
                valid = false;
                continue;
            }
            if !seen_results.insert(raw.result.clone()) {
                errors.push(CompileError::type_error(
                    format!("C symbol `{symbol_name}` repeats outcome `{}`", raw.result),
                    span,
                ));
                valid = false;
                continue;
            }

            let mut mentioned = HashSet::new();
            let mut outcome_valid = true;
            for (field, names, expected_mode) in [
                ("initializes", &raw.initializes, COutputMode::Out),
                ("updates", &raw.updates, COutputMode::InOut),
                ("invalidates", &raw.invalidates, COutputMode::InOut),
            ] {
                for name in names {
                    if !mentioned.insert(name.as_str()) {
                        errors.push(CompileError::type_error(
                            format!(
                                "C symbol `{symbol_name}` outcome `{}` repeats parameter `{name}`",
                                raw.result
                            ),
                            span,
                        ));
                        outcome_valid = false;
                        continue;
                    }
                    let Some(parameter) = parameters.iter().find(|parameter| parameter.name == *name) else {
                        errors.push(CompileError::type_error(
                            format!(
                                "C symbol `{symbol_name}` outcome `{}` names unknown parameter `{name}`",
                                raw.result
                            ),
                            span,
                        ));
                        outcome_valid = false;
                        continue;
                    };
                    if !matches!(&parameter.ty, CBindingType::Output { mode, .. } if *mode == expected_mode) {
                        let expected = match expected_mode {
                            COutputMode::Out => "c.Out[...]",
                            COutputMode::InOut => "c.InOut[...]",
                        };
                        errors.push(CompileError::type_error(
                            format!(
                                "C symbol `{symbol_name}` outcome `{}` `{field}` must name a {expected} parameter",
                                raw.result
                            ),
                            span,
                        ));
                        outcome_valid = false;
                        continue;
                    }
                    if expected_mode == COutputMode::Out {
                        initialized_outputs.insert(name.clone());
                    }
                }
            }
            if outcome_valid {
                outcomes.push(CBindingOutcome {
                    result: raw.result,
                    initializes: raw.initializes,
                    updates: raw.updates,
                    invalidates: raw.invalidates,
                });
            } else {
                valid = false;
            }
        }

        for parameter in parameters {
            if matches!(
                &parameter.ty,
                CBindingType::Output {
                    mode: COutputMode::Out,
                    ..
                }
            ) && !initialized_outputs.contains(&parameter.name)
            {
                errors.push(CompileError::type_error(
                    format!(
                        "C symbol `{symbol_name}` must initialize c.Out parameter `{}` through an outcome",
                        parameter.name
                    ),
                    span,
                ));
                valid = false;
            }
        }

        valid.then_some(outcomes)
    }

    /// Require an outcome to name a declared enum variant with the symbol's scalar result carrier before any
    /// duplicate-result or output-slot validation. The first failing carrier check retains its original diagnostic.
    pub(super) fn c_outcome_carrier_valid(
        raw: &RawCBindingOutcome,
        symbol_name: &str,
        return_type: &CBindingType,
        enums: &[CBindingEnum],
        span: Span,
        errors: &mut Vec<CompileError>,
    ) -> bool {
        let Some((enum_name, variant_name)) = raw.result.split_once('.') else {
            errors.push(CompileError::type_error(
                format!(
                    "C symbol `{symbol_name}` outcome `{}` must name an enum variant",
                    raw.result
                ),
                span,
            ));
            return false;
        };
        let Some(enumeration) = enums.iter().find(|enumeration| enumeration.name == enum_name) else {
            errors.push(CompileError::type_error(
                format!(
                    "C symbol `{symbol_name}` outcome `{}` names an unknown enum",
                    raw.result
                ),
                span,
            ));
            return false;
        };
        let Some(_variant) = enumeration.variants.iter().find(|variant| variant.name == variant_name) else {
            errors.push(CompileError::type_error(
                format!(
                    "C symbol `{symbol_name}` outcome `{}` names an unknown enum variant",
                    raw.result
                ),
                span,
            ));
            return false;
        };
        if !matches!(return_type, CBindingType::Scalar(carrier) if carrier == &enumeration.carrier) {
            errors.push(CompileError::type_error(
                format!(
                    "C symbol `{symbol_name}` outcome `{}` has a carrier incompatible with its return type",
                    raw.result
                ),
                span,
            ));
            return false;
        }
        true
    }
}
