//! Module seed construction: source storage, body contracts, published declarations, and source body lowering.

use super::*;

/// Construct the compatibility module before representation refinement. This remains the whole-module route;
/// separating its seed lets the cold baseline compare individual representation operations from identical inputs.
pub(super) fn build_module_seed(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
    context: &BodyIrContext<'_>,
) -> bir::BodyIrModule {
    build_module_seed_observed(program, module_path, type_info, context, &mut |_, _, _, _| None)
}

/// Build the compatibility seed while exposing each free-function seed to an optional migration comparison. The
/// observer can substitute an independently built seed for comparison and is absent from production fact selection.
pub(super) fn build_module_seed_observed(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
    context: &BodyIrContext<'_>,
    observer: &mut dyn FnMut(
        &ast::FunctionDecl,
        ast::Span,
        &BodyIrLoweringFacts<'_, '_>,
        &bir::Body,
    ) -> Option<bir::Body>,
) -> bir::BodyIrModule {
    let provider_operations = context.provider_operations;
    let registry_package = context.registry_package;
    let include_inline_tests = context.include_inline_tests;
    let module_identity = match context.library_package {
        Some(package) => incan_semantics_core::package_module_identity(package, module_path),
        None => body_ir_module_identity(module_path),
    };
    let retain_nominal_carriers = true;
    let context = context.modules;
    let module_id = CompilerNodeId::module(module_identity.clone());
    let function_default_sources = collect_function_default_sources(program, type_info);
    let local_function_declarations = collect_local_function_declarations(program);
    let local_nongeneric_functions = collect_local_nongeneric_function_identities(program, type_info);
    let DeclarationSkeletons {
        enum_declarations,
        mut nominal_declarations,
        fieldless_enum_declarations,
        value_enum_declarations,
    } = collect_declaration_skeletons(program, &module_identity, type_info);
    let local_enum_declarations = collect_visible_enum_declarations(&enum_declarations, context);
    let local_receiver_methods = collect_local_receiver_methods(program, type_info);
    // Immutable values bind to retained layout skeletons before deferred defaults are attached.
    let constant_declarations = collect::collect_constants(program, type_info, &nominal_declarations);
    let published_constants = collect_published_constants(context, &constant_declarations);
    let mut local_nominal_declarations = nominal_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalNominalDeclarations>();
    let mut local_fieldless_enum_declarations = fieldless_enum_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalFieldlessEnumDeclarations>();
    let mut local_value_enum_declarations = value_enum_declarations
        .iter()
        .map(|declaration| (declaration.name.clone(), declaration.clone()))
        .collect::<LocalValueEnumDeclarations>();
    extend_published_nominal_declarations(
        context,
        &mut local_nominal_declarations,
        &mut local_fieldless_enum_declarations,
        &mut local_value_enum_declarations,
    );
    let published_callable_returns = collect::collect_published_callable_returns(context, type_info);
    let published_callable_defaults = collect::collect_published_callable_defaults(context);
    let published_newtype_returns = collect::collect_published_newtype_returns(context);
    let published_storage_types = collect::collect_published_storage_types(context);
    let local_trait_partials = trait_partials::collect(program, type_info);
    let published_methods = published_methods::collect(context);
    let runtime_type_conflicts = parameter_types::runtime_name_conflicts(context);
    let model_constants = model_constants::collect(program, type_info, &local_nominal_declarations);
    let lowering_facts = BodyIrLoweringFacts {
        model_constants: &model_constants,
        local_enum_declarations: &local_enum_declarations,
        published_methods: &published_methods,
        published_callable_returns: &published_callable_returns,
        published_callable_defaults: &published_callable_defaults,
        published_newtype_returns: &published_newtype_returns,
        published_storage_types: &published_storage_types,
        published_constants: &published_constants,
        type_info,
        function_default_sources: &function_default_sources,
        local_function_declarations: &local_function_declarations,
        local_nongeneric_functions: &local_nongeneric_functions,
        local_receiver_methods: &local_receiver_methods,
        local_trait_partials: &local_trait_partials,
        local_nominal_declarations: &local_nominal_declarations,
        local_fieldless_enum_declarations: &local_fieldless_enum_declarations,
        local_value_enum_declarations: &local_value_enum_declarations,
        runtime_type_conflicts: &runtime_type_conflicts,
        retain_nominal_carriers,
        module_identity: &module_identity,
        registry_package,
        provider_operations,
    };
    defaults::attach_nominal_field_defaults(program, &mut nominal_declarations, &lowering_facts);
    defaults::attach_method_partial_defaults(program, &mut nominal_declarations, &lowering_facts);
    let mut bodies = lower_source_bodies(program, &lowering_facts, include_inline_tests, observer);
    let mut static_declarations = collect_source_storage(program, &lowering_facts, &constant_declarations);
    let decorated_bindings =
        decorated_bindings::collect(program, &lowering_facts, &mut bodies, &mut static_declarations);
    for body in &bodies {
        if !type_info.reused_callable_bodies.contains_key(&body.span) {
            work::built_body(&body.direct_call_id);
        }
    }
    retain_source_body_contracts(program, type_info, &mut bodies);
    static_declarations.sort_by_key(|declaration| declaration.canonical.declaration_span.start);
    registry_effects::collect(program, &lowering_facts, &mut static_declarations);
    bir::BodyIrModule {
        runtime_callable_bindings: Vec::new(),
        string_conversion_contract: bir::StringConversionContract::Unselected,
        decorated_bindings,
        entrypoint_effects: entrypoint_effects::collect(program),
        program_entrypoints: Vec::new(),
        constant_declarations,
        type_alias_declarations: collect::collect_type_aliases(program, type_info),
        trait_declarations: collect::collect_local_trait_declarations(program, type_info),
        trait_implementations: collect::collect_local_trait_implementations(
            program,
            type_info,
            &nominal_declarations,
            &enum_declarations,
        ),
        enum_declarations,
        loaf_callables: rust_calls::declarations(type_info)
            .into_iter()
            .chain(runtime_nominals::members(type_info))
            .collect(),
        native_union_declarations: Vec::new(),
        static_declarations,
        module_id,
        nominal_declarations,
        fieldless_enum_declarations,
        value_enum_declarations,
        bodies,
    }
}

/// A module's model, nominal and enum layout skeletons: what other modules construct and match against, before its
/// deferred defaults, bodies and representation refinements exist.
pub(super) struct DeclarationSkeletons {
    pub(super) enum_declarations: Vec<bir::EnumDeclaration>,
    pub(super) nominal_declarations: Vec<bir::NominalDeclaration>,
    pub(super) fieldless_enum_declarations: Vec<bir::FieldlessEnumDeclaration>,
    pub(super) value_enum_declarations: Vec<bir::ValueEnumDeclaration>,
}

/// Collect a module's layout skeletons from its checked source alone; no other module's Body IR is consulted.
pub(super) fn collect_declaration_skeletons(
    program: &ast::Program,
    module_identity: &str,
    type_info: &TypeCheckInfo,
) -> DeclarationSkeletons {
    let mut enum_declarations = collect_local_enum_declarations(program, module_identity, type_info);
    enum_declarations.extend(rust_values::enum_declarations(type_info));
    let mut nominal_declarations = collect_local_nominal_declarations(program, module_identity, type_info);
    nominal_declarations.extend(rust_values::declarations(type_info));
    newtype_string_conversion::finalize_candidates(&mut nominal_declarations);
    DeclarationSkeletons {
        enum_declarations,
        nominal_declarations,
        fieldless_enum_declarations: collect_local_fieldless_enum_declarations(program, module_identity, type_info),
        value_enum_declarations: collect_local_value_enum_declarations(program, module_identity, type_info),
    }
}

/// The declarations-only module an import cycle's member shows the other members while their bodies are lowered: its
/// layout skeletons under its own module identity. Every member then sees every other member's models whatever order
/// the cycle's bodies are lowered in; the member's own complete module replaces this once it is lowered.
pub(super) fn build_cycle_declarations(
    program: &ast::Program,
    module_path: &[String],
    type_info: &TypeCheckInfo,
    library_package: Option<&str>,
) -> bir::BodyIrModule {
    let module_identity = match library_package {
        Some(package) => incan_semantics_core::package_module_identity(package, module_path),
        None => body_ir_module_identity(module_path),
    };
    let skeletons = collect_declaration_skeletons(program, &module_identity, type_info);
    let mut module = bir::BodyIrModule::declarations_only(module_identity);
    module.enum_declarations = skeletons.enum_declarations;
    module.nominal_declarations = skeletons.nominal_declarations;
    module.fieldless_enum_declarations = skeletons.fieldless_enum_declarations;
    module.value_enum_declarations = skeletons.value_enum_declarations;
    module
}

/// Collect explicit storage before computed constants; decoration and registry effects append their declarations
/// afterward, before the module's existing source-span ordering is applied.
pub(super) fn collect_source_storage(
    program: &ast::Program,
    facts: &BodyIrLoweringFacts<'_, '_>,
    constants: &[bir::ConstantDeclaration],
) -> Vec<bir::StaticDeclaration> {
    let mut declarations = collect::collect_statics(program, facts);
    declarations.extend(collect::collect_computed_constants(program, facts, constants));
    declarations
}

/// Preserve explicit input-contract refusals before retaining each body's executable representation facts.
/// These checks run after decoration has added its bodies and before module-wide representation passes.
pub(super) fn retain_source_body_contracts(
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    bodies: &mut [bir::Body],
) {
    apply_top_level_input_contract_refusal(program, bodies);
    registry_effects::refuse_unretained_functions(program, type_info, bodies);
    for body in bodies {
        representation_facts::retain(body);
    }
}

/// Index local and dependency enum declarations by the checked canonical identity, preserving dependency
/// overwrite precedence when the same identity is already present in the local inventory.
pub(super) fn collect_visible_enum_declarations(
    local: &[bir::EnumDeclaration],
    context: &[bir::BodyIrModule],
) -> HashMap<CanonicalSymbolId, bir::EnumDeclaration> {
    local
        .iter()
        .chain(context.iter().flat_map(|module| module.enum_declarations.iter()))
        .map(|declaration| (declaration.canonical.clone(), declaration.clone()))
        .collect()
}

/// Add dependency layouts under their existing compact canonical lookup keys. Local spelling entries stay
/// available alongside those qualified keys; this does not recreate identity from an unqualified name.
pub(super) fn extend_published_nominal_declarations(
    context: &[bir::BodyIrModule],
    nominals: &mut LocalNominalDeclarations,
    fieldless_enums: &mut LocalFieldlessEnumDeclarations,
    value_enums: &mut LocalValueEnumDeclarations,
) {
    for module in context {
        for declaration in &module.nominal_declarations {
            nominals.insert(declaration.canonical.render_compact(), declaration.clone());
        }
        for declaration in &module.fieldless_enum_declarations {
            fieldless_enums.insert(declaration.canonical.render_compact(), declaration.clone());
        }
        for declaration in &module.value_enum_declarations {
            value_enums.insert(declaration.canonical.render_compact(), declaration.clone());
        }
    }
}

/// Index dependency constants and the local immutable values whose retained layouts are available before
/// deferred defaults are attached. Other local constant forms keep their existing lowering path.
pub(super) fn collect_published_constants(
    context: &[bir::BodyIrModule],
    local_constants: &[bir::ConstantDeclaration],
) -> HashMap<CanonicalSymbolId, bir::Constant> {
    context
        .iter()
        .flat_map(|module| module.constant_declarations.iter())
        .chain(local_constants.iter().filter(|declaration| {
            matches!(
                declaration.value,
                bir::Constant::Int(_)
                    | bir::Constant::TypedNumeric(_)
                    | bir::Constant::FrozenAggregate(..)
                    | bir::Constant::NumericNewtype(_)
            )
        }))
        .map(|declaration| (declaration.canonical.clone(), declaration.value.clone()))
        .collect()
}

/// Lower executable declarations in source order using the module's already collected checked facts. Inline test
/// functions enter Body IR only for a test unit; ordinary program and library compilations must not carry their bodies
/// or test-only calls. Attribute externs and transparent native stubs supply compiler adapters rather than executable
/// source bodies.
pub(super) fn lower_source_bodies(
    program: &ast::Program,
    facts: &BodyIrLoweringFacts<'_, '_>,
    include_inline_tests: bool,
    observer: &mut dyn FnMut(
        &ast::FunctionDecl,
        ast::Span,
        &BodyIrLoweringFacts<'_, '_>,
        &bir::Body,
    ) -> Option<bir::Body>,
) -> Vec<bir::Body> {
    program
        .declarations
        .iter()
        .flat_map(|decl| -> Vec<bir::Body> {
            match &decl.node {
                ast::Declaration::Partial(partial) => {
                    vec![partials::lower_source_partial(partial, decl.span, facts)]
                }
                ast::Declaration::Function(function) => {
                    if let Some(body) = facts.type_info.reused_callable_bodies.get(&decl.span) {
                        return vec![body.clone()];
                    }
                    let body = lower_function_body(function, decl.span, facts);
                    let selected = observer(function, decl.span, facts, &body);
                    vec![selected.unwrap_or(body)]
                }
                ast::Declaration::TestModule(test_module) if include_inline_tests => test_module
                    .body
                    .iter()
                    .filter_map(|nested| match &nested.node {
                        ast::Declaration::Function(function) => {
                            let body = lower_function_body(function, nested.span, facts);
                            Some(observer(function, nested.span, facts, &body).unwrap_or(body))
                        }
                        _ => None,
                    })
                    .collect(),
                ast::Declaration::TestModule(_) => Vec::new(),
                ast::Declaration::Model(model) => lower_owner_member_bodies(
                    &model.methods,
                    &model.properties,
                    &model.name,
                    owner_self_type(&model.name, &model.type_params),
                    facts,
                ),
                ast::Declaration::Class(class) => lower_owner_member_bodies(
                    &class.methods,
                    &class.properties,
                    &class.name,
                    owner_self_type(&class.name, &class.type_params),
                    facts,
                ),
                ast::Declaration::Trait(trait_decl) => {
                    lower_owner_method_bodies(&trait_decl.methods, &trait_decl.name, IncanType::SelfType, facts)
                }
                ast::Declaration::Newtype(newtype) if is_direct_native_rusttype_alias(newtype, facts.type_info) => {
                    Vec::new()
                }
                ast::Declaration::Newtype(newtype) => lower_owner_method_bodies(
                    &newtype.methods,
                    &newtype.name,
                    owner_self_type(&newtype.name, &newtype.type_params),
                    facts,
                ),
                ast::Declaration::Enum(enum_decl) => lower_owner_method_bodies(
                    &enum_decl.methods,
                    &enum_decl.name,
                    owner_self_type(&enum_decl.name, &enum_decl.type_params),
                    facts,
                ),
                _ => Vec::new(),
            }
        })
        .collect()
}

/// Retain representation facts after every declaration and body exists. Pass order preserves source trait
/// annotations before opaque propagation and keeps ownership refinement after the resulting layouts are settled.
pub(super) fn retain_module_representations(
    module: &mut bir::BodyIrModule,
    program: &ast::Program,
    type_info: &TypeCheckInfo,
    context: &[bir::BodyIrModule],
    selected: &HashSet<CanonicalSymbolId>,
) {
    source_comparison::retain_equality_borrows(module, selected);
    let opaque_carriers = opaque_returns::collect(module, type_info);
    // Retain source trait annotations before local opaque propagation replaces return types with model witnesses.
    trait_values::retain_inputs(module, program, type_info, context, selected);
    trait_returns::retain_returns(module, program, type_info, context, selected);
    opaque_returns::retain(module, &opaque_carriers, selected);
    nominal_carriers::retain(module, type_info, context, selected);
    primitive_adapters::retain(module);
    union_layouts::retain(module);
}

/// Collect the identity-bearing leaves of `ty` with their structural paths below `path`, through generic arguments and
/// tuple items and references, as `ResultVariant::has_complete_canonical_types` walks them.
pub(super) fn named_leaf_paths(ty: &IncanType, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, RetainedTypeLeaf)>) {
    match ty {
        IncanType::Named(name) => out.push((path.clone(), RetainedTypeLeaf::Named(name.clone()))),
        IncanType::NativeBuiltin(builtin) => out.push((path.clone(), RetainedTypeLeaf::NativeBuiltin(builtin.clone()))),
        IncanType::Tuple(items) | IncanType::Generic { args: items, .. } => {
            for (index, item) in items.iter().enumerate() {
                path.push(index);
                named_leaf_paths(item, path, out);
                path.pop();
            }
        }
        IncanType::Ref(inner) | IncanType::RefMut(inner) | IncanType::NativeArray { element: inner, .. } => {
            path.push(0);
            named_leaf_paths(inner, path, out);
            path.pop();
        }
        _ => {}
    }
}

/// Make a broken caller's raw top-level vocabulary declaration fail every executable body rather than disappearing.
///
/// `BodyIrModule` intentionally remains a total lowering result, so it cannot return a module-level error. The
/// visible fail-closed representation is therefore an `Unsupported` marker at the original declaration span in
/// every lowered body. An executor selecting any source body stops before evaluating source statements, while a
/// declaration-only module still has no executable body to select.
pub(super) fn apply_top_level_input_contract_refusal(program: &ast::Program, bodies: &mut [bir::Body]) {
    let Some((description, span)) = program.declarations.iter().find_map(|declaration| {
        refusals::unsupported_top_level_declaration_label(&declaration.node)
            .map(|description| (description, hir_span(declaration.span)))
    }) else {
        return;
    };

    for body in bodies {
        body.block.stmts.insert(
            0,
            bir::Statement {
                kind: bir::StatementKind::Unsupported {
                    description: description.clone(),
                },
                span,
            },
        );
    }
}
