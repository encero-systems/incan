//! Canonical symbol identity through lowering (RFC 120) and the published round trip (RFC 123): one declaration keeps
//! one identity across local, imported, aliased and re-exported calls; same-named declarations along import chains and
//! byte-identical modules stay distinct; overloads, facades, multi-hop re-exports and sibling-relative imports resolve
//! to the owning declaration; canonical local and global roots, imported alias globals and rejected const writes keep
//! their targets.

use super::*;

/// Lower a module that imports from other modules, declaring each dependency's flattened cache name *and* its
/// real path segments.
///
/// Both are required because the flattened name is not injective: `("pkg_helpers", &["pkg", "helpers"], ..)` and
/// `("pkg_helpers", &["pkg_helpers"], ..)` are different modules sharing one cache key, and only the segments say
/// which one a fixture means.
fn build_with_imports(
    source: &str,
    module_path: &[&str],
    imports: &[(&str, &[&str], &str)],
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let mut import_programs = Vec::new();
    for (name, segments, import_source) in imports {
        let tokens = lexer::lex(import_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
        import_programs.push((*name, *segments, program));
    }
    let import_refs: Vec<(&str, &ast::Program)> = import_programs
        .iter()
        .map(|(name, _, program)| (*name, program))
        .collect();

    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    for (name, segments, _) in &import_programs {
        checker.register_dependency_module_path_segments(
            name,
            segments.iter().map(|segment| segment.to_string()).collect(),
        );
    }
    checker
        .check_with_imports(&program, &import_refs)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

#[test]
fn a_real_lowered_module_survives_the_published_round_trip_exactly() -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::executable_representation::{decode_module, encode_module};

    // RFC 123's feasibility rests on this claim and no weaker one: what a consumer decodes is the module the
    // declaring compilation encoded, not an equivalent reconstruction of it. A hand-built fixture cannot show that,
    // because it exercises only the shapes whoever wrote it remembered. This lowers real source through the real
    // typechecker and compares the whole module.
    let module = build(
        "model Point:\n\
        \x20   x: int\n\
        \x20   y: int\n\
         \n\
         enum Status:\n\
        \x20   Active\n\
        \x20   Retired\n\
         \n\
         def distance_squared(p: Point) -> int:\n\
        \x20   return p.x * p.x + p.y * p.y\n\
         \n\
         def classify(value: int) -> str:\n\
        \x20   if value > 10:\n\
        \x20       return \"large\"\n\
        \x20   elif value > 0:\n\
        \x20       return \"small\"\n\
        \x20   return \"none\"\n\
         \n\
         def accumulate(limit: int) -> int:\n\
        \x20   mut total = 0\n\
        \x20   for value in 0..limit:\n\
        \x20       total += value\n\
        \x20   return total\n",
        &["probe"],
    )?;
    assert!(
        !module.bodies.is_empty() && !module.nominal_declarations.is_empty(),
        "the probe must actually lower something for the round trip to mean anything: {module:#?}"
    );

    let encoded = encode_module(&module)?;
    let decoded = decode_module(&encoded)?;

    assert_eq!(
        decoded, module,
        "a decoded representation must equal the module that was encoded, field for field"
    );
    Ok(())
}

#[test]
fn imported_callable_without_call_site_identity_does_not_recover_authority_from_its_binding_name()
-> Result<(), Box<dyn std::error::Error>> {
    let dependency_source = "pub def helper() -> int:\n  return 42\n";
    let source = "from helpers import helper\n\ndef main() -> int:\n  return helper()\n";
    let dependency_tokens =
        lexer::lex(dependency_source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let dependency =
        parser::parse(&dependency_tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let module_path = vec!["app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_with_imports(&program, &[("helpers", &dependency)])
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    assert!(
        checker.type_info().resolved_import_identity("helper").is_some(),
        "the fixture must retain import-binding metadata independently of the call-site fact"
    );
    let mut type_info = checker.type_info().clone();
    type_info.references.resolved_identities.clear();

    let module = build_body_ir_module_v0(&program, &module_path, &type_info);
    let target = named_targets(&module, "main")
        .into_iter()
        .next()
        .ok_or("missing imported helper call")?;
    assert_eq!(target.canonical, None);
    assert_eq!(target.direct_call_id, None);
    assert_eq!(target.builtin, None);
    Ok(())
}

#[test]
fn canonical_local_and_global_roots_survive_body_ir_lowering() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
const LIMIT: int = 3
static COUNT: int = 0

def update(value: int) -> int:
  mut local = value
  COUNT += local
  return LIMIT + COUNT
"#;
    let module = build(source, &["app"])?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "update")
        .ok_or("missing update body")?;
    assert!(
        body.locals
            .iter()
            .filter(|local| !matches!(local.origin, bir::LocalOrigin::Temporary))
            .all(|local| local.identity.is_some()),
        "every source local and parameter must retain its canonical declaration identity: {:?}",
        body.locals
    );
    assert!(
        !body
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "resolved globals must not degrade to External locals: {:?}",
        body.locals
    );
    let snapshot = body.render_snapshot();
    assert!(
        snapshot.contains("@const:app::LIMIT@"),
        "missing canonical const root: {snapshot}"
    );
    assert!(
        snapshot.contains("@static:app::COUNT@"),
        "missing canonical static root: {snapshot}"
    );
    Ok(())
}

#[test]
fn imported_alias_globals_keep_the_provider_identity() -> Result<(), Box<dyn std::error::Error>> {
    let provider = r#"
pub const LIMIT: int = 3
pub static COUNT: int = 1
"#;
    let consumer = r#"
from provider import LIMIT as maximum, COUNT as current

def read() -> int:
  return maximum + current
"#;
    let module = build_with_imports(consumer, &["consumer"], &[("provider", &["provider"], provider)])?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "read")
        .ok_or("missing read body")?;
    let snapshot = body.render_snapshot();
    assert!(
        snapshot.contains("@const:provider::LIMIT@"),
        "alias lost provider const identity: {snapshot}"
    );
    assert!(
        snapshot.contains("@static:provider::COUNT@"),
        "alias lost provider static identity: {snapshot}"
    );
    assert!(
        !snapshot.contains("maximum") && !snapshot.contains("current"),
        "global identity must not be reconstructed from the consumer alias: {snapshot}"
    );
    Ok(())
}

#[test]
fn rejected_const_write_keeps_its_canonical_target_and_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
const LIMIT: int = 3

def overwrite() -> int:
  LIMIT = 4
  return LIMIT
"#;
    let (module, diagnostics) = build_after_expected_typecheck_errors(source, &["app", "const_write"])?;
    assert!(
        diagnostics.iter().any(|message| message.contains("LIMIT")),
        "the source checker must reject the const write: {diagnostics:?}"
    );
    let body = body_named(&module, "overwrite")?;
    assert!(
        !body
            .locals
            .iter()
            .any(|local| matches!(local.origin, bir::LocalOrigin::External)),
        "a rejected canonical const target must not degrade to an External local: {:?}",
        body.locals
    );
    assert!(
        body.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Unsupported { description }
                if description.contains("const:app::const_write::LIMIT")
                    && description.contains("not writable")
        )),
        "the rejected write must retain the const identity in its explicit refusal: {}",
        body.render_snapshot()
    );
    Ok(())
}

/// RFC 120's guide-level example: one declaration reached four ways is one identity.
///
/// A local call, a plain import, an import alias, and a re-export through a facade are all *bindings to* one
/// declaration. None of them creates a second identity for the thing it names, and the facade in particular must
/// not be recorded as an owner of what it merely re-exports.
#[test]
fn one_declaration_keeps_one_identity_across_local_imported_aliased_and_reexported_calls()
-> Result<(), Box<dyn std::error::Error>> {
    let helpers_source = r#"
pub def render() -> int:
  return 1

def use_local() -> int:
  return render()
"#;
    let facade_source = r#"
from helpers import render
"#;
    let app_source = r#"
from helpers import render
from helpers import render as draw
from facade import render as relayed

def use_imported() -> int:
  return render()

def use_alias() -> int:
  return draw()

def use_reexport() -> int:
  return relayed()
"#;
    let helpers = build(helpers_source, &["helpers"])?;
    let app = build_with_imports(
        app_source,
        &["app"],
        &[
            ("helpers", &["helpers"], helpers_source),
            ("facade", &["facade"], facade_source),
        ],
    )?;

    let mut facts = Vec::new();
    for (module, body) in [
        (&helpers, "use_local"),
        (&app, "use_imported"),
        (&app, "use_alias"),
        (&app, "use_reexport"),
    ] {
        let targets = named_targets(module, body);
        let [target] = targets.as_slice() else {
            return Err(Box::from(format!(
                "expected one named call in `{body}`, got {}",
                targets.len()
            )));
        };
        let Some(fact) = &target.canonical else {
            return Err(Box::from(format!("`{body}` must carry a canonical identity")));
        };
        facts.push((body, target.name.clone(), fact.clone()));
    }

    // One declaration, one identity, however each call site spelled it.
    let (_, _, first) = &facts[0];
    for (body, _, fact) in &facts {
        assert_eq!(fact, first, "`{body}` must resolve to the one declaration identity");
    }

    // The identity describes the declaration, never the reference.
    assert_eq!(first.declaration_name, "render");
    assert_eq!(first.kind, SemanticSourceTargetKind::Function);
    assert_eq!(first.namespace, incan_semantics_core::SymbolNamespace::OrdinaryLexical);
    assert_eq!(
        first.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["helpers".to_string()]),
        "the origin is the declaring module, never the importing or re-exporting one"
    );
    assert_eq!(
        first.scope_discriminant, None,
        "a module-level declaration is unique within its origin"
    );

    // It anchors to the one declaration site, not to any call site.
    let render_body = helpers
        .bodies
        .iter()
        .find(|body| body.name == "render")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("lowered `render` body missing"))?;
    assert_eq!(first.declaration_span, render_body.span);

    // The call-site spellings genuinely differ; only the identity collapses them.
    let spellings: Vec<&str> = facts.iter().map(|(_, name, _)| name.as_str()).collect();
    assert_eq!(spellings, vec!["render", "render", "draw", "relayed"]);
    Ok(())
}

/// A nested facade resolves its own relative re-export against *its* module, not the consumer's.
///
/// `pkg.facade` writing `from helpers import render` means `pkg.helpers`, because a sibling-relative candidate is
/// tried before the bare one from `pkg.facade`. Resolving that link from the consumer instead binds the root
/// `helpers`, and since the cache and the identity both resolved it, they would agree on the wrong declaration.
/// Distinguished by arity so the binding itself proves which module won.
#[test]
fn a_nested_facade_reexport_resolves_against_the_facade_not_the_consumer() -> Result<(), Box<dyn std::error::Error>> {
    let root_helpers = r#"
pub def render(first: int, second: int) -> int:
  return first + second
"#;
    let nested_helpers = r#"
pub def render() -> int:
  return 1
"#;
    let facade_source = r#"
from helpers import render
"#;
    let app_source = r#"
from pkg.facade import render

def run() -> int:
  return render()
"#;
    let app = build_with_imports(
        app_source,
        &["app"],
        &[
            ("helpers", &["helpers"], root_helpers),
            ("pkg_helpers", &["pkg", "helpers"], nested_helpers),
            ("pkg_facade", &["pkg", "facade"], facade_source),
        ],
    )?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from("the re-exported call must carry an identity".to_string()));
    };

    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["pkg".to_string(), "helpers".to_string()]),
        "the facade's own module decides what its relative re-export means"
    );

    // The span must be the nested declaration's, not the root's identically-named one.
    let nested = build(nested_helpers, &["pkg", "helpers"])?;
    let nested_render = nested
        .bodies
        .iter()
        .find(|body| body.name == "render")
        .ok_or_else(|| Box::<dyn std::error::Error>::from("nested `render` body missing"))?;
    assert_eq!(fact.declaration_span, nested_render.span);

    let root = build(root_helpers, &["helpers"])?;
    if let Some(root_render) = root.bodies.iter().find(|body| body.name == "render") {
        assert_ne!(
            fact.declaration_span, root_render.span,
            "the root module's `render` is a different declaration"
        );
    }
    Ok(())
}

/// Same-named declarations cross-imported along an acyclic chain: `a` <- `b` <- `c` <- `d`.
///
/// Module `c` ends up with three `make` declarations in scope at once — its own, `a`'s, and `b`'s — reached under
/// three spellings. Each must resolve to its own declaration, and `d` must agree with `c` about which is which.
#[test]
fn same_named_declarations_cross_imported_along_a_chain_stay_distinct() -> Result<(), Box<dyn std::error::Error>> {
    let a_source = r#"
pub def make() -> int:
  return 1
"#;
    let b_source = r#"
from a import make as make_a

pub def make() -> int:
  return 2

def use_all() -> int:
  return make() + make_a()
"#;
    let c_source = r#"
from a import make as make_a
from b import make as make_b

pub def make() -> int:
  return 3

def use_all() -> int:
  return make() + make_a() + make_b()
"#;
    let d_source = r#"
from a import make as make_a
from b import make as make_b
from c import make as make_c

def use_all() -> int:
  return make_a() + make_b() + make_c()
"#;

    fn origins(
        module: &bir::BodyIrModule,
        body: &str,
    ) -> Result<Vec<incan_semantics_core::SymbolOrigin>, Box<dyn std::error::Error>> {
        let mut found = Vec::new();
        for target in named_targets(module, body) {
            let Some(fact) = &target.canonical else {
                return Err(Box::from(format!("a call in `{body}` carried no identity")));
            };
            assert_eq!(fact.declaration_name, "make");
            found.push(fact.origin.clone());
        }
        found.sort();
        Ok(found)
    }
    let module_origin = |name: &str| incan_semantics_core::SymbolOrigin::Module(vec![name.to_string()]);

    let b = build_with_imports(b_source, &["b"], &[("a", &["a"], a_source)])?;
    assert_eq!(
        origins(&b, "use_all")?,
        vec![module_origin("a"), module_origin("b")],
        "`b` sees its own `make` and `a`'s as different declarations"
    );

    let c = build_with_imports(c_source, &["c"], &[("a", &["a"], a_source), ("b", &["b"], b_source)])?;
    assert_eq!(
        origins(&c, "use_all")?,
        vec![module_origin("a"), module_origin("b"), module_origin("c")],
        "`c` holds three same-named declarations at once and must keep them apart"
    );

    let d = build_with_imports(
        d_source,
        &["d"],
        &[
            ("a", &["a"], a_source),
            ("b", &["b"], b_source),
            ("c", &["c"], c_source),
        ],
    )?;
    assert_eq!(
        origins(&d, "use_all")?,
        vec![module_origin("a"), module_origin("b"), module_origin("c")],
        "a consumer that only imports must agree with `c` about which declaration is which"
    );
    Ok(())
}

/// Three modules with byte-identical contents, consumed together.
///
/// Because the sources are identical, `declaration_name`, `kind`, and `declaration_span` are identical across all
/// three `make` declarations, so `origin` is the only field that can separate them. If origin were dropped, wrong,
/// or recovered from a spelling, the three would collapse into one identity — and a consumer would dispatch a call
/// on `a` to the declaration in `c`.
///
/// It also pins the converse: one declaration reached from two modules under two spellings stays one identity.
#[test]
fn identical_modules_consumed_together_keep_three_distinct_identities() -> Result<(), Box<dyn std::error::Error>> {
    // One source text, used verbatim for `a`, `b`, and `c`.
    let shared_source = r#"
pub model Item:
  value: int

pub def make() -> int:
  return 1

def use_local() -> int:
  return make()
"#;
    let consumer_source = r#"
from a import make as make_a
from b import make as make_b
from c import make as make_c

def use_a() -> int:
  return make_a()

def use_b() -> int:
  return make_b()

def use_c() -> int:
  return make_c()
"#;
    let consumer = build_with_imports(
        consumer_source,
        &["d"],
        &[
            ("a", &["a"], shared_source),
            ("b", &["b"], shared_source),
            ("c", &["c"], shared_source),
        ],
    )?;

    let mut facts = Vec::new();
    for body in ["use_a", "use_b", "use_c"] {
        let targets = named_targets(&consumer, body);
        let [target] = targets.as_slice() else {
            return Err(Box::from(format!(
                "expected one named call in `{body}`, got {}",
                targets.len()
            )));
        };
        let Some(fact) = &target.canonical else {
            return Err(Box::from(format!("`{body}` must carry an identity")));
        };
        facts.push(fact.clone());
    }

    // The premise: everything except origin is identical, so origin alone carries the distinction.
    for fact in &facts {
        assert_eq!(fact.declaration_name, "make");
        assert_eq!(fact.kind, SemanticSourceTargetKind::Function);
    }
    assert_eq!(facts[0].declaration_span, facts[1].declaration_span);
    assert_eq!(facts[1].declaration_span, facts[2].declaration_span);

    for (fact, module) in facts.iter().zip(["a", "b", "c"]) {
        assert_eq!(
            fact.origin,
            incan_semantics_core::SymbolOrigin::Module(vec![module.to_string()]),
            "each call must name the module it imported from"
        );
    }
    assert_ne!(facts[0], facts[1]);
    assert_ne!(facts[1], facts[2]);
    assert_ne!(facts[0], facts[2]);

    // One declaration reached two ways — locally in `a`, and through `d`'s alias — stays one identity.
    let a_module = build(shared_source, &["a"])?;
    let local = named_targets(&a_module, "use_local");
    let [local] = local.as_slice() else {
        return Err(Box::from("expected one named call in `use_local`".to_string()));
    };
    let Some(local_fact) = &local.canonical else {
        return Err(Box::from("the local call must carry an identity".to_string()));
    };
    assert_eq!(
        *local_fact, facts[0],
        "`a`'s own `make` and `d`'s `make_a` are one declaration"
    );

    // And the seam refuses an identity this module does not own, despite the identical spelling and span.
    assert!(a_module.body_for_canonical_target(&facts[0]).is_some());
    assert!(
        a_module.body_for_canonical_target(&facts[1]).is_none(),
        "`a` must not answer for `b`'s identically-spelled, identically-spanned declaration"
    );
    Ok(())
}

/// A local declaration beside an explicitly aliased import is identified as the *local* declaration.
///
/// RFC 120 rejects an implicit same-spelling replacement of an import. The explicit alias is the valid spelling for
/// keeping both bindings active, and the local call must still carry the local declaration's identity.
#[test]
fn a_local_declaration_beside_an_aliased_import_is_identified_locally() -> Result<(), Box<dyn std::error::Error>> {
    let helpers_source = r#"
pub def render() -> int:
  return 1
"#;
    let app_source = r#"
from helpers import render as imported_render

def render(value: int) -> int:
  return value

def run() -> int:
  return render(7)
"#;
    let app = build_with_imports(app_source, &["app"], &[("helpers", &["helpers"], helpers_source)])?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from(
            "the call bound a local declaration and must carry its identity".to_string(),
        ));
    };

    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["app".to_string()]),
        "the local declaration owns this call; the explicitly aliased import is a separate binding"
    );
    // The two facts on one target must never name different declarations.
    let resolved = app
        .body_for_canonical_target(fact)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("this module owns the declaration and must resolve it"))?;
    assert_eq!(resolved.name, "render");
    assert_eq!(
        Some(&resolved.direct_call_id),
        target.direct_call_id.as_ref(),
        "the canonical identity and the span identity must select one declaration"
    );
    Ok(())
}

/// Bodies do not carry owner-qualified names, so one module can hold a class method `render` and a free function
/// `render`. The consumer seam must separate them by declaration span; matching on the declared name would hand
/// back whichever body came first, silently, for an identity that names the other one.
#[test]
fn the_consumer_seam_separates_same_named_bodies_by_declaration_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
class Canvas:
  def render(self) -> int:
    return 1

def render() -> int:
  return 2

def run() -> int:
  return render()
"#;
    let module = build(source, &["app"])?;

    let same_named: Vec<&bir::Body> = module.bodies.iter().filter(|body| body.name == "render").collect();
    assert_eq!(
        same_named.len(),
        2,
        "this fixture is only meaningful while the module really holds two bodies named `render`"
    );

    let targets = named_targets(&module, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from("the free function call must carry an identity".to_string()));
    };

    let resolved = module
        .body_for_canonical_target(fact)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("the owning module must resolve its own identity"))?;
    assert_eq!(resolved.span, fact.declaration_span);
    assert_eq!(
        resolved.block.stmts.len(),
        module
            .bodies
            .iter()
            .find(|body| body.span == fact.declaration_span)
            .map(|body| body.block.stmts.len())
            .unwrap_or_default()
    );
    // The method body shares the spelling and must not be what the seam returns.
    let method_span = same_named
        .iter()
        .map(|body| body.span)
        .find(|span| *span != fact.declaration_span)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("expected a second, differently-spanned `render`"))?;
    assert_ne!(resolved.span, method_span);
    Ok(())
}

/// A re-export chain longer than one hop, with a rename in the middle. Exercises the recursion in
/// `dependency_member_identity_from` and proves a rename never leaks into `declaration_name`.
#[test]
fn a_renamed_multi_hop_re_export_still_resolves_to_the_original_declaration() -> Result<(), Box<dyn std::error::Error>>
{
    let helpers_source = r#"
pub def render() -> int:
  return 1
"#;
    let inner_source = r#"
from helpers import render as painted
"#;
    let facade_source = r#"
from inner import painted
"#;
    let app_source = r#"
from facade import painted as relayed

def run() -> int:
  return relayed()
"#;
    let app = build_with_imports(
        app_source,
        &["app"],
        &[
            ("helpers", &["helpers"], helpers_source),
            ("inner", &["inner"], inner_source),
            ("facade", &["facade"], facade_source),
        ],
    )?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from(
            "a multi-hop re-export resolves to a declaration and must carry an identity".to_string(),
        ));
    };
    assert_eq!(
        fact.declaration_name, "render",
        "neither `painted` nor `relayed` may become the declared name"
    );
    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["helpers".to_string()]),
        "the origin is the declaring module, not either facade"
    );
    assert_eq!(target.name, "relayed", "the call site keeps its own spelling");
    Ok(())
}

/// The dependency cache key is the flattened, underscore-joined module name, which also names the emitted Rust
/// module and is therefore not injective: the path `pkg.helpers` and a module literally named `pkg_helpers` are
/// one key. Registering the real segments makes the identity name the module that answered rather than the
/// spelling that asked — and note a leading-underscore segment like `pkg._helpers` is exactly what defeats an
/// escaping scheme, which is why the real segments are carried instead of being encoded into one string.
#[test]
fn a_registered_module_path_wins_over_the_matching_candidate_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let helpers_source = r#"
pub def render() -> int:
  return 1
"#;
    let app_source = r#"
from helpers import render

def run() -> int:
  return render()
"#;
    let helpers_tokens = lexer::lex(helpers_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let helpers_program = parser::parse(&helpers_tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let tokens = lexer::lex(app_source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let module_path = vec!["pkg".to_string(), "app".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    // One real module, a single segment literally named `pkg_helpers`, reached from `pkg.app` as sibling
    // `helpers`. Without the registration the matching candidate would spell a `pkg::helpers` that does not exist.
    checker.register_dependency_module_path_segments("pkg_helpers", vec!["pkg_helpers".to_string()]);
    checker
        .check_with_imports(&program, &[("pkg_helpers", &helpers_program)])
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let app = build_body_ir_module_v0(&program, &module_path, checker.type_info());

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from(
            "the import resolves to a declaration and must carry an identity".to_string(),
        ));
    };
    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["pkg_helpers".to_string()]),
        "the origin must be the module that answered, not the candidate spelling that matched its flattened name"
    );
    Ok(())
}

/// Import resolution tries the sibling-relative candidate before the bare one, so the path written at an import is
/// not necessarily the module it bound. An identity built from the written path would name the root module's
/// declaration here — a different function that merely shares the name.
#[test]
fn a_sibling_relative_import_is_owned_by_the_module_resolution_actually_selected()
-> Result<(), Box<dyn std::error::Error>> {
    // Distinguishable by arity: the zero-argument call below only typechecks against the sibling.
    let root_helpers = r#"
pub def render(first: int, second: int) -> int:
  return first + second
"#;
    let sibling_helpers = r#"
pub def render() -> int:
  return 1
"#;
    let app_source = r#"
from helpers import render

def run() -> int:
  return render()
"#;
    let app = build_with_imports(
        app_source,
        &["pkg", "app"],
        &[
            ("helpers", &["helpers"], root_helpers), /* The sibling is genuinely the nested module
                                                      * `pkg.helpers`, not a module named `pkg_helpers`. */
            ("pkg_helpers", &["pkg", "helpers"], sibling_helpers),
        ],
    )?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from(
            "a sibling-relative import resolves to a proven declaration and must carry an identity".to_string(),
        ));
    };

    assert_eq!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["pkg".to_string(), "helpers".to_string()]),
        "the origin must be the module resolution selected, not the path the import spelled"
    );
    assert_ne!(
        fact.origin,
        incan_semantics_core::SymbolOrigin::Module(vec!["helpers".to_string()]),
        "naming the written path would collide with the root module's unrelated `render`"
    );
    // Origin alone would still pass if a name-keyed span lookup picked the wrong file's declaration.
    assert_eq!(
        fact.declaration_span,
        HirSourceSpan::new(1, 37),
        "the span must be the sibling's zero-argument declaration, not the root's two-argument one"
    );
    Ok(())
}

/// An imported overload is selected at the call site, so its identity must carry that checked selection through an
/// ordinary import and an alias instead of falling back to the overloaded spelling.
#[test]
fn imported_overload_calls_retain_the_selected_canonical_identity() -> Result<(), Box<dyn std::error::Error>> {
    let helpers_source = r#"
pub def render(value: int) -> int:
  return value

pub def render(value: str) -> int:
  return 1
"#;
    let app_source = r#"
from helpers import render
from helpers import render as draw

def use_imported() -> int:
  return render(2)

def use_alias() -> int:
  return draw(3)
"#;
    let app = build_with_imports(app_source, &["app"], &[("helpers", &["helpers"], helpers_source)])?;

    let helpers = build(helpers_source, &["helpers"])?;
    let overload_spans = helpers
        .bodies
        .iter()
        .filter(|body| body.name == "render")
        .map(|body| body.span)
        .collect::<Vec<_>>();
    let [int_overload_span, str_overload_span] = overload_spans.as_slice() else {
        return Err(format!("expected both helper overload bodies, got {overload_spans:?}").into());
    };
    let mut selected = Vec::new();
    for body in ["use_imported", "use_alias"] {
        let targets = named_targets(&app, body);
        let [target] = targets.as_slice() else {
            return Err(Box::from(format!(
                "expected one named call in `{body}`, got {}",
                targets.len()
            )));
        };
        let canonical = target
            .canonical
            .as_ref()
            .ok_or_else(|| format!("`{body}` must retain the overload selected by typechecking"))?;
        assert_eq!(canonical.declaration_span, *int_overload_span);
        assert_ne!(canonical.declaration_span, *str_overload_span);
        assert_eq!(canonical.declaration_name, "render");
        assert_eq!(canonical.kind, SemanticSourceTargetKind::Function);
        assert_eq!(
            canonical.origin,
            incan_semantics_core::SymbolOrigin::Module(vec!["helpers".to_string()])
        );
        selected.push(canonical.clone());
    }
    assert_eq!(
        selected[0], selected[1],
        "the alias must preserve the selected declaration identity"
    );
    Ok(())
}

/// The consumer seam: an identity resolves to a declaration, or to nothing. It must never be satisfied by a
/// same-named declaration that happens to live in the consuming module.
#[test]
fn a_canonical_identity_resolves_to_its_declaration_only_in_the_owning_module() -> Result<(), Box<dyn std::error::Error>>
{
    let helpers_source = r#"
pub def render() -> int:
  return 1
"#;
    // `app` declares its own same-named `render`, so a seam keyed on the spelling would wrongly match it.
    let app_source = r#"
from helpers import render as draw

def render() -> int:
  return 2

def run() -> int:
  return draw()
"#;
    let helpers = build(helpers_source, &["helpers"])?;
    let app = build_with_imports(app_source, &["app"], &[("helpers", &["helpers"], helpers_source)])?;

    let targets = named_targets(&app, "run");
    let [target] = targets.as_slice() else {
        return Err(Box::from(format!("expected one named call, got {}", targets.len())));
    };
    let Some(fact) = &target.canonical else {
        return Err(Box::from("the aliased import must carry an identity".to_string()));
    };

    let owning = helpers
        .body_for_canonical_target(fact)
        .ok_or_else(|| Box::<dyn std::error::Error>::from("owning module must resolve the identity"))?;
    assert_eq!(owning.name, "render");
    assert!(
        app.body_for_canonical_target(fact).is_none(),
        "the consuming module's own same-named `render` must not satisfy an identity it does not own"
    );
    Ok(())
}

/// Two same-name declarations in one module get two identities, because the identity anchors to a declaration
/// span rather than to the spelling. The *spelling* cannot separate overloads; the identity can, and the
/// typechecker's per-call-site overload selection is what tells them apart.
#[test]
fn each_local_overload_gets_its_own_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def render(value: int) -> int:
  return value

def render(value: str) -> int:
  return 1

def use_int() -> int:
  return render(2)

def use_str() -> int:
  return render("x")
"#;
    let module = build(source, &["app"])?;

    let mut facts = Vec::new();
    for body in ["use_int", "use_str"] {
        let targets = named_targets(&module, body);
        let [target] = targets.as_slice() else {
            return Err(Box::from(format!(
                "expected one named call in `{body}`, got {}",
                targets.len()
            )));
        };
        let Some(fact) = &target.canonical else {
            return Err(Box::from(format!(
                "`{body}` selected one overload and must carry its identity"
            )));
        };
        // Refusing to name an overload must not cost the span dispatch, and the two must agree.
        assert!(target.direct_call_id.is_some());
        facts.push(fact.clone());
    }

    assert_ne!(
        facts[0], facts[1],
        "two overloads are two declarations and must not collapse to one identity"
    );
    assert_eq!(facts[0].declaration_name, "render");
    assert_eq!(facts[1].declaration_name, "render");

    // Each identity resolves to the declaration whose signature that call actually selected.
    for fact in &facts {
        let resolved = module
            .body_for_canonical_target(fact)
            .ok_or_else(|| Box::<dyn std::error::Error>::from("this module owns both overloads"))?;
        assert_eq!(resolved.name, "render");
        assert_eq!(resolved.span, fact.declaration_span);
    }
    Ok(())
}
