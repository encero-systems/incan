//! Validation of retained storage, module declarations, refused features and generic adoptions.

use super::*;

/// Refuse source storage whose checked layout was deliberately omitted from Body IR.
///
/// In particular, a checked newtype constructor must not reach body validation as if it were an ordinary raw wrapper.
/// Transparent rusttypes already passed declaration admission and introduce no physical wrapper to inventory here.
/// The frontend declaration collector owns that decision; this boundary only checks whether its layout was retained.
/// Decorated callable replacements share the storage table but are validated under their original function identity,
/// so they do not count as source `static` declarations.
pub(super) fn validate_retained_storage(
    program: &incan_frontend::ast::Program,
    module: &BodyIrModule,
) -> Result<(), String> {
    use incan_frontend::ast::Declaration;
    use incan_semantics_core::SemanticSourceTargetKind;

    let unretained = program
        .declarations
        .iter()
        .filter_map(|source| match &source.node {
            Declaration::Static(storage) => Some((storage, source.span)),
            _ => None,
        })
        .filter(|(storage, span)| {
            !module.static_declarations.iter().any(|declaration| {
                declaration.canonical.kind == SemanticSourceTargetKind::Static
                    && incan_semantics_core::canonical_module_identity(&declaration.canonical).as_deref()
                        == Some(module.module_id.path())
                    && storage.name == declaration.canonical.declaration_name
                    && span.start == declaration.canonical.declaration_span.start
                    && span.end == declaration.canonical.declaration_span.end
            })
        })
        .map(|(storage, _)| storage.name.as_str())
        .collect::<Vec<_>>();
    if !unretained.is_empty() {
        return Err(format!(
            "unsupported source Static initializer or carrier on the native route: `{}` in {}",
            unretained.join("`, `"),
            module.module_id.path()
        ));
    }
    let newtype_count = program
        .declarations
        .iter()
        .filter(|declaration| matches!(&declaration.node, Declaration::Newtype(item) if !item.is_rusttype))
        .count();
    let retained_newtypes = module
        .nominal_declarations
        .iter()
        .filter(|declaration| declaration.canonical.kind == SemanticSourceTargetKind::Newtype)
        .count();
    if newtype_count != retained_newtypes {
        return Err("unsupported source checked Newtype construction on the native route".to_owned());
    }
    Ok(())
}

/// Report each checked module's declaration refusal with its source owner, so one unsupported module does not hide
/// independent refusals in the same unit. No Body IR is built when any declaration is refused.
pub(super) fn validate_module_declarations(
    modules: &[ParsedModule],
    analysis: &CompilationAnalysis,
) -> Result<(), String> {
    let mut refusals = Vec::new();
    for module in modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .ok_or("module analysis is missing")?;
        if let Err(error) = validate_declarations(&module.ast, type_info) {
            refusals.push(format!("{}: {error}", module.file_path.display()));
        }
    }
    if refusals.is_empty() {
        return Ok(());
    }
    Err(refusals.join("\n"))
}

/// Refuse declarations whose observable behavior the direct route cannot retain, independently for each module.
pub(super) fn validate_declarations(
    program: &incan_frontend::ast::Program,
    type_info: &incan_frontend::typechecker::TypeCheckInfo,
) -> Result<(), String> {
    for declaration in &program.declarations {
        use incan_frontend::ast::Declaration;
        let kind = match &declaration.node {
            Declaration::Function(_) | Declaration::Docstring(_) | Declaration::Import(_) | Declaration::Const(_) => {
                continue;
            }
            Declaration::Static(_) => continue,
            Declaration::Model(model) => {
                if !incan_frontend::body_ir::is_direct_replacement_checked_model(model, type_info) {
                    return Err(format!(
                        "unsupported source Model {} on {} on the native route",
                        model_refusal_feature(model),
                        model.name
                    ));
                }
                validate_generic_adoptions(&model.traits, program, type_info)?;
                continue;
            }
            Declaration::Class(class) => {
                if type_info.c_abi.verified_bindings.contains(&class.name) {
                    continue;
                }
                if !incan_frontend::body_ir::is_direct_replacement_checked_class(class, type_info) {
                    return Err(format!(
                        "unsupported source Class {} on {} on the native route",
                        class_refusal_feature(class),
                        class.name
                    ));
                }
                validate_generic_adoptions(&class.traits, program, type_info)?;
                continue;
            }
            Declaration::Enum(value) => {
                if !incan_frontend::body_ir::is_direct_native_enum(value) {
                    return Err(format!("unsupported source Enum {} on the native route", value.name));
                }
                if !incan_frontend::body_ir::has_direct_native_enum_derives(value, type_info) {
                    return Err(format!(
                        "unsupported source Enum derives on {} on the native route",
                        value.name
                    ));
                }
                validate_generic_adoptions(&value.traits, program, type_info)?;
                continue;
            }
            Declaration::Trait(item) => {
                if let Some(feature) = trait_refusal_feature(item) {
                    return Err(format!("unsupported source Trait {feature} on the native route"));
                }
                continue;
            }
            Declaration::Newtype(newtype)
                if incan_frontend::body_ir::is_direct_native_rusttype_alias(newtype, type_info) =>
            {
                continue;
            }
            Declaration::Newtype(newtype)
                if incan_frontend::body_ir::is_direct_replacement_checked_newtype(newtype, type_info) =>
            {
                validate_generic_adoptions(&newtype.traits, program, type_info)?;
                continue;
            }
            Declaration::Newtype(newtype) => {
                return Err(format!(
                    "unsupported source nonplain Newtype {} on the native route",
                    newtype.name
                ));
            }
            Declaration::Alias(_) => continue,
            Declaration::Partial(_) => continue,
            // Type aliases have no runtime declaration; checked Body IR carries their resolved uses.
            Declaration::TypeAlias(_) => continue,
            // A capability declares authority, not code: consumers reach it through its checked identity.
            Declaration::Capability(_) => continue,
            // An inline test module is compiled by the test runner, never into the production unit.
            Declaration::TestModule(_) => continue,
            _ => "top-level declaration",
        };
        return Err(format!("unsupported source {kind} on the native route"));
    }
    Ok(())
}

/// Name the first unsupported model feature without relaxing the frontend's declaration admission predicate.
pub(super) fn model_refusal_feature(model: &incan_frontend::ast::ModelDecl) -> String {
    // ---- Member binding features ----
    if !incan_frontend::body_ir::has_direct_replacement_method_partials(
        &model.type_params,
        &model.methods,
        &model.method_aliases,
        &model.method_partials,
    ) {
        return "method partials".to_owned();
    }
    if model.properties.iter().any(|property| property.node.body.is_none()) {
        return "abstract properties".to_owned();
    }

    // ---- Method facts ----
    if has_refused_method_decorator(&model.methods, false) {
        return "decorated methods".to_owned();
    }

    // ---- Decorator expansion ----
    refused_decorator_feature(&model.decorators).unwrap_or_else(|| "unsupported declaration shape".to_owned())
}

/// Name the first unsupported class feature without relaxing the frontend's declaration admission predicate.
pub(super) fn class_refusal_feature(class: &incan_frontend::ast::ClassDecl) -> String {
    // ---- Structural declaration features ----
    if class.extends.is_some() {
        return "inheritance".to_owned();
    }
    // ---- Member binding features ----
    if !incan_frontend::body_ir::has_direct_replacement_method_partials(
        &class.type_params,
        &class.methods,
        &class.method_aliases,
        &class.method_partials,
    ) {
        return "method partials".to_owned();
    }
    if class.properties.iter().any(|property| property.node.body.is_none()) {
        return "abstract properties".to_owned();
    }
    if !class.declarative_members.is_empty() {
        return "declarative members".to_owned();
    }

    // ---- Method facts ----
    if has_refused_method_decorator(&class.methods, false) {
        return "decorated methods".to_owned();
    }

    // ---- Decorator expansion ----
    refused_decorator_feature(&class.decorators).unwrap_or_else(|| "unsupported declaration shape".to_owned())
}

/// Report executable method decorators outside the admitted builtin markers. A trait may also retain a plain
/// `@rust.extern` abstract slot: it has no body or initialization effect, and using it still requires a checked
/// implementation or native intrinsic. Models and classes receive no such exemption.
pub(super) fn has_refused_method_decorator(
    methods: &[incan_frontend::ast::Spanned<incan_frontend::ast::MethodDecl>],
    allow_abstract_extern: bool,
) -> bool {
    methods.iter().any(|method| {
        method.node.decorators.iter().any(|decorator| {
            if incan_frontend::body_ir::is_direct_replacement_method_decorator(&decorator.node) {
                return false;
            }
            !(allow_abstract_extern
                && method.node.body.is_none()
                && decorator.node.args.is_empty()
                && decorator.node.type_args.is_empty()
                && incan_lang::lang::decorators::from_segments(&decorator.node.path.segments)
                    == Some(incan_lang::lang::decorators::DecoratorId::RustExtern))
        })
    })
}

/// Name the first declaration decorator outside the bare builtin derive spelling, with its identifier arguments.
pub(super) fn refused_decorator_feature(
    decorators: &[incan_frontend::ast::Spanned<incan_frontend::ast::Decorator>],
) -> Option<String> {
    let decorator = decorators.iter().find(|decorator| {
        !incan_frontend::body_ir::is_direct_replacement_model_derive(&decorator.node)
            && !incan_frontend::body_ir::is_lint_suppression_decorator(&decorator.node)
    })?;
    let arguments = decorator
        .node
        .args
        .iter()
        .filter_map(|argument| match argument {
            incan_frontend::ast::DecoratorArg::Positional(value) => match &value.node {
                incan_frontend::ast::Expr::Ident(name) => Some(name.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("decorator @{}({arguments})", decorator.node.name))
}

/// Admit builtin trait markers with no executable decorator behavior. The checker validates required field types
/// and each adopter's conformance before admission; default method bodies retain their checked field accesses.
pub(super) fn retained_trait_marker(decorator: &incan_frontend::ast::Decorator) -> bool {
    let id = incan_lang::lang::decorators::from_segments(&decorator.path.segments);
    if id == Some(incan_lang::lang::decorators::DecoratorId::Requires)
        && decorator.type_args.is_empty()
        && decorator
            .args
            .iter()
            .all(|argument| matches!(argument, DecoratorArg::Named(_, DecoratorArgValue::Type(_))))
    {
        return true;
    }
    id == Some(incan_lang::lang::decorators::DecoratorId::RustDerive)
        && decorator.type_args.is_empty()
        && !decorator.args.is_empty()
        && decorator.args.iter().all(|argument| matches!(argument,
            DecoratorArg::Positional(value) if matches!(&value.node,
                Expr::Literal(Literal::String(path)) if matches!(derives::from_str(path), Some(DeriveId::Debug | DeriveId::Clone)) || matches!(path.as_str(), "serde::Serialize" | "serde::Deserialize"))))
}

/// Name the first trait feature the direct route cannot retain.
///
/// Type parameters and supertraits declared in this module are retained: Body IR records each adopter's checked
/// instantiation and the supertrait slots it fills. A method's own type parameters are retained like any method's:
/// each checked call schedules the adopter's default at its call-site arguments. A supertrait from elsewhere adds no
/// declaration behavior of its own: the checker records the serde derives a standard JSON supertrait implies, and a
/// call of one of its slots that has no retained body refuses by name where it is lowered.
/// Literal presets on source-local method partials become checked call arguments; computed presets remain refused until
/// they have a declaration-owned execution frame.
pub(super) fn trait_refusal_feature(item: &incan_frontend::ast::TraitDecl) -> Option<&'static str> {
    if item
        .decorators
        .iter()
        .any(|decorator| !retained_trait_marker(&decorator.node))
    {
        return Some("decorators");
    }
    if !item.method_aliases.is_empty() {
        return Some("method aliases");
    }
    if !incan_frontend::body_ir::has_direct_replacement_literal_trait_partials(item) {
        return Some("method partials");
    }
    if item.properties.iter().any(|property| property.node.body.is_some()) {
        return Some("property defaults");
    }
    // Ordinary markers follow nominal rules; a trait's abstract extern slot carries no executable decorator effect.
    if has_refused_method_decorator(&item.methods, true) {
        return Some("decorated methods");
    }
    None
}

/// Whether this module declares a trait under `name`.
pub(super) fn declares_trait(program: &incan_frontend::ast::Program, name: &str) -> bool {
    program.declarations.iter().any(
        |declaration| matches!(&declaration.node, incan_frontend::ast::Declaration::Trait(item) if item.name == name),
    )
}

/// Refuse a generic adoption of a trait this module does not declare, unless the standard library declares it.
///
/// Body IR instantiates only source-local trait defaults; another module's generic trait's implementation facts are
/// not retained, so admitting the adoption could lose behavior. A standard-library or SDK protocol's slots are retained
/// where the adopter's own method fills them, and a call that needs one of its defaults refuses by name where lowered.
pub(super) fn validate_generic_adoptions(
    adoptions: &[incan_frontend::ast::Spanned<incan_frontend::ast::TraitBound>],
    program: &incan_frontend::ast::Program,
    type_info: &incan_frontend::typechecker::TypeCheckInfo,
) -> Result<(), String> {
    if adoptions.iter().any(|adoption| {
        !adoption.node.type_args.is_empty()
            && !declares_trait(program, &adoption.node.name)
            && !type_info.traits.is_standard_trait(&adoption.node.name)
    }) {
        return Err("unsupported source generic adoption of an imported trait on the native route".to_owned());
    }
    Ok(())
}
