//! Syntactic regions collected from declarations, methods and bodies, and their resolution per byte.

use super::*;

/// Collect every AST byte region that refines the token stream's default identifier classification.
///
/// Only slots the parser records with a span can contribute. Where the AST offers nothing — a call's callee, a
/// field access — the token-stream rules in [`classify_token_stream`] answer instead.
pub(super) fn collect_regions(program: &Program) -> Vec<Region> {
    let mut collector = RegionCollector::default();
    for decl in &program.declarations {
        collect_declaration_regions(&decl.node, &mut collector);
    }
    let mut regions = collector.regions;
    regions.sort_by_key(|region| (region.start, region.width()));
    regions
}

/// Collect the regions one declaration contributes, recursing into nested declarations and bodies.
pub(super) fn collect_declaration_regions(decl: &Declaration, out: &mut RegionCollector) {
    match decl {
        Declaration::Function(function) => {
            for decorator in &function.decorators {
                out.push(decorator.span, RegionKind::Decorator);
            }
            for type_param in &function.type_params {
                out.push(type_param.span, RegionKind::TypeParameter);
            }
            for param in &function.params {
                out.param(param);
            }
            out.ty(&function.return_type);
            if let Some(source) = &function.result_source {
                out.push(source.node.binding.span, RegionKind::Parameter);
            }
            collect_body_regions(&function.body, out);
        }
        Declaration::Model(model) => {
            collect_type_decl_regions(&model.decorators, &model.type_params, &model.traits, out);
            for field in &model.fields {
                out.ty(&field.node.ty);
            }
            for property in &model.properties {
                out.ty(&property.node.return_type);
                if let Some(body) = &property.node.body {
                    collect_body_regions(body, out);
                }
            }
            for method in &model.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::Class(class) => {
            collect_type_decl_regions(&class.decorators, &class.type_params, &class.traits, out);
            for field in &class.fields {
                out.ty(&field.node.ty);
            }
            for property in &class.properties {
                out.ty(&property.node.return_type);
                if let Some(body) = &property.node.body {
                    collect_body_regions(body, out);
                }
            }
            for method in &class.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::Trait(trait_decl) => {
            collect_type_decl_regions(&trait_decl.decorators, &trait_decl.type_params, &trait_decl.traits, out);
            for property in &trait_decl.properties {
                out.ty(&property.node.return_type);
                if let Some(body) = &property.node.body {
                    collect_body_regions(body, out);
                }
            }
            for method in &trait_decl.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::Enum(enum_decl) => {
            collect_type_decl_regions(&enum_decl.decorators, &enum_decl.type_params, &enum_decl.traits, out);
            for variant in &enum_decl.variants {
                for field in &variant.node.fields {
                    out.ty(field);
                }
            }
            for method in &enum_decl.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::Newtype(newtype) => {
            collect_type_decl_regions(&newtype.decorators, &newtype.type_params, &newtype.traits, out);
            out.ty(&newtype.underlying);
            for method in &newtype.methods {
                collect_method_regions(&method.node, out);
            }
        }
        Declaration::TypeAlias(alias) => {
            for type_param in &alias.type_params {
                out.push(type_param.span, RegionKind::TypeParameter);
            }
            out.ty(&alias.target);
        }
        Declaration::Const(constant) => {
            if let Some(ty) = &constant.ty {
                out.ty(ty);
            }
        }
        Declaration::Static(value) => out.ty(&value.ty),
        Declaration::TestModule(module) => {
            for nested in &module.body {
                collect_declaration_regions(&nested.node, out);
            }
        }
        _ => {}
    }
}

/// Collect the decorator, type-parameter and trait-bound regions shared by every nominal type declaration.
pub(super) fn collect_type_decl_regions(
    decorators: &[Spanned<incan_syntax::ast::Decorator>],
    type_params: &[incan_syntax::ast::TypeParam],
    traits: &[Spanned<incan_syntax::ast::TraitBound>],
    out: &mut RegionCollector,
) {
    for decorator in decorators {
        out.push(decorator.span, RegionKind::Decorator);
    }
    for type_param in type_params {
        out.push(type_param.span, RegionKind::TypeParameter);
    }
    for bound in traits {
        out.push(bound.span, RegionKind::Type);
    }
}

/// Collect the regions one method contributes.
pub(super) fn collect_method_regions(method: &incan_syntax::ast::MethodDecl, out: &mut RegionCollector) {
    for decorator in &method.decorators {
        out.push(decorator.span, RegionKind::Decorator);
    }
    for type_param in &method.type_params {
        out.push(type_param.span, RegionKind::TypeParameter);
    }
    if let Some(target) = &method.trait_target {
        out.push(target.span, RegionKind::Type);
    }
    for param in &method.params {
        out.param(param);
    }
    out.ty(&method.return_type);
    if let Some(source) = &method.result_source {
        out.push(source.node.binding.span, RegionKind::Parameter);
    }
    if let Some(body) = &method.body {
        collect_body_regions(body, out);
    }
}

/// Collect regions from a statement body, reaching annotated local bindings inside nested blocks.
///
/// `let total: Decimal = ...` is the one statement-level slot carrying a type, and `AssignmentStmt` records both the
/// bound name's span and that annotation, so both are recoverable without re-parsing.
pub(super) fn collect_body_regions(body: &[Spanned<Statement>], out: &mut RegionCollector) {
    for stmt in body {
        match &stmt.node {
            Statement::Assignment(assignment) => {
                out.push(assignment.name_span, RegionKind::Variable);
                if let Some(ty) = &assignment.ty {
                    out.ty(ty);
                }
            }
            Statement::If(if_stmt) => {
                collect_body_regions(&if_stmt.then_body, out);
                if let Some(else_body) = &if_stmt.else_body {
                    collect_body_regions(else_body, out);
                }
            }
            Statement::While(while_stmt) => collect_body_regions(&while_stmt.body, out),
            Statement::Loop(loop_stmt) => collect_body_regions(&loop_stmt.body, out),
            Statement::For(for_stmt) => collect_body_regions(&for_stmt.body, out),
            Statement::Unsafe(unsafe_stmt) => collect_body_regions(&unsafe_stmt.body, out),
            _ => {}
        }
    }
}

/// Resolve every AST region to a per-byte answer, so classification can ask about an offset in constant time.
///
/// Regions nest — a parameter contains its annotation, a generic type contains its arguments — and the narrowest
/// one covering an offset is the correct answer. Asking that per identifier by scanning the region list is
/// `O(identifiers × regions)`, which measured at 4.3 seconds for an 84 KB file; the language server recomputes this
/// on every keystroke, so that is not a cost it can carry. Painting widest-first lets the narrowest region land
/// last and win, and turns the per-token question into an index.
pub(super) fn resolve_regions(regions: &[Region], len: usize) -> Vec<Option<RegionKind>> {
    let mut map = vec![None; len];
    let mut ordered: Vec<&Region> = regions.iter().collect();
    ordered.sort_by_key(|region| std::cmp::Reverse(region.width()));
    for region in ordered {
        for slot in map.iter_mut().take(region.end.min(len)).skip(region.start.min(len)) {
            *slot = Some(region.kind);
        }
    }
    map
}
