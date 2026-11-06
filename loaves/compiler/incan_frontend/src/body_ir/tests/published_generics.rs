//! Declaration-owned generic types remain checked when executable package fragments are published.

use std::collections::BTreeSet;
use std::sync::Arc;

use incan_semantics_core::body_ir::BodyIrModule;
use incan_semantics_core::executable_representation::{
    CoverageReason, DeclarationCoverage, ExecutableDeclaration, SurfaceReader, build_surface,
};
use incan_semantics_core::{CanonicalSymbolId, IncanType};

use super::{body_named, build, build_body_ir_module_v0};
use crate::provider::ProviderPlan;
use crate::typechecker::TypeChecker;
use crate::{ast, lexer, parser};

const SOURCE: &str = r#"pub model Holder[T]:
    pub value: T
    pub marker: Option[T] = None

    def get(self) -> T:
        return self.value

pub trait Wrap[T]:
    def wrap(self, value: T) -> Holder[T]:
        return Holder[T](value=value)

pub def saved[T](value: T) -> T:
    return value
"#;

/// Check real package-owned source before auditing its executable publication rather than forging generic layouts.
fn checked() -> Result<BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(SOURCE).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_package_identity(Some("publication_probe".to_owned()));
    checker.set_current_module_path(Some(vec!["lib".to_owned()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok(build_body_ir_module_v0(
        &program,
        &["lib".to_owned()],
        checker.type_info(),
    ))
}

/// The probe exposes each actual declaration and field; publication still owns recursive type and reference checks.
fn publish(module: BodyIrModule) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    publish_modules(&[module], &BTreeSet::new())
}

/// Publish the actual declaring modules together so a checked imported trait method is present in the public closure.
fn publish_modules(
    modules: &[BodyIrModule],
    public_members: &BTreeSet<CanonicalSymbolId>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut public = public_members.clone();
    for module in modules {
        public.extend(module.trait_declarations.iter().cloned());
        for nominal in &module.nominal_declarations {
            public.insert(nominal.canonical.clone());
            public.extend(nominal.field_identities.iter().cloned());
        }
        public.extend(module.bodies.iter().filter_map(|body| body.canonical.clone()));
    }
    Ok(build_surface(
        modules,
        "publication_probe",
        "1.0.0",
        &public,
        &BTreeSet::new(),
    )?)
}

/// Parse the actual callable declarations shared by SDK source checking and the publication closure.
fn callable_source() -> Result<ast::Program, Box<dyn std::error::Error>> {
    let source = include_str!("../../../../../stdlib/core/src/traits/callable.incn");
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    Ok(parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?)
}

/// Check the real callable module under its physical SDK path, preserving its own declaration identities.
fn checked_callable_context() -> Result<(BodyIrModule, BTreeSet<CanonicalSymbolId>), Box<dyn std::error::Error>> {
    let program = callable_source()?;
    let path = ["traits", "callable"].map(str::to_owned);
    let mut checker = TypeChecker::new();
    checker.set_standard_library_source(true);
    checker.set_current_package_identity(Some("publication_probe".to_owned()));
    checker.set_current_module_path(Some(path.to_vec()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let api = crate::api_metadata::collect_checked_api_metadata(&program, &checker, path.to_vec());
    let mut public = BTreeSet::new();
    for declaration in api.declarations {
        if let crate::api_metadata::ApiDeclaration::Trait(owner) = declaration {
            for method in owner.methods {
                public.insert(
                    method
                        .canonical
                        .as_ref()
                        .and_then(|identity| identity.hydrate())
                        .ok_or("checked public callable method identity absent")?,
                );
            }
        }
    }
    Ok((build_body_ir_module_v0(&program, &path, checker.type_info()), public))
}

/// A generic SDK-like carrier retains its declared fields and actual owner-scoped default frame through the codec.
#[test]
fn published_generic_nominal_keeps_its_default_frame() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked()?;
    let owner = module.nominal_declarations.first().ok_or("generic owner absent")?;
    let identity = owner.canonical.clone();
    let getter = body_named(&module, "get")?;
    assert!(getter.type_parameters.is_empty());
    let getter_id = getter.canonical.clone().ok_or("generic getter identity absent")?;
    let defaults = owner.field_default_body.clone().ok_or("generic default frame absent")?;
    let fields = owner.field_types.clone();
    let bytes = publish(module)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        reader.covers(&getter_id),
        "getter needs its exact enclosing owner's binder"
    );
    let ExecutableDeclaration::Nominal(decoded) = reader.fragment(&identity)? else {
        return Err("generic owner is not a published nominal".into());
    };
    assert_eq!(decoded.type_parameters, ["T"]);
    assert_eq!(decoded.field_types, fields);
    let projected = decoded
        .field_default_body
        .ok_or("published generic default frame absent")?;
    assert_eq!(projected.canonical, defaults.canonical);
    assert_eq!(projected.type_parameters, defaults.type_parameters);
    assert_eq!(projected.return_type, defaults.return_type);
    assert_eq!(projected.first_representation_gap(), None);
    Ok(())
}

/// The concrete return keeps its carrier and enclosing trait as explicit requirements of the published default.
#[test]
fn published_trait_factory_keeps_its_declaring_owner() -> Result<(), Box<dyn std::error::Error>> {
    let module = checked()?;
    let body = body_named(&module, "wrap")?;
    assert_eq!(body.first_representation_gap(), None);
    let identity = body.canonical.clone().ok_or("factory identity absent")?;
    let trait_owner = module.trait_declarations.first().ok_or("factory trait absent")?.clone();
    let carrier = module
        .nominal_declarations
        .first()
        .ok_or("factory carrier absent")?
        .canonical
        .clone();
    let result = body.return_type.clone();
    let bytes = publish(module)?;
    let reader = SurfaceReader::open(&bytes)?;
    let DeclarationCoverage::Covered { requirements, .. } = reader.index().coverage(&identity)? else {
        return Err("generic trait factory is uncovered".into());
    };
    assert!(requirements.contains(&trait_owner), "{requirements:?}");
    assert!(requirements.contains(&carrier), "{requirements:?}");
    let decoded = reader.declaration(&identity)?;
    assert_eq!(decoded.return_type, result);
    assert_eq!(decoded.first_representation_gap(), None);
    Ok(())
}

/// A concrete-looking generic use grants neither a foreign field binder nor a missing callable declaration binder.
#[test]
fn published_open_type_requires_its_declared_binder() -> Result<(), Box<dyn std::error::Error>> {
    let mut module = checked()?;
    let owner = module.nominal_declarations.first_mut().ok_or("generic owner absent")?;
    let owner_id = owner.canonical.clone();
    owner.field_types[0] = IncanType::TypeVar("Foreign".to_owned());
    let saved = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "saved")
        .ok_or("generic function absent")?;
    let saved_id: CanonicalSymbolId = saved.canonical.clone().ok_or("generic function identity absent")?;
    saved.type_parameters.clear();
    let getter = module
        .bodies
        .iter_mut()
        .find(|body| body.name == "get")
        .ok_or("generic getter absent")?;
    let getter_id = getter.canonical.as_mut().ok_or("generic getter identity absent")?;
    getter_id.declaration_span = incan_semantics_core::HirSourceSpan::new(SOURCE.len() + 100, SOURCE.len() + 101);
    let unrelated_getter = getter_id.clone();
    let bytes = publish(module)?;
    let reader = SurfaceReader::open(&bytes)?;
    for identity in [owner_id, saved_id, unrelated_getter] {
        assert!(matches!(
            reader.index().coverage(&identity)?,
            DeclarationCoverage::Uncovered(CoverageReason::UnresolvedReference)
        ));
    }
    Ok(())
}

/// The actual stdlib consumer bodies retain executable publication rather than stranding every inherited carrier.
#[test]
fn checked_collection_consumers_keep_publication() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("../../../../../stdlib/core/src/derives/collection.incn");
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let mut program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    program.declarations.retain(|declaration| match &declaration.node {
        ast::Declaration::Trait(owner) => owner.name == "FallibleIterator",
        ast::Declaration::Import(import) => matches!(&import.kind,
            ast::ImportKind::From { module, .. } if module.segments == ["std", "traits", "callable"]
        ),
        _ => false,
    });
    let owner = program
        .declarations
        .iter_mut()
        .find_map(|declaration| match &mut declaration.node {
            ast::Declaration::Trait(owner) => Some(owner),
            _ => None,
        })
        .ok_or("stdlib fallible trait absent")?;
    owner
        .methods
        .retain(|method| matches!(method.node.name.as_str(), "__next__" | "collect" | "fold"));
    assert_eq!(owner.methods.len(), 3);
    let mut checker = TypeChecker::new();
    let callable = callable_source()?;
    let path = ["derives", "collection"].map(str::to_owned);
    checker.set_current_package_identity(Some("publication_probe".to_owned()));
    checker.set_current_module_path(Some(path.to_vec()));
    checker
        .register_dependency_module_path_segments("traits_callable", vec!["traits".to_owned(), "callable".to_owned()]);
    checker.set_provider_plan(Arc::new(
        ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["derives".to_owned(), "traits".to_owned()]),
    ));
    checker
        .check_with_imports_allow_private(&program, &[("traits_callable", &callable)])
        .map_err(|errors| format!("{errors:?}"))?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let (callable, public_members) = checked_callable_context()?;
    let bytes = publish_modules(&[module.clone(), callable], &public_members)?;
    let reader = SurfaceReader::open(&bytes)?;
    let mut uncovered = Vec::new();
    for name in ["collect", "fold"] {
        let body = body_named(&module, name)?;
        assert_eq!(
            body.first_representation_gap(),
            None,
            "{name}: {}",
            body.render_snapshot()
        );
        let identity = body.canonical.as_ref().ok_or("stdlib consumer identity absent")?;
        if !reader.covers(identity) {
            uncovered.push(name);
            eprintln!("uncovered {name}: {:?}\n{body:#?}", reader.index().coverage(identity)?);
        }
    }
    assert!(uncovered.is_empty(), "actual stdlib consumers uncovered: {uncovered:?}");
    Ok(())
}

/// A checked source Some variant keeps its identity while the actual Option carrier names the builtin constructor.
#[test]
fn checked_some_patterns_keep_their_declaring_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"enum Maybe:
    Some(int)

def source_some(value: Maybe) -> int:
    match value:
        Maybe.Some(found) => return found

def builtin_some(value: Option[int]) -> bool:
    match value:
        Some(_) => return true
        None => return false
"#;
    let module = build(source, &["some_constructor_identity"])?;
    for (name, builtin) in [("source_some", false), ("builtin_some", true)] {
        let pattern = body_named(&module, name)?
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                incan_semantics_core::body_ir::StatementKind::Assign {
                    rvalue: incan_semantics_core::body_ir::Rvalue::Match { arms, .. },
                    ..
                } => arms.first().map(|arm| &arm.pattern),
                _ => None,
            })
            .ok_or("checked Some pattern absent")?;
        let incan_semantics_core::body_ir::Pattern::Enum {
            canonical: Some(identity),
            ..
        } = pattern
        else {
            return Err(format!("{name} has no checked constructor identity: {pattern:?}").into());
        };
        assert_eq!(
            identity.origin == incan_semantics_core::SymbolOrigin::Builtin,
            builtin,
            "{name}: {identity:?}"
        );
        if builtin {
            assert_eq!(
                identity,
                &crate::symbols::canonical_builtin_constructor_identity(
                    incan_lang::lang::surface::constructors::ConstructorId::Some
                )
            );
        } else {
            assert_eq!(identity.kind, incan_semantics_core::SemanticSourceTargetKind::Variant);
            assert_eq!(identity.declaration_name, "Some");
        }
    }
    Ok(())
}
