use crate::ast::{ImportDecl, ImportKind, Spanned, Visibility};
use crate::diagnostics::{errors, lints};

use super::*;

#[test]
fn typecheck_phase_uses_import_span_for_import_diagnostics() {
    let program = Program {
        declarations: vec![
            Spanned::new(
                Declaration::Import(ImportDecl {
                    visibility: Visibility::Private,
                    kind: ImportKind::PubLibrary {
                        library: "missing".to_string(),
                        path: Vec::new(),
                    },
                    alias: None,
                }),
                Span::new(4, 24),
            ),
            Spanned::new(Declaration::Docstring("body".to_string()), Span::new(40, 46)),
        ],
        ..Program::default()
    };

    assert_eq!(
        phase_for_typecheck_span(&program, Span::new(8, 12)),
        DiagnosticPhase::Import
    );
    assert_eq!(
        phase_for_typecheck_span(&program, Span::new(42, 44)),
        DiagnosticPhase::Typecheck
    );
}

/// #1561: a vocabulary declaration such as `binding` lowers to an import and a class that share the block's span;
/// a refusal inside the block is a typecheck refusal (`INCAN-T0001`), not an import refusal (`INCAN-I0001`).
#[test]
fn refusal_inside_a_lowered_vocabulary_block_keeps_the_typecheck_phase() {
    let block = Span::new(40, 120);
    let program = Program {
        declarations: vec![
            Spanned::new(
                Declaration::Import(ImportDecl {
                    visibility: Visibility::Private,
                    kind: ImportKind::From {
                        module: crate::ast::ImportPath::simple(vec!["std".to_string(), "interop".to_string()]),
                        items: Vec::new(),
                    },
                    alias: None,
                }),
                Span::new(0, 30),
            ),
            Spanned::new(
                Declaration::Import(ImportDecl {
                    visibility: Visibility::Private,
                    kind: ImportKind::From {
                        module: crate::ast::ImportPath::simple(vec!["std".to_string(), "interop".to_string()]),
                        items: Vec::new(),
                    },
                    alias: None,
                }),
                block,
            ),
            Spanned::new(Declaration::Docstring("lowered class".to_string()), block),
        ],
        ..Program::default()
    };
    let refusal = errors::python_import_unsupported("numpy", Span::new(60, 70));

    assert_eq!(
        phase_for_typecheck_span(&program, Span::new(60, 70)),
        DiagnosticPhase::Typecheck
    );
    assert_eq!(
        code_for_error(
            &CompileError::type_error("empty header".to_string(), Span::new(60, 70)),
            phase_for_typecheck_span(&program, Span::new(60, 70))
        ),
        "INCAN-T0001"
    );
    assert_eq!(
        phase_for_typecheck_span(&program, Span::new(4, 8)),
        DiagnosticPhase::Import,
        "a written import keeps its span"
    );
    assert_eq!(code_for_error(&refusal, DiagnosticPhase::Import), "INCAN-I0001");
}

#[test]
fn stable_diagnostic_preserves_structured_facts_and_related_spans() {
    let error = CompileError::type_error("type mismatch".to_string(), Span::new(8, 13))
        .with_expected_actual("int", "str")
        .with_related_span(Span::new(0, 7), "Function parameter declared here");
    let diagnostic = stable_diagnostic("main.incn", "takes_x\n\"text\"\n", &error, DiagnosticPhase::Typecheck);

    assert_eq!(diagnostic.origin, DiagnosticOrigin::Typechecker);
    assert_eq!(diagnostic.expected.as_deref(), Some("int"));
    assert_eq!(diagnostic.actual.as_deref(), Some("str"));
    assert_eq!(diagnostic.related_spans.len(), 1);
    assert_eq!(diagnostic.related_spans[0].label, "Function parameter declared here");
    assert_eq!(diagnostic.related_spans[0].span.start.offset, 0);
}

#[test]
fn stable_diagnostic_keeps_cross_file_declarations_out_of_primary_source_coordinates()
-> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind};

    let declaration = CanonicalSymbolId::module_declaration(
        vec!["provider".to_string()],
        "parse",
        SemanticSourceTargetKind::Function,
        HirSourceSpan::new(120, 180),
    );
    let error = CompileError::type_error("bad call through alias".to_string(), Span::new(0, 5))
        .with_related_declaration(declaration.clone(), "declaration of `parse`");
    let diagnostic = stable_diagnostic("consumer.incn", "alias()\n", &error, DiagnosticPhase::Typecheck);

    assert!(diagnostic.related_spans.is_empty());
    let [related] = diagnostic.related_declarations.as_slice() else {
        return Err(format!("expected one related declaration: {diagnostic:?}").into());
    };
    assert_eq!(related.identity, declaration);
    assert_eq!(related.identity.declaration_span.start, 120);
    Ok(())
}

#[test]
fn provider_import_failures_use_distinct_stable_codes() {
    let disabled = errors::sdk_component_disabled("std.web", "stdlib-web", Span::default());
    let unavailable = errors::sdk_component_unavailable("std.web", "stdlib-web", "incan@0.5.0", Span::default());
    let gated = errors::pub_library_symbol_requires_features(
        "JsonReport",
        "reporting",
        &[vec!["json".to_string()]],
        Span::default(),
    );

    assert_eq!(code_for_error(&disabled, DiagnosticPhase::Import), "INCAN-I0101");
    assert_eq!(code_for_error(&unavailable, DiagnosticPhase::Import), "INCAN-I0102");
    assert_eq!(code_for_error(&gated, DiagnosticPhase::Import), "INCAN-I0103");
    let loop_refusal = errors::import_link_loop(
        &[
            (
                "a.X".to_string(),
                incan_semantics_core::SymbolOrigin::Module(vec!["a".to_string()]),
                Span::new(0, 19),
            ),
            (
                "b.X".to_string(),
                incan_semantics_core::SymbolOrigin::Module(vec!["b".to_string()]),
                Span::new(0, 19),
            ),
        ],
        Span::new(80, 81),
    );

    assert_eq!(code_for_error(&loop_refusal, DiagnosticPhase::Import), "INCAN-I0104");
    for code in ["INCAN-I0101", "INCAN-I0102", "INCAN-I0103", "INCAN-I0104"] {
        assert!(explain(code).is_some(), "{code} must have a catalog explanation");
    }
}

/// Checker refusal families retain distinct codes and the remedies attached by their owning catalog entries.
#[test]
fn checker_refusals_of_unbuildable_programs_use_distinct_stable_codes() -> Result<(), Box<dyn std::error::Error>> {
    let self_mutation = errors::self_mutation_requires_mut_self(
        "pop",
        "self.items",
        errors::SelfMutation::MutatingCall { callee: "pop" },
        Span::default(),
    );
    let print_union = errors::value_has_no_printed_form(
        errors::DisplayPosition::Print { builtin: "print" },
        Some("value"),
        errors::UnprintableValue::Union,
        Span::default(),
    );
    let str_of_model = errors::value_has_no_printed_form(
        errors::DisplayPosition::Str,
        None,
        errors::UnprintableValue::Nominal { type_name: "Account" },
        Span::default(),
    );
    let interpolated_union = errors::value_has_no_printed_form(
        errors::DisplayPosition::Interpolation,
        Some("value"),
        errors::UnprintableValue::Union,
        Span::default(),
    );
    let bound_enum = errors::value_has_no_printed_form(
        errors::DisplayPosition::Bound {
            callee: "show",
            type_param: "T",
        },
        None,
        errors::UnprintableValue::Nominal { type_name: "Color" },
        Span::default(),
    );
    let bare_tuple = errors::tuple_annotation_requires_element_types("Tuple", Span::default());
    let bare_option = errors::collection_annotation_requires_type_arguments("Option", Span::default());
    let open_generics = errors::rust_owner_type_args_not_inferred(
        "HashMap",
        "new",
        &["K".to_string(), "V".to_string()],
        Some("untyped"),
        Span::default(),
    );
    let open_parameter = ["S".to_string()];
    let candidates = ["AsRef<OsStr>::as_ref".to_string(), "AsRef<Path>::as_ref".to_string()];
    let unselected = [
        errors::NativeCalleeGap::OpenTypeParameters(&open_parameter),
        errors::NativeCalleeGap::NoCandidate,
        errors::NativeCalleeGap::Ambiguous(&candidates),
    ]
    .map(|gap| errors::rust_callee_not_selected("rust::native::Owner.method", gap, Span::default()));

    assert_eq!(
        code_for_error(&self_mutation, DiagnosticPhase::Typecheck),
        "INCAN-T0102"
    );
    let debug_generator =
        errors::value_has_no_debug_form(Some("numbers"), errors::UndebuggableValue::Generator, Span::default());
    for display in [
        &print_union,
        &str_of_model,
        &interpolated_union,
        &bound_enum,
        &debug_generator,
    ] {
        assert_eq!(code_for_error(display, DiagnosticPhase::Typecheck), "INCAN-T0103");
    }
    assert_eq!(code_for_error(&bare_tuple, DiagnosticPhase::Typecheck), "INCAN-T0104");
    assert_eq!(code_for_error(&bare_option, DiagnosticPhase::Typecheck), "INCAN-T0104");
    assert_eq!(
        code_for_error(&open_generics, DiagnosticPhase::Typecheck),
        "INCAN-T0105"
    );
    for refusal in &unselected {
        assert_eq!(code_for_error(refusal, DiagnosticPhase::Typecheck), "INCAN-T0122");
    }
    assert_eq!(
        unselected.each_ref().map(|refusal| refusal.message.as_str()),
        [
            "Cannot select the Rust callee of `rust::native::Owner.method`: its type parameter `S` is not fixed by \
                 the call",
            "Cannot select the Rust callee of `rust::native::Owner.method`: no admitted implementation of the \
                 method applies to this receiver and these arguments",
            "Cannot select the Rust callee of `rust::native::Owner.method`: `AsRef<OsStr>::as_ref` and \
                 `AsRef<Path>::as_ref` apply equally",
        ]
    );
    for code in [
        "INCAN-T0102",
        "INCAN-T0103",
        "INCAN-T0104",
        "INCAN-T0105",
        "INCAN-T0122",
    ] {
        let Some(entry) = explain(code) else {
            return Err(format!("{code} must have a catalog explanation").into());
        };
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
    }
    assert!(
        open_generics
            .hints()
            .iter()
            .any(|hint| hint.contains("HashMap.new[str, int]()") && hint.contains("untyped: HashMap[str, int]")),
        "the remedy must spell both the call and the binding form, got {:?}",
        open_generics.hints()
    );
    assert_eq!(print_union.message, "'print' cannot print the union value 'value'");
    assert_eq!(
        str_of_model.message,
        "'str' cannot convert a value of type 'Account' to text"
    );
    assert_eq!(
        interpolated_union.message,
        "f-string cannot interpolate the union value 'value'"
    );
    assert_eq!(
        bound_enum.message,
        "Call to 'show' cannot bind a value of type 'Color' to 'T', which requires 'Display'"
    );
    assert!(
        bound_enum
            .hints()
            .iter()
            .all(|hint| hint.contains("__str__") && !hint.contains(":?")),
        "a bound's remedy names __str__ and no f-string part the caller does not have, got {:?}",
        bound_enum.hints()
    );
    assert_eq!(
        bare_option.message,
        "Option annotation 'Option' is missing its value type"
    );
    assert!(
        bare_option.hints().iter().any(|hint| hint.contains("'Option[int]'")),
        "the remedy must spell the parameterized annotation, got {:?}",
        bare_option.hints()
    );
    Ok(())
}

/// Reserved compiler names retain an explainable code and a usable source-name suggestion.
#[test]
fn reserved_compiler_name_refusal_uses_its_own_stable_code() -> Result<(), Box<dyn std::error::Error>> {
    let function = errors::reserved_compiler_name("__incan_original_target", "function", Span::default());
    let bare_prefix = errors::reserved_compiler_name("__incan_", "binding", Span::default());

    assert_eq!(code_for_error(&function, DiagnosticPhase::Typecheck), "INCAN-T0111");
    let Some(entry) = explain("INCAN-T0111") else {
        return Err("INCAN-T0111 must have a catalog explanation".into());
    };
    assert_eq!(entry.severity, "error");
    assert_eq!(entry.phase, "typecheck");
    assert!(
        function
            .hints()
            .iter()
            .any(|hint| hint.contains("for example 'original_target'")),
        "the remedy must suggest the name without the prefix, got {:?}",
        function.hints()
    );
    assert!(
        bare_prefix.hints().iter().all(|hint| !hint.contains("for example")),
        "a name that is only the prefix has nothing left to suggest, got {:?}",
        bare_prefix.hints()
    );
    Ok(())
}

/// Typed record declaration refusals retain their own explainable codes and label both sides of a collision.
#[test]
fn record_declaration_refusals_use_their_stable_codes() -> Result<(), Box<dyn std::error::Error>> {
    let reserved = errors::store_reserved_model_field("___id", Span::default());
    let collision = errors::record_member_identity_collision(
        "model",
        "CollidingFields",
        "field",
        "fieldbbea15ef9eff",
        Span::new(10, 20),
        "field3d4624c2d3a2",
        Span::new(30, 40),
        "38dd14b42a9a",
    );

    assert_eq!(code_for_error(&reserved, DiagnosticPhase::Typecheck), "INCAN-T0127");
    assert_eq!(code_for_error(&collision, DiagnosticPhase::Typecheck), "INCAN-T0128");
    let reserved_entry = explain("INCAN-T0127").ok_or("INCAN-T0127 must have a catalog explanation")?;
    let collision_entry = explain("INCAN-T0128").ok_or("INCAN-T0128 must have a catalog explanation")?;
    assert_eq!(reserved_entry.severity, "error");
    assert_eq!(collision_entry.severity, "error");
    assert_eq!(collision.related_spans().len(), 1);
    Ok(())
}

#[test]
fn kept_dict_lookup_refusal_uses_its_stable_code_followups_a() -> Result<(), Box<dyn std::error::Error>> {
    let refusal = errors::kept_dict_lookup_value_cannot_be_copied("JoinHandle[int]", Span::default());
    assert_eq!(code_for_error(&refusal, DiagnosticPhase::Typecheck), "INCAN-T0118");
    let entry = explain("INCAN-T0118").ok_or("INCAN-T0118 must have a catalog explanation")?;
    assert_eq!(entry.severity, "error");
    assert_eq!(entry.phase, "typecheck");
    assert!(
        refusal.message.contains("JoinHandle[int]"),
        "the refusal must name the stored value type, got {:?}",
        refusal.message
    );
    Ok(())
}

/// Mutable argument diagnostics preserve distinct remedies for bindings, positions and element writes.
#[test]
fn immutable_argument_to_mut_parameter_uses_its_stable_code_issue1773() -> Result<(), String> {
    let binding = errors::immutable_argument_to_mut_parameter(
        errors::MutParameterLabel::Named("items"),
        "extend",
        errors::MutParameterChange::Changes,
        errors::MutArgumentPlace::Binding("items".to_string()),
        Span::default(),
    );
    let element = errors::immutable_argument_to_mut_parameter(
        errors::MutParameterLabel::Named("items"),
        "extend",
        errors::MutParameterChange::Changes,
        errors::MutArgumentPlace::Element,
        Span::default(),
    );
    let positional = errors::immutable_argument_to_mut_parameter(
        errors::MutParameterLabel::Position(1),
        "step",
        errors::MutParameterChange::MayChange,
        errors::MutArgumentPlace::Binding("counter".to_string()),
        Span::default(),
    );
    assert_eq!(
        positional.message,
        "Argument for the 'mut' parameter at position 1 of 'step' must be a mutable binding"
    );
    assert!(
        positional
            .notes()
            .iter()
            .any(|note| note.starts_with("'step' may change")),
        "a callee whose body is not known is said to possibly change the parameter, got {:?}",
        positional.notes()
    );
    assert!(
        binding.notes().iter().any(|note| note.starts_with("'extend' changes")),
        "a callee known to change the parameter is said to change it, got {:?}",
        binding.notes()
    );
    for error in [&binding, &element, &positional] {
        assert_eq!(code_for_error(error, DiagnosticPhase::Typecheck), "INCAN-T0117");
    }
    let entry = explain("INCAN-T0117").ok_or("INCAN-T0117 must have a catalog explanation")?;
    assert_eq!(entry.severity, "error");
    assert_eq!(entry.phase, "typecheck");
    let uncopyable = errors::uncopyable_argument_to_mut_parameter(
        errors::MutParameterLabel::Named("g"),
        "ignore",
        "Generator[int]",
        errors::MutArgumentPlace::Binding("g".to_string()),
        Span::default(),
    );
    assert_eq!(code_for_error(&uncopyable, DiagnosticPhase::Typecheck), "INCAN-T0117");
    assert!(
        uncopyable
            .notes()
            .iter()
            .any(|note| note.contains("does not change") && note.contains("'Generator[int]' cannot be copied")),
        "a refusal for an unchanged parameter says the argument cannot be copied, got {:?}",
        uncopyable.notes()
    );
    assert!(
        binding.hints().iter().any(|hint| hint.contains("mut items = ...")),
        "an immutable binding's remedy names the binding to declare 'mut', got {:?}",
        binding.hints()
    );
    assert!(
        element.hints().iter().any(|hint| hint.contains("store it back")),
        "an element's remedy says to pass a 'mut' variable and store it back, got {:?}",
        element.hints()
    );
    Ok(())
}

/// An uncopyable retained dictionary lookup reports its stable code and explains the unavailable copy.
#[test]
fn kept_dict_lookup_of_a_value_that_cannot_be_copied_uses_its_stable_code() {
    let refusal = errors::kept_dict_lookup_value_cannot_be_copied("Generator[int]", Span::default());
    assert_eq!(code_for_error(&refusal, DiagnosticPhase::Typecheck), "INCAN-T0118");
    let Some(entry) = explain("INCAN-T0118") else {
        panic!("INCAN-T0118 must have a catalog explanation");
    };
    assert_eq!(entry.severity, "error");
    assert_eq!(entry.phase, "typecheck");
    assert!(
        refusal.message.contains("Generator[int]"),
        "the refusal names the value type"
    );
    assert!(
        refusal
            .hints()
            .iter()
            .any(|hint| hint.contains("`len`, `print` or `println`") && hint.contains("f-string")),
        "the hint names the uses that count as reading, got {:?}",
        refusal.hints()
    );
    assert!(
        refusal.hints().iter().any(|hint| {
            hint.contains("a Rust method with a shared receiver whose result is not kept")
                && hint.contains("an Incan method taking `self` does not count")
        }),
        "the hint names exactly which method calls count as reading, got {:?}",
        refusal.hints()
    );
}

/// Route and operator refusals retain their distinct codes, notes and source-level remedies.
#[test]
fn route_handler_and_type_parameter_operator_refusals_use_distinct_stable_codes() {
    let return_type = errors::route_handler_return_not_response("create_user", "int", Span::default());
    let unbound = errors::route_handler_parameter_unbound(
        "create",
        "id",
        "/things",
        &["year".to_string(), "month".to_string()],
        Span::default(),
    );
    let operator = errors::operator_has_no_type_parameter_bound("%", "T", "__mod__", Span::default());
    let payload = errors::route_payload_without_json_form("search", "Query[Search]", "Search", false, Span::default());

    assert_eq!(code_for_error(&return_type, DiagnosticPhase::Typecheck), "INCAN-T0107");
    assert_eq!(code_for_error(&unbound, DiagnosticPhase::Typecheck), "INCAN-T0108");
    assert_eq!(code_for_error(&operator, DiagnosticPhase::Typecheck), "INCAN-T0109");
    assert_eq!(code_for_error(&payload, DiagnosticPhase::Typecheck), "INCAN-T0112");
    assert!(
        payload.hints().iter().any(|hint| hint.contains("'@derive(json)'")),
        "the remedy must name the JSON derive, got {:?}",
        payload.hints()
    );
    for code in ["INCAN-T0107", "INCAN-T0108", "INCAN-T0109", "INCAN-T0112"] {
        let Some(entry) = explain(code) else {
            panic!("{code} must have a catalog explanation");
        };
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
    }
    assert!(
        unbound.hints().iter().any(|hint| hint.contains("'/things/{id}'")),
        "the remedy must spell the path with the segment added, got {:?}",
        unbound.hints()
    );
    assert!(
        unbound
            .notes()
            .iter()
            .any(|note| note == "The path binds 'year', 'month'"),
        "the captures the path does bind must be listed, got {:?}",
        unbound.notes()
    );
    assert!(
        operator.hints().iter().any(|hint| hint.contains("'__mod__'")),
        "the remedy must name the operator's trait hook, got {:?}",
        operator.hints()
    );
}

/// Issue #1790: the refused receiver spelling on a method decorator has its own explainable code, and its remedy
/// names the spelling to write instead.
#[test]
fn method_decorator_receiver_spelling_refusal_uses_a_distinct_stable_code() -> Result<(), Box<dyn std::error::Error>> {
    let shared = errors::method_decorator_receiver_spelling(
        "Method decorator '@as_int'",
        "label",
        "&Box",
        "(Box, int) -> str",
        false,
        Span::default(),
    );
    let mutable = errors::method_decorator_receiver_spelling(
        "Method decorator '@keep'",
        "bump",
        "&mut Counter",
        "(mut Counter, int) -> int",
        true,
        Span::default(),
    );

    for (refusal, shape, receiver) in [
        (&shared, "`(Box, int) -> str`", "`self`"),
        (&mutable, "`(mut Counter, int) -> int`", "`mut self`"),
    ] {
        assert_eq!(code_for_error(refusal, DiagnosticPhase::Typecheck), "INCAN-T0110");
        assert!(
            refusal.hints().iter().any(|hint| hint.contains(shape)
                && hint.contains(receiver)
                && hint.contains("the compiler decides how the receiver is passed")),
            "the remedy must spell the replacement shape, got {:?}",
            refusal.hints()
        );
    }
    let Some(entry) = explain("INCAN-T0110") else {
        return Err("INCAN-T0110 must have a catalog explanation".into());
    };
    assert_eq!(entry.severity, "error");
    assert_eq!(entry.phase, "typecheck");
    Ok(())
}

/// Issue #1790: a `self`-method decorator chain the receiver cannot be passed through has its own explainable
/// code, and the refusal says what is refused, why, and what to write instead.
/// Invalid method decorator receiver plans retain the dedicated remedy and catalog explanation.
#[test]
fn method_decorator_receiver_plan_refusal_uses_a_distinct_stable_code() -> Result<(), Box<dyn std::error::Error>> {
    let refusal = errors::method_decorator_receiver_not_planned(
        "'parse' cannot be used here",
        "it takes the place of `self` method 'label' through '@as_int', so it is only returned there or called \
             directly",
        "Call 'parse' directly, or give this use a function of its own",
        Span::default(),
    );
    assert_eq!(code_for_error(&refusal, DiagnosticPhase::Typecheck), "INCAN-T0116");
    assert_eq!(
        refusal.message,
        "'parse' cannot be used here: it takes the place of `self` method 'label' through '@as_int', so it is \
             only returned there or called directly"
    );
    assert!(
        refusal
            .hints()
            .iter()
            .any(|hint| hint == "Call 'parse' directly, or give this use a function of its own"),
        "the refusal must say what to write instead, got {:?}",
        refusal.hints()
    );
    let Some(entry) = explain("INCAN-T0116") else {
        return Err("INCAN-T0116 must have a catalog explanation".into());
    };
    assert_eq!(entry.severity, "error");
    assert_eq!(entry.phase, "typecheck");
    Ok(())
}

/// Issue #1844: a list used again after a `for` loop took its items has its own explainable code, and the
/// refusal names the list, points back at the loop and says what to write instead.
/// Reusing a taken list points to a binding before the loop under its dedicated stable diagnostic code.
#[test]
fn taken_list_reuse_refusal_uses_a_distinct_stable_code() -> Result<(), Box<dyn std::error::Error>> {
    let refusal = errors::taken_list_used_again(
        "handles",
        "JoinHandle[int]",
        errors::TakenListReuse::AfterLoop,
        Span::new(90, 97),
        Span::new(40, 47),
    );
    assert_eq!(code_for_error(&refusal, DiagnosticPhase::Typecheck), "INCAN-T0119");
    assert_eq!(
        refusal.message,
        "`handles` is used after the `for` loop that took its items"
    );
    assert_eq!(
        refusal.related_spans().len(),
        1,
        "the refusal must point back at the loop"
    );
    assert!(
        refusal.hints().iter().any(|hint| hint.contains("before the loop")),
        "the refusal must say what to write instead, got {:?}",
        refusal.hints()
    );
    let Some(entry) = explain("INCAN-T0119") else {
        return Err("INCAN-T0119 must have a catalog explanation".into());
    };
    assert_eq!(entry.severity, "error");
    assert_eq!(entry.phase, "typecheck");
    let untakeable = errors::loop_cannot_take_items(
        errors::UntakeableIterable::MutParameter("handles"),
        "JoinHandle[int]",
        Span::new(40, 47),
    );
    assert_eq!(code_for_error(&untakeable, DiagnosticPhase::Typecheck), "INCAN-T0119");
    assert_eq!(
        untakeable.message,
        "the `for` loop cannot take its `JoinHandle[int]` items out of `handles`, a `mut` parameter whose items belong to the caller"
    );
    Ok(())
}

/// Unmet capability requirements preserve separate codes and actionable key and task remedies.
#[test]
fn capability_requirement_refusals_use_distinct_stable_codes() -> Result<(), Box<dyn std::error::Error>> {
    let field = errors::member_type_lacks_automatic_derives(
        "model",
        "Pending",
        errors::DerivedMember::Field("handle"),
        "JoinHandle[int]",
        "JoinHandle[int]",
        &["Clone", "Debug"],
        Span::default(),
    );
    let key = errors::collection_member_lacks_hash_derives(
        errors::HashedCollectionRole::SetElement,
        "Tag",
        "Tag",
        &["Eq", "Hash"],
        errors::HashRemedy::AddDerives,
        Span::default(),
    );
    let instantiation = errors::type_argument_lacks_hash_derives(
        "unique",
        "T",
        "Tag",
        "Tag",
        &["Hash"],
        errors::HashRemedy::AddDerives,
        Span::default(),
    );
    let task = errors::argument_is_not_a_task("spawn", errors::TaskArgument::AsyncFunction("work"), Span::default());

    assert_eq!(code_for_error(&field, DiagnosticPhase::Typecheck), "INCAN-T0113");
    assert_eq!(code_for_error(&key, DiagnosticPhase::Typecheck), "INCAN-T0114");
    assert_eq!(
        code_for_error(&instantiation, DiagnosticPhase::Typecheck),
        "INCAN-T0114"
    );
    assert_eq!(code_for_error(&task, DiagnosticPhase::Typecheck), "INCAN-T0115");
    for code in ["INCAN-T0113", "INCAN-T0114", "INCAN-T0115"] {
        let Some(entry) = explain(code) else {
            return Err(format!("{code} must have a catalog explanation").into());
        };
        assert_eq!(entry.severity, "error");
        assert_eq!(entry.phase, "typecheck");
    }
    assert!(
        key.hints().iter().any(|hint| hint.contains("@derive(Eq, Hash)")),
        "the remedy must name the derives to add, got {:?}",
        key.hints()
    );
    assert!(
        task.hints()
            .iter()
            .any(|hint| hint.contains("'work()' in place of 'work'")),
        "the remedy must rewrite only the offending argument, got {:?}",
        task.hints()
    );
    Ok(())
}

#[test]
fn unreachable_code_warning_projects_as_an_explainable_typecheck_warning() {
    let warning = lints::unreachable_code_after_return(Span::new(20, 40), Span::new(8, 16));
    let diagnostic = stable_diagnostic(
        "main.incn",
        "def f() -> int:\n    return 1\n    println(\"dead\")\n",
        &warning,
        DiagnosticPhase::Typecheck,
    );

    assert_eq!(diagnostic.code, "INCAN-T0101");
    assert_eq!(diagnostic.severity, "warning");
    assert_eq!(diagnostic.phase, DiagnosticPhase::Typecheck);
    assert_eq!(diagnostic.origin, DiagnosticOrigin::Typechecker);
    assert_eq!(diagnostic.explain, "incan explain INCAN-T0101");
    assert_eq!(
        diagnostic.related_spans.len(),
        1,
        "the projection must keep the originating `return` location"
    );
    assert!(
        explain("INCAN-T0101").is_some(),
        "INCAN-T0101 must have a catalog explanation"
    );
}

/// Issue #2716: explicit type arguments on a nongeneric callable are refused at the written list with an actionable,
/// explainable diagnostic instead of reaching lowering.
#[test]
fn nongeneric_call_type_arguments_use_a_distinct_stable_code() -> Result<(), Box<dyn std::error::Error>> {
    let span = Span::new(14, 17);
    let refusal = errors::explicit_type_arg_arity("upper", &[], &["int".to_string()], span);

    assert_eq!(code_for_error(&refusal, DiagnosticPhase::Typecheck), "INCAN-T0129");
    assert_eq!(refusal.message, "'upper' takes no type arguments");
    assert_eq!(refusal.span, span);
    assert!(
        refusal
            .hints()
            .iter()
            .any(|hint| hint == "Remove the type argument list"),
        "the refusal must name the source fix, got {:?}",
        refusal.hints()
    );
    let Some(entry) = explain("INCAN-T0129") else {
        return Err("INCAN-T0129 must have a catalog explanation".into());
    };
    assert_eq!(entry.severity, "error");
    assert_eq!(entry.phase, "typecheck");
    Ok(())
}
